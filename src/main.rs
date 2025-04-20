use std::{fmt::Display, path::Path, process::Command};

use anyhow::{Context, Result, anyhow};
use app_dirs::AppInfo;
use clap::{Parser, ValueEnum};

mod config;

use config::Scanner;

pub const APP_INFO: AppInfo = AppInfo {
    name: "arkivisto",
    author: env!("CARGO_PKG_AUTHORS"),
};

#[derive(Debug, Clone, ValueEnum)]
enum Mode {
    Scan,
    Process,
    Archive,
    Single,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Single
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
#[command(next_line_help = true)]
struct Args {
    /// Processing mode
    #[arg(value_enum, default_value_t = Mode::default())]
    mode: Mode,

    /// Force processing, even if output files already exist
    #[arg(long)]
    force: bool,
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
    fn options() -> Vec<Self> {
        vec![
            ScanMode::SingleSidedAdf,
            ScanMode::DuplexAdf,
            ScanMode::ManualDuplexAdf,
        ]
    }

    fn to_scan_source(&self) -> &'static str {
        match self {
            ScanMode::SingleSidedAdf => "ADF".into(),
            ScanMode::DuplexAdf => "ADF".into(),
            ScanMode::ManualDuplexAdf => "ADF".into(),
        }
    }
}

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

    println!("Scanning to {}", scans_dir.display());

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

    Command::new("scanimage").args(&args).output()?;
    Ok(())
}

/// Select a device from the list of available scanners
fn select_scanner(scanners: &[Scanner]) -> Result<Scanner> {
    // If there is only one device, return it
    if scanners.len() == 1 {
        return Ok(scanners[0].clone());
    }

    // Otherwise, rompt the user to select a scan device
    Ok(inquire::Select::new("Which device do you want to use?", scanners.to_vec()).prompt()?)
}

/// Scan a document
fn scan_document(scanner: &Scanner) -> Result<()> {
    // Determine the XDG cache directory, creating it if it doesn't exist
    let scans_dir = app_dirs::app_dir(app_dirs::AppDataType::UserCache, &APP_INFO, "scans")
        .context("Could not determine XDG app cache directory for scans")?;

    // Determine scan configuration
    let mode = inquire::Select::new("How to scan?", ScanMode::options()).prompt()?;
    let resolution = Resolution::default(); // TODO

    // Run `scanimage` binary
    run_scanimage(&scans_dir, scanner, 0, None, &mode, &resolution)
        .context("Failed to run `scanimage` command")?;

    Ok(())
}

fn main() -> Result<()> {
    // Parse args
    let args = Args::parse();

    // Load config
    let config = config::Config::load().context("Failed to load config")?;

    // Select scan device
    let scanner = select_scanner(&config.scanners)?;

    scan_document(&scanner)?;

    Ok(())
}
