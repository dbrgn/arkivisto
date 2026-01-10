use anyhow::{Context, Result};
use app_dirs::AppInfo;
use clap::Parser;
use tracing::{debug, level_filters::LevelFilter};
use tracing_subscriber::{filter::Targets, prelude::*};

use crate::{args::Mode, common::CheckDependencyResult};

mod archive;
mod args;
mod common;
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

    // Load config (mutable for archive mode to add new authors/document types)
    let mut config = config::Config::load().context("Failed to load config")?;

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

    // Check dependencies on external commands
    let mut check_dependency_result = CheckDependencyResult::AllAvailable;
    if matches!(args.mode, Mode::Single | Mode::Process) {
        check_dependency_result.merge(process::check_dependencies());
    }
    if matches!(args.mode, Mode::Single | Mode::Scan) {
        check_dependency_result.merge(scan::check_dependencies());
    }
    if let CheckDependencyResult::SomeMissing(missing) = check_dependency_result {
        eprintln!("Error: Missing system dependencies:");
        for dep in &missing {
            eprintln!("  - {} (part of {})", dep.bin, dep.name);
        }
        std::process::exit(1);
    }

    // Prepare dependencies
    if matches!(args.mode, Mode::Single | Mode::Process) {
        process::prepare_dependencies()?;
    }

    // Act depending on mode
    match args.mode {
        Mode::Single => {
            // Scan, process and archive single document
            let scan_context = get_scan_context()?;
            let document_dir = scan::scan_document(&scan_context)?;
            process::process_document(&document_dir, None)
                .context("Failed to post-process document")?;
            archive::archive_document(&mut config, &document_dir, &scans_dir)
                .context("Failed to archive document")?;
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
            // Archive any processed documents
            let document_dirs = archive::find_archivable_document_dirs(&scans_dir)?;
            if document_dirs.is_empty() {
                println!("No documents ready for archiving.");
            } else {
                for document_dir in document_dirs {
                    archive::archive_document(&mut config, &document_dir, &scans_dir)
                        .context("Failed to archive document")?;
                }
            }
        }
    }

    Ok(())
}
