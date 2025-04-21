use anyhow::{Context, Result};
use app_dirs::AppInfo;
use clap::Parser;
use tracing::{debug, level_filters::LevelFilter};
use tracing_subscriber::{filter::Targets, prelude::*};

mod args;
mod config;
mod fs_utils;
mod process;
mod scan;

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

fn main() -> Result<()> {
    // Parse args
    let args = args::Args::try_parse().context("Failed to parse command line arguments")?;

    // Initialize tracing
    initialize_tracing(args.log_level.to_filter())?;

    // Load config
    let config = config::Config::load().context("Failed to load config")?;

    // Select scan device
    let scanner = scan::select_scanner(&config.scanners)?;
    debug!("Selected scanner: {} ({})", scanner.id, scanner.device_name);

    // Create scan context
    let scan_context = scan::ScanContext {
        scanner: &scanner,
        fake_scan: args.fake_scan,
    };

    // TODO: Handle mode

    // Scan a document
    let document_dir = scan::scan_document(&scan_context)?;
    process::process_document(&document_dir).context("Failed to post-process document")?;

    Ok(())
}
