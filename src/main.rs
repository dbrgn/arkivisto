use anyhow::{Context, Result};
use app_dirs::AppInfo;
use clap::Parser;
use tracing::{debug, level_filters::LevelFilter};
use tracing_subscriber::{filter::Targets, prelude::*};

use crate::args::Mode;

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
    let args = args::Args::parse();

    // Initialize tracing
    initialize_tracing(args.log_level.to_filter())?;

    // Load config
    let config = config::Config::load().context("Failed to load config")?;

    // Determine the XDG cache directory, creating it if it doesn't exist
    // TODO: Should this really be in the cache dir? Or is it better to store files in a more permanent location?
    let scans_dir = app_dirs::app_dir(app_dirs::AppDataType::UserCache, &crate::APP_INFO, "scans")
        .context("Could not determine XDG app cache directory for scans")?;

    // Create scan context after selecting a scanner
    let get_scan_context = || -> Result<scan::ScanContext> {
        // Select scan device
        let scanner = scan::select_scanner(&config.scanners)?;
        debug!("Selected scanner: {} ({})", scanner.id, scanner.device_name);

        Ok(scan::ScanContext {
            scanner,
            scans_dir: &scans_dir,
            fake_scan: args.fake_scan,
        })
    };

    // Act depending on mode
    match args.mode {
        Mode::Single => {
            // Scan, process and archive single document
            let scan_context = get_scan_context()?;
            let document_dir = scan::scan_document(&scan_context)?;
            process::process_document(&document_dir, None)
                .context("Failed to post-process document")?;
            // TODO archive
        }
        Mode::Scan => {
            // Scan documents in a loop
            let scan_context = get_scan_context()?;
            loop {
                let document_dir = scan::scan_document(&scan_context)?;
                println!("Scanned document to {}", document_dir.display());
            }
        }
        Mode::Process => {
            // Process any unprocessed documents
            // TODO: Do things in parallel. The `multiprogress` struct has support for this. Maybe use rayon?
            let document_dirs = {
                let mut dirs =
                    process::find_unprocessed_document_dirs(&scans_dir)?.collect::<Vec<_>>();
                dirs.sort();
                dirs
            };
            if document_dirs.is_empty() {
                println!("No unprocessed documents found.");
            } else {
                let multiprogress = indicatif::MultiProgress::new();
                multiprogress
                    .println(format!("Processing {} directories", document_dirs.len()))
                    .ok();
                for document_dir in document_dirs.iter() {
                    process::process_document(document_dir, Some(&multiprogress))
                        .context("Failed to post-process document")?;
                }
            }
        }
        Mode::Archive => {
            todo!("Archiving not yet implemented");
        }
    }

    Ok(())
}
