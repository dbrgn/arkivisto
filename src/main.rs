use anyhow::{Context, Result};
use app_dirs::AppInfo;
use clap::Parser;
use tracing::{debug, level_filters::LevelFilter, trace};
use tracing_subscriber::{filter::Targets, prelude::*};

use crate::{args::Mode, common::CheckDependencyResult};

mod archive;
mod args;
mod common;
mod config;
mod fs_utils;
mod init_config;
mod metadata;
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

    // Check dependencies on external commands
    let mut check_dependency_result = CheckDependencyResult::AllAvailable;
    if matches!(args.mode, Mode::Single | Mode::Process) {
        check_dependency_result.merge(process::check_dependencies());
    }
    if matches!(args.mode, Mode::Single | Mode::Scan | Mode::InitConfig) {
        check_dependency_result.merge(scan::check_dependencies());
    }
    if let CheckDependencyResult::SomeMissing(missing) = check_dependency_result {
        eprintln!("Error: Missing system dependencies:");
        for dep in &missing {
            eprintln!("  - {} (part of {})", dep.bin, dep.name);
        }
        std::process::exit(1);
    }

    // Handle init-config mode before loading config (config may not exist yet)
    if matches!(args.mode, Mode::InitConfig) {
        init_config::run_init_config()?;
        return Ok(());
    }

    // Load config (mutable for archive mode to add new authors/document types)
    let mut config = {
        let path = config::Config::config_path()?;
        if !path.exists() {
            eprintln!(
                "Config file not found at: {}\n\nTo generate a config file, run:\n\n    arkivisto init-config",
                path.display()
            );
            std::process::exit(1);
        }
        config::Config::load()?
    };

    // Determine the XDG data directory, creating it if it doesn't exist
    let scans_dir = app_dirs::app_dir(app_dirs::AppDataType::UserData, &crate::APP_INFO, "scans")
        .context("Could not determine XDG app data directory for scans")?;
    trace!("Scans path: {:?}", scans_dir);

    // Create scan context after selecting a scanner
    let get_scan_context = || -> Result<scan::ScanContext> {
        // Select scan device
        let scanner = scan::select_scanner(&config.scanners)?;
        debug!(
            "Selected scanner: {} ({})",
            scanner.name, scanner.device_name
        );

        Ok(scan::ScanContext {
            scanner,
            scans_dir: &scans_dir,
            fake_scan: args.fake_scan,
        })
    };

    // Prepare dependencies
    if matches!(args.mode, Mode::Single | Mode::Process) {
        process::prepare_dependencies()?;
    }

    // Act depending on mode
    match args.mode {
        Mode::Single => {
            // Scan, process and archive single document
            let scan_context = get_scan_context()?;
            let (document_dir, _) = scan::scan_document(&scan_context, None)?;
            process::process_document(&document_dir, &config, None)
                .context("Failed to post-process document")?;
            archive::archive_document(&mut config, &document_dir, &scans_dir, true)
                .context("Failed to archive document")?;
        }
        Mode::Scan => {
            // Scan documents in a loop
            let scan_context = get_scan_context()?;
            let mut last_scan_mode_index = None;
            loop {
                let (document_dir, selected_index) =
                    scan::scan_document(&scan_context, last_scan_mode_index)?;
                last_scan_mode_index = Some(selected_index);
                println!("Scanned document to {}", document_dir.display());
            }
        }
        Mode::Process => {
            // Process any unprocessed documents
            // TODO: Do things in parallel. The `multiprogress` struct has support for this. Maybe use rayon?
            let document_dirs = process::find_unprocessed_document_dirs(&scans_dir)?;
            if document_dirs.is_empty() {
                println!("No unprocessed documents found.");
            } else {
                let multiprogress = indicatif::MultiProgress::new();
                multiprogress
                    .println(format!("Processing {} directories", document_dirs.len()))
                    .ok();
                for document_dir in document_dirs.iter() {
                    process::process_document(document_dir, &config, Some(&multiprogress))
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
                println!(
                    "Found {} documents ready for archiving.",
                    document_dirs.len()
                );
                for (i, document_dir) in document_dirs.iter().enumerate() {
                    let offer_preview_open = i == 0;
                    archive::archive_document(
                        &mut config,
                        document_dir,
                        &scans_dir,
                        offer_preview_open,
                    )
                    .context("Failed to archive document")?;
                }
            }
        }
        Mode::InitConfig => {
            // This is unreachable because InitConfig is handled earlier before config loading
            unreachable!("InitConfig mode should have been handled before config loading");
        }
    }

    Ok(())
}
