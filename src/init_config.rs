use std::{path::PathBuf, process::Command, time::Duration};

use anyhow::{Context, Result};
use indicatif::ProgressBar;
use tracing::trace;

use crate::config::{Config, OcrmypdfConfig, Scanner, Tools};

/// OCR language option
#[derive(Debug, Clone)]
struct OcrLanguage {
    name: &'static str,
    code: &'static str,
}

impl std::fmt::Display for OcrLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// List of OCR languages supported by the OCRmyPDF docker image
const OCR_LANGUAGES: &[OcrLanguage] = &[
    OcrLanguage {
        name: "English",
        code: "eng",
    },
    OcrLanguage {
        name: "Chinese (Simplified)",
        code: "chi-sim",
    },
    OcrLanguage {
        name: "German",
        code: "deu",
    },
    OcrLanguage {
        name: "French",
        code: "fra",
    },
    OcrLanguage {
        name: "Portuguese",
        code: "por",
    },
    OcrLanguage {
        name: "Spanish",
        code: "spa",
    },
    OcrLanguage {
        name: "Orientation and script detection",
        code: "osd",
    },
];

/// PDF viewer option
#[derive(Debug, Clone)]
struct PdfViewer {
    name: &'static str,
    binary: &'static str,
}

impl std::fmt::Display for PdfViewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// List of PDF viewer options
const PDF_VIEWERS: &[PdfViewer] = &[
    PdfViewer {
        name: "System default (through xdg-open)",
        binary: "xdg-open",
    },
    PdfViewer {
        name: "Evince",
        binary: "evince",
    },
    PdfViewer {
        name: "Okular",
        binary: "okular",
    },
    PdfViewer {
        name: "Atril",
        binary: "atril",
    },
    PdfViewer {
        name: "Zathura",
        binary: "zathura",
    },
    PdfViewer {
        name: "MuPDF",
        binary: "mupdf",
    },
    PdfViewer {
        name: "Xpdf",
        binary: "xpdf",
    },
    PdfViewer {
        name: "Foxit Reader",
        binary: "foxitreader",
    },
];

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

/// Run the init-config wizard
pub fn run_init_config() -> Result<()> {
    // Check if config already exists
    let config_path = Config::config_path()?;
    if config_path.exists() {
        anyhow::bail!(
            "Config file already exists at: {}\nRemove it first if you want to re-initialize.",
            config_path.display()
        );
    }

    // Detect scanners
    let detected_scanners = detect_scanners()?;
    if detected_scanners.is_empty() {
        println!("No scanners detected. Please ensure your scanners are turned on and accessible.");
        return Ok(());
    }

    // Ask user which scanners to add
    let mut selected_scanners = Vec::new();
    for scanner in detected_scanners {
        let add = inquire::Confirm::new(&format!(
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
        anyhow::bail!("No scanners detected or selected.");
    }

    // For each selected scanner, detect and classify sources
    let mut scanners = Vec::new();
    for detected in selected_scanners {
        let scanner = configure_scanner(detected)?;
        scanners.push(scanner);
    }

    // Ask for output directory
    let mut output_directory: Option<PathBuf> = None;
    while output_directory.is_none() {
        let path_str = inquire::Text::new("Output directory path:").prompt()?;
        let path = PathBuf::from(&path_str);
        if !path.exists() {
            println!("Path {:?} does not exist.", &path);
            continue;
        }
        if !path.is_dir() {
            println!("Path {:?} is not a directory.", &path);
            continue;
        }
        output_directory = Some(path);
    }

    // Ask for OCR languages
    let ocr_languages = {
        let choices = inquire::MultiSelect::new(
            "Which languages do you want to support with OCR?",
            OCR_LANGUAGES.to_vec(),
        )
        .with_default(&[0])
        .with_validator(inquire::min_length!(1, "Please select at least one option"))
        .prompt()?;

        choices
            .iter()
            .map(|lang| lang.code)
            .collect::<Vec<_>>()
            .join("+")
    };

    // Ask for PDF viewer
    let pdf_viewer = {
        let filtered_options = PDF_VIEWERS
            .iter()
            .filter(|viewer| which::which(viewer.binary).is_ok())
            .collect::<Vec<_>>();
        if filtered_options.is_empty() {
            anyhow::bail!("Could not find any available PDF viewer");
        }
        inquire::Select::new("Which PDF viewer do you want to use?", filtered_options).prompt()?
    };

    // Create config struct
    let config = Config {
        output_directory: output_directory.expect("Output directory is None"),
        tools: Tools {
            ocrmypdf: OcrmypdfConfig {
                language: ocr_languages,
            },
            pdf_viewer: pdf_viewer.binary.to_string(),
        },
        scanners,
        authors: vec![],
    };

    // Print
    println!("Writing config to {:?}:", config_path);
    println!("  Output directory: {:?}", &config.output_directory);
    println!("  Scanners:");
    for scanner in &config.scanners {
        println!("  - {}", scanner.name);
    }
    println!("  OCR languages: {:?}", &config.tools.ocrmypdf.language);
    println!("  PDF viewer: {:?}", &config.tools.pdf_viewer);

    // Save
    config.save(None)?;

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
            trace!("Parsing line: {:?}", line);
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
    let sources = detect_sources(&detected.device_name, &detected.description)?;

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
fn detect_sources(device_name: &str, description: &str) -> Result<Vec<String>> {
    let spinner = ProgressBar::new_spinner()
        .with_message(format!("Detecting sources for '{}'...", description));
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
            trace!("Parsing sources: {:?}", line);
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
    trace!("Classify source: {:?}", source);

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

    let answer = inquire::Select::new(
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
            let answer = inquire::Select::new(
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
