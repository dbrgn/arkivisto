use std::{fmt::Display, fs, path::Path, process::Command, time::Duration};

use anyhow::{Context, Result, anyhow, ensure};
use app_dirs::AppInfo;
use clap::Parser;
use indicatif::ProgressBar;
use tracing::{debug, level_filters::LevelFilter, trace, warn};
use tracing_subscriber::{filter::Targets, prelude::*};

mod args;
mod config;
mod fs_utils;

use config::{Scanner, ScannerSources};

pub const APP_INFO: AppInfo = AppInfo {
    name: "arkivisto",
    author: env!("CARGO_PKG_AUTHORS"),
};

fn initialize_tracing(level_filter: LevelFilter) -> Result<()> {
    let filter = Targets::new()
        .with_default(LevelFilter::WARN)
        .with_target(env!("CARGO_PKG_NAME"), level_filter);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter)
        .try_init()
        .context("Failed to initialize tracing")?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum ScanMode {
    SingleSidedAdf,
    DuplexAdf,
    ManualDuplexAdf,
}

impl Display for ScanMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanMode::SingleSidedAdf => write!(f, "Single sided from ADF"),
            ScanMode::DuplexAdf => write!(f, "Duplex from ADF"),
            ScanMode::ManualDuplexAdf => write!(f, "Manual duplex from ADF"),
        }
    }
}

impl ScanMode {
    fn options(available_sources: &ScannerSources) -> Vec<Self> {
        let mut options = Vec::new();
        if available_sources.adf_single.is_some() {
            options.push(ScanMode::SingleSidedAdf);
        }
        if available_sources.adf_duplex.is_some() {
            options.push(ScanMode::DuplexAdf);
        }
        if available_sources.adf_single.is_some() {
            options.push(ScanMode::ManualDuplexAdf);
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
/// Scanned files will be stored as TIF files in the scans cache directory.
/// The filename contains a number starting at 1000.
///
/// Parameters:
///   start:
///     The batch offset. If this is set to 0, the filename of the first
///     scanned page will be `1000.tif`. If it's set to 4, the filename
///     of the first scanned page will be `1004.tif`.
///   count:
///     The number of pages to scan. If set to `None`, then all
///     available pages will be scanned.
fn run_scanimage(
    scans_dir: &Path,
    scanner: &Scanner,
    start: usize,
    count: Option<usize>,
    mode: &ScanMode,
    resolution: &Resolution,
) -> Result<()> {
    let mut args = Vec::new();

    debug!("Scanning to {}", scans_dir.display());

    // Generic scanimage parameters
    args.push("--format=tiff".into());
    args.push(format!("--batch={}", scans_dir.join("%d.tif").display()));
    args.push(format!("--batch-start={}", 1000 + start));

    // Common scanner-specific parameters for which we assume support by all scanners
    args.push(format!("--resolution={}", resolution.as_dpi()));
    args.push("-x".into());
    args.push("210".into());
    args.push("-y".into());
    args.push("297".into());

    // Specify scan source
    let source =
        match mode {
            ScanMode::SingleSidedAdf => scanner.sources.adf_single.as_ref().ok_or_else(|| {
                anyhow!("ADF single-sided not available for scanner {}", scanner.id)
            }),
            ScanMode::DuplexAdf => scanner
                .sources
                .adf_duplex
                .as_ref()
                .ok_or_else(|| anyhow!("ADF duplex not available for scanner {}", scanner.id)),
            ScanMode::ManualDuplexAdf => scanner.sources.adf_single.as_ref().ok_or_else(|| {
                anyhow!("ADF manual duplex not available for scanner {}", scanner.id)
            }),
        }?;
    args.push(format!("--source={}", source));

    // Scanner-specific additional arguments
    args.extend_from_slice(&scanner.additional_args);

    trace!("Calling `scanimage` with arguments: {:?}", args);
    let spinner = ProgressBar::new_spinner().with_message("Calling `scanimage` to scan documents…");
    spinner.enable_steady_tick(Duration::from_millis(100));
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

    std::thread::sleep(Duration::from_secs(1));

    fs_utils::copy_dir_file_contents(testdata_dir, scans_dir)?;

    Ok(())
}

/// Select a device from the list of available scanners
fn select_scanner(scanners: &[Scanner]) -> Result<Scanner> {
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

/// Scan a document
fn scan_document(context: &ScanContext) -> Result<()> {
    let scanner = context.scanner;

    // Determine the XDG cache directory, creating it if it doesn't exist
    let scans_dir = app_dirs::app_dir(app_dirs::AppDataType::UserCache, &APP_INFO, "scans")
        .context("Could not determine XDG app cache directory for scans")?;

    // Ensure that "current" scan directory exists and is empty
    let current_dir = scans_dir.join("current");
    fs_utils::ensure_empty_dir_exists(&current_dir)?;

    // Determine scan configuration
    let mode =
        inquire::Select::new("How to scan?", ScanMode::options(&scanner.sources)).prompt()?;
    let option_highdpi = "High resolution (600dpi instead of 300dpi)";
    let options = inquire::MultiSelect::new("Scan options?", vec![option_highdpi]).prompt()?;
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
    if context.fake_scan {
        fake_scanimage(&current_dir).context("Failed to fake `scanimage` command")?;
    } else {
        run_scanimage(&current_dir, scanner, 0, None, &mode, &resolution)
            .context("Failed to run `scanimage` command")?;
    }

    // Rename current scan directory
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let new_dir = scans_dir.join(timestamp);
    fs::rename(&current_dir, &new_dir)?;

    Ok(())
}

struct ScanContext<'a> {
    /// The scanner to use for scanning
    scanner: &'a Scanner,

    /// Whether to fake scanning
    fake_scan: bool,
}

fn main() -> Result<()> {
    // Parse args
    let args = args::Args::try_parse().context("Failed to parse command line arguments")?;

    // Initialize tracing
    initialize_tracing(args.log_level.to_filter())?;

    // Load config
    let config = config::Config::load().context("Failed to load config")?;

    // Select scan device
    let scanner = select_scanner(&config.scanners)?;
    debug!("Selected scanner: {} ({})", scanner.id, scanner.device_name);

    // Create scan context
    let scan_context = ScanContext {
        scanner: &scanner,
        fake_scan: args.fake_scan,
    };

    // Scan a document
    scan_document(&scan_context)?;

    Ok(())
}
