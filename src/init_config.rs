use std::{process::Command, time::Duration};

use anyhow::{Context, Result};
use indicatif::ProgressBar;
use inquire::{Confirm, Select};

use crate::config::{Config, Scanner};

/// A scanner detected by `scanimage -L`
#[derive(Debug, serde::Serialize)]
struct DetectedScanner {
    device_name: String,
    description: String,
}

/// Classification of a scanner source
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceType {
    Flatbed,
    AdfSingle,
    AdfDuplex,
}

impl SourceType {
    fn as_str(&self) -> &'static str {
        match self {
            SourceType::Flatbed => "Flatbed",
            SourceType::AdfSingle => "ADF single-sided",
            SourceType::AdfDuplex => "ADF duplex",
        }
    }
}

/// Result of the scanner detection process
#[derive(Debug)]
#[allow(dead_code)]
pub struct ScannerDetectionResult {
    pub scanners: Vec<Scanner>,
}

/// Run the init-config wizard
pub fn run_init_config() -> Result<()> {
    // 1. Check if config already exists
    let config_path = Config::config_path()?;
    if config_path.exists() {
        anyhow::bail!(
            "Config file already exists at: {}\nRemove it first if you want to re-initialize.",
            config_path.display()
        );
    }

    // 2. Detect scanners
    let detected_scanners = detect_scanners()?;
    if detected_scanners.is_empty() {
        println!("No scanners detected.");
        return Ok(());
    }

    // 3. Ask user which scanners to add
    let mut selected_scanners = Vec::new();
    for scanner in detected_scanners {
        let add = Confirm::new(&format!(
            "Add scanner '{}' ({})?",
            scanner.description, scanner.device_name
        ))
        .with_default(true)
        .prompt()?;

        if add {
            selected_scanners.push(scanner);
        }
    }
    if selected_scanners.is_empty() {
        println!("No scanners selected.");
        return Ok(());
    }

    // 4. For each selected scanner, detect and classify sources
    let mut scanners = Vec::new();
    for detected in selected_scanners {
        let scanner = configure_scanner(detected)?;
        scanners.push(scanner);
    }

    // 5. Print debug output
    let result = ScannerDetectionResult { scanners };
    println!("{:#?}", result);

    Ok(())
}

/// Detect available scanners using `scanimage -L`
fn detect_scanners() -> Result<Vec<DetectedScanner>> {
    let spinner = ProgressBar::new_spinner().with_message("Detecting scanners...");
    spinner.enable_steady_tick(Duration::from_millis(100));

    let output = Command::new("scanimage")
        .arg("-L")
        .output()
        .context("Failed to run `scanimage -L`")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        anyhow::bail!(
            "`scanimage -L` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_scanimage_list(&stdout)
}

/// Parse output of `scanimage -L`
/// Format: device `brother4:net1;dev0' is a Brother *brother-mfc9330cdw MFC-9330CDW
fn parse_scanimage_list(output: &str) -> Result<Vec<DetectedScanner>> {
    let re = regex::Regex::new(r"device `([^']+)' is a (.+)").context("Failed to compile regex")?;

    let mut scanners = Vec::new();
    for line in output.lines() {
        if let Some(caps) = re.captures(line) {
            scanners.push(DetectedScanner {
                device_name: caps[1].to_string(),
                description: caps[2].trim().to_string(),
            });
        }
    }
    Ok(scanners)
}

/// Detect sources for a scanner and configure it
fn configure_scanner(detected: DetectedScanner) -> Result<Scanner> {
    let sources = detect_sources(&detected.device_name)?;

    // Classify sources
    let mut flatbed_options: Vec<&str> = Vec::new();
    let mut adf_duplex_options: Vec<&str> = Vec::new();
    let mut adf_single_options: Vec<&str> = Vec::new();
    for source in &sources {
        match classify_source(source) {
            Some(SourceType::Flatbed) => flatbed_options.push(source),
            Some(SourceType::AdfDuplex) => adf_duplex_options.push(source),
            Some(SourceType::AdfSingle) => adf_single_options.push(source),
            None => {
                // Ask user to classify ambiguous source
                if let Some(source_type) = ask_source_classification(source)? {
                    match source_type {
                        SourceType::Flatbed => flatbed_options.push(source),
                        SourceType::AdfDuplex => adf_duplex_options.push(source),
                        SourceType::AdfSingle => adf_single_options.push(source),
                    }
                }
                // If None returned, user chose to skip
            }
        }
    }

    // For each source type with multiple options, ask user to choose
    let source_flatbed = select_source_option("flatbed", flatbed_options)?;
    let source_adf_single = select_source_option("ADF single-sided", adf_single_options)?;
    let source_adf_duplex = select_source_option("ADF duplex", adf_duplex_options)?;

    Ok(Scanner {
        name: detected.description,
        device_name: detected.device_name,
        additional_args: Vec::new(),
        source_adf_duplex,
        source_adf_single,
        source_flatbed,
    })
}

/// Detect available sources for a scanner
fn detect_sources(device_name: &str) -> Result<Vec<String>> {
    let spinner = ProgressBar::new_spinner().with_message("Detecting scanner config...");
    spinner.enable_steady_tick(Duration::from_millis(100));

    let output = Command::new("scanimage")
        .arg("-d")
        .arg(device_name)
        .arg("--help")
        .output()
        .context("Failed to run `scanimage --help`")?;

    spinner.finish_and_clear();

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_source_options(&stdout)
}

/// Parse `--source` options from `scanimage -d DEVICE --help` output
fn parse_source_options(output: &str) -> Result<Vec<String>> {
    let re = regex::Regex::new(r"--source\s+([^\[]+)\s*\[").context("Failed to compile regex")?;

    for line in output.lines() {
        if let Some(caps) = re.captures(line) {
            let options_str = caps[1].trim();
            return Ok(options_str
                .split('|')
                .map(|s| s.trim().to_string())
                .collect());
        }
    }

    Ok(Vec::new()) // No --source option found
}

/// Classify a source based on its name
fn classify_source(source: &str) -> Option<SourceType> {
    let lower = source.to_lowercase();

    // Check for duplex first (more specific)
    if lower.contains("duplex") {
        return Some(SourceType::AdfDuplex);
    }

    // Check for flatbed
    if lower.contains("flatbed") || lower.contains("flat bed") || lower.contains("platen") {
        return Some(SourceType::Flatbed);
    }

    // Check for ADF (without duplex, since we checked that first)
    if lower.contains("adf") || lower.contains("feeder") || lower.contains("document feeder") {
        return Some(SourceType::AdfSingle);
    }

    None // Ambiguous
}

/// Ask user to classify an ambiguous source
fn ask_source_classification(source: &str) -> Result<Option<SourceType>> {
    let options = vec![
        SourceType::Flatbed.as_str(),
        SourceType::AdfSingle.as_str(),
        SourceType::AdfDuplex.as_str(),
        "Skip (don't use this source)",
    ];

    let answer = Select::new(
        &format!("How should source '{}' be classified?", source),
        options,
    )
    .prompt()?;

    match answer {
        "Flatbed" => Ok(Some(SourceType::Flatbed)),
        "ADF single-sided" => Ok(Some(SourceType::AdfSingle)),
        "ADF duplex" => Ok(Some(SourceType::AdfDuplex)),
        _ => Ok(None), // Skip
    }
}

/// If multiple options exist for a source type, ask user to choose
fn select_source_option(source_type_name: &str, options: Vec<&str>) -> Result<Option<String>> {
    match options.len() {
        0 => Ok(None),
        1 => Ok(Some(options[0].to_string())),
        _ => {
            let answer = Select::new(
                &format!(
                    "Multiple {} sources detected. Which one should be used?",
                    source_type_name
                ),
                options.iter().map(|s| s.to_string()).collect(),
            )
            .prompt()?;
            Ok(Some(answer))
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    mod parse_scanimage_list {
        use super::*;

        #[test]
        fn parses_single_scanner() {
            let output =
                "device `brother4:net1;dev0' is a Brother *brother-mfc9330cdw MFC-9330CDW\n";
            let result = parse_scanimage_list(output).unwrap();

            insta::assert_yaml_snapshot!(result);
        }

        #[test]
        fn parses_multiple_scanners() {
            let output = r#"device `brother4:net1;dev0' is a Brother *brother-mfc9330cdw MFC-9330CDW
device `airscan:e1:HP ScanJet Flow N7000 snw1' is a HP ScanJet Flow N7000 snw1
"#;
            let result = parse_scanimage_list(output).unwrap();

            insta::assert_yaml_snapshot!(result);
        }

        #[test]
        fn ignores_non_device_lines() {
            let output = r#"Some other output line
device `brother4:net1;dev0' is a Brother *brother-mfc9330cdw MFC-9330CDW
Another line that doesn't match
"#;
            let result = parse_scanimage_list(output).unwrap();

            insta::assert_yaml_snapshot!(result);
        }

        #[test]
        fn handles_empty_output() {
            let output = "";
            let result = parse_scanimage_list(output).unwrap();

            assert!(result.is_empty());
        }
    }

    mod parse_source_options {
        use super::*;

        #[test]
        fn parses_brother_scanner_sources() {
            let output = include_str!("snapshots/scanimage-help-brother.txt");
            let result = parse_source_options(output).unwrap();

            assert_eq!(
                result,
                vec![
                    "FlatBed",
                    "Automatic Document Feeder(left aligned)",
                    "Automatic Document Feeder(left aligned,Duplex)",
                    "Automatic Document Feeder(centrally aligned)",
                    "Automatic Document Feeder(centrally aligned,Duplex)",
                ]
            );
        }

        #[test]
        fn handles_empty_output() {
            let output = "";
            let result = parse_source_options(output).unwrap();

            assert!(result.is_empty());
        }
    }

    mod classify_source {
        use super::*;

        #[rstest]
        #[case("FlatBed")]
        #[case("Flatbed")]
        #[case("flatbed")]
        #[case("flat bed")]
        #[case("FLATBED")]
        #[case("Platen")]
        #[case("platen")]
        fn classifies_flatbed(#[case] input: &str) {
            assert_eq!(classify_source(input), Some(SourceType::Flatbed));
        }

        #[rstest]
        #[case("Automatic Document Feeder(left aligned,Duplex)")]
        #[case("Automatic Document Feeder(centrally aligned,Duplex)")]
        #[case("ADF Duplex")]
        #[case("ADF DUPLEX")]
        #[case("duplex")]
        #[case("ADF with Duplex support")]
        #[case("duplex feeder")]
        #[case("duplex ADF")]
        fn classifies_adf_duplex(#[case] input: &str) {
            assert_eq!(classify_source(input), Some(SourceType::AdfDuplex));
        }

        #[rstest]
        #[case("Automatic Document Feeder(left aligned)")]
        #[case("Automatic Document Feeder(centrally aligned)")]
        #[case("ADF")]
        #[case("AUTOMATIC DOCUMENT FEEDER")]
        #[case("Document Feeder")]
        #[case("Feeder")]
        fn classifies_adf_single(#[case] input: &str) {
            assert_eq!(classify_source(input), Some(SourceType::AdfSingle));
        }

        #[rstest]
        #[case("Manual Feed")]
        #[case("Unknown Source")]
        #[case("")]
        fn returns_none_for_ambiguous(#[case] input: &str) {
            assert_eq!(classify_source(input), None);
        }
    }
}
