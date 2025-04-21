use std::{fmt::Display, fs, path::Path, process::Command, time::Duration};

use anyhow::{Context, Result, anyhow, ensure};
use tracing::{debug, trace, warn};

use crate::{
    config::{Scanner, ScannerSources},
    fs_utils,
};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum ScanMode {
    AdfSingleSided,
    AdfDuplex,
    AdfManualDuplex,
    Flatbed { page_count: usize },
}

impl Display for ScanMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanMode::AdfSingleSided => write!(f, "ADF single sided"),
            ScanMode::AdfDuplex => write!(f, "ADF duplex"),
            ScanMode::AdfManualDuplex => write!(f, "ADF manual duplex"),
            ScanMode::Flatbed { .. } => write!(f, "Flatbed"),
        }
    }
}

impl ScanMode {
    fn options(available_sources: &ScannerSources) -> Vec<Self> {
        let mut options = Vec::new();
        if available_sources.adf_single.is_some() {
            options.push(ScanMode::AdfSingleSided);
        }
        if available_sources.adf_duplex.is_some() {
            options.push(ScanMode::AdfDuplex);
        }
        if available_sources.adf_single.is_some() {
            options.push(ScanMode::AdfManualDuplex);
        }
        if available_sources.flatbed.is_some() {
            options.push(ScanMode::Flatbed { page_count: 0 });
        }
        options
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Resolution {
    /// 300 DPI
    Normal,
    /// 600 DPI
    High,
}

impl Resolution {
    fn as_dpi(&self) -> u32 {
        match self {
            Resolution::Normal => 300,
            Resolution::High => 600,
        }
    }
}

impl Default for Resolution {
    fn default() -> Self {
        Resolution::Normal
    }
}

/// Scan one or more pages using `scanimage`
///
/// Scanned files will be stored as TIF files in the scans cache directory. The
/// filename contains a number starting at 1000.
fn run_scanimage(
    scans_dir: &Path,
    context: &ScanContext,
    mode: &ScanMode,
    resolution: &Resolution,
) -> Result<()> {
    debug!("Scanning to {}", scans_dir.display());

    // TODO: Manual duplex

    // Macro to reduce repetition in source checking
    macro_rules! get_source {
        ($field:ident, $desc:expr) => {
            context.scanner.sources.$field.as_ref().ok_or_else(|| {
                anyhow!("{} not available for scanner {}", $desc, context.scanner.id)
            })
        };
    }

    // Determine source string
    let source = match mode {
        ScanMode::AdfSingleSided => get_source!(adf_single, "ADF single-sided"),
        ScanMode::AdfDuplex => get_source!(adf_duplex, "ADF duplex"),
        ScanMode::AdfManualDuplex => get_source!(adf_single, "ADF manual duplex"),
        ScanMode::Flatbed { .. } => get_source!(flatbed, "Flatbed"),
    }?;

    // Call scanimage
    match mode {
        ScanMode::AdfSingleSided | ScanMode::AdfDuplex | ScanMode::AdfManualDuplex => {
            // Scan all available pages from ADF
            _scanimage(scans_dir, context, source, 0, None, resolution)?;
        }
        ScanMode::Flatbed { page_count } => {
            assert!(
                *page_count > 0,
                "Page count is 0, this indicates an internal logic bug"
            );
            // Scan n pages from flatbed
            for i in 0..*page_count {
                let scan_next_page =
                    inquire::Confirm::new(&format!("Scan page {}/{}?", i + 1, page_count))
                        .with_default(true)
                        .with_help_message(
                            "Press enter to scan, or type 'n' to abort the scan process.",
                        )
                        .prompt()?;
                if !scan_next_page {
                    return Err(anyhow!("Scan aborted by user"));
                }
                _scanimage(scans_dir, context, source, i, Some(1), resolution)?;
            }
        }
    }

    Ok(())
}

/// Low-level function to call the `scanimage` binary.
///
/// Parameters:
///   scans_dir:
///     The directory where the scanned pages will be saved.
///   context:
///     The scan context.
///   source:
///     The scanner source.
///   start:
///     The batch offset. If this is set to 0, the filename of the first
///     scanned page will be `1000.tif`. If it's set to 4, the filename
///     of the first scanned page will be `1004.tif`.
///   count:
///     The number of pages to scan. If this is `None`, no count will be passed
///     to `scanimage` (i.e. all available pages will be scanned).
///   resolution:
///     The resolution of the scanned pages.
fn _scanimage(
    scans_dir: &Path,
    context: &ScanContext,
    source: &str,
    start: usize,
    count: Option<usize>,
    resolution: &Resolution,
) -> Result<()> {
    let mut args = Vec::new();

    // Generic scanimage parameters
    args.push("--format=tiff".into());
    args.push(format!("--batch={}", scans_dir.join("%d.tif").display()));
    args.push(format!("--batch-start={}", 1000 + start));
    if let Some(batch_count) = count {
        args.push(format!("--batch-count={}", batch_count));
    }

    // Common scanner-specific parameters for which we assume support by all scanners
    args.push(format!("--resolution={}", resolution.as_dpi()));
    args.push("-x".into());
    args.push("210".into());
    args.push("-y".into());
    args.push("297".into());

    // Scanner-specific arguments
    args.push(format!("--source={}", source));

    // Additional arguments from scanner config
    args.extend_from_slice(&context.scanner.additional_args);

    debug!("Calling `scanimage` with arguments: {:?}", args);

    // Show spinner
    let spinner_message = if context.fake_scan {
        "Faking `scanimage` to scan documents…"
    } else {
        "Calling `scanimage` to scan documents…"
    };
    let spinner = indicatif::ProgressBar::new_spinner().with_message(spinner_message);
    spinner.enable_steady_tick(Duration::from_millis(100));

    // Run or fake command
    if context.fake_scan {
        fake_scanimage(scans_dir).context("Failed to fake `scanimage` command")?;
        spinner.finish_with_message(format!(
            "Simulated document scan in {:.1}s",
            spinner.elapsed().as_secs_f32()
        ));
    } else {
        let output = Command::new("scanimage").args(&args).output()?;
        if output.status.success() {
            spinner.finish_with_message(format!(
                "Scanned documents in {:.1}s",
                spinner.elapsed().as_secs_f32()
            ));
        } else {
            spinner.abandon_with_message(format!(
                "Failed to scan documents after {:.1}s",
                spinner.elapsed().as_secs_f32()
            ));
            warn!(
                "Scanimage failed with status {}. Stderr: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr),
            );
            return Err(anyhow!(
                "Call to `scanimage` failed with non-successful exit status ({}). Ensure that device is running and reachable.",
                output.status,
            ));
        }
    }

    Ok(())
}

/// Fake scanimage function for testing purposes
///
/// Note that this will only work, if a `testdata` folder exists in the current
/// working directory.
fn fake_scanimage(scans_dir: &Path) -> Result<()> {
    debug!("Faking scan to {}", scans_dir.display());

    let testdata_dir = Path::new("testdata");
    ensure!(
        testdata_dir.exists(),
        "`testdata` folder not found in current working directory"
    );
    ensure!(testdata_dir.is_dir(), "`testdata` is not a directory");

    std::thread::sleep(Duration::from_secs(2));

    fs_utils::copy_dir_file_contents(testdata_dir, scans_dir)?;

    Ok(())
}

/// Select a device from the list of available scanners
pub fn select_scanner(scanners: &[Scanner]) -> Result<Scanner> {
    // If there is only one device, return it
    if scanners.len() == 1 {
        trace!("Only one scanner available, using it");
        return Ok(scanners[0].clone());
    }

    // Otherwise, rompt the user to select a scan device
    trace!(
        "{} scanners available, asking user for selection",
        scanners.len()
    );
    Ok(inquire::Select::new("Which device do you want to use?", scanners.to_vec()).prompt()?)
}

pub struct ScanContext<'a> {
    /// The scanner to use for scanning
    pub scanner: &'a Scanner,

    /// Whether to fake scanning
    pub fake_scan: bool,
}

/// Scan a document
pub fn scan_document(context: &ScanContext) -> Result<()> {
    let scanner = context.scanner;

    // Determine the XDG cache directory, creating it if it doesn't exist
    let scans_dir = app_dirs::app_dir(app_dirs::AppDataType::UserCache, &crate::APP_INFO, "scans")
        .context("Could not determine XDG app cache directory for scans")?;

    // Ensure that "current" scan directory exists and is empty
    let current_dir = scans_dir.join("current");
    fs_utils::ensure_empty_dir_exists(&current_dir)?;

    // Determine scan mode
    let mut mode =
        inquire::Select::new("How to scan?", ScanMode::options(&scanner.sources)).prompt()?;

    // Determine number of pages to scan
    if matches!(mode, ScanMode::Flatbed { .. }) {
        let page_count = inquire::CustomType::<usize>::new("Number of pages to scan?")
            .with_default(1)
            .with_validator(|input: &usize| {
                Ok(if *input > 0 {
                    inquire::validator::Validation::Valid
                } else {
                    inquire::validator::Validation::Invalid("Please enter a number ≥ 1".into())
                })
            })
            .with_error_message("Please enter a valid number ≥ 1")
            .prompt()?;
        mode = ScanMode::Flatbed { page_count };
    };

    // Determine scan options
    let option_highdpi = "High resolution (600dpi instead of 300dpi)";
    let options = inquire::MultiSelect::new(
        "Choose options (if desired) and press enter to start scanning!",
        vec![option_highdpi],
    )
    .prompt()?;
    let resolution = if options.contains(&option_highdpi) {
        Resolution::High
    } else {
        Resolution::Normal
    };
    trace!(
        "Using resolution {:?} ({}dpi)",
        resolution,
        resolution.as_dpi()
    );

    // Run `scanimage` binary
    run_scanimage(&current_dir, context, &mode, &resolution)
        .context("Failed to run `scanimage` command")?;

    // Rename current scan directory
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let new_dir = scans_dir.join(timestamp);
    fs::rename(&current_dir, &new_dir)?;

    Ok(())
}
