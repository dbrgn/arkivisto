use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow};
use indicatif::{ProgressBar, ProgressFinish, ProgressStyle};
use nix::unistd::{Gid, Uid};
use regex::Regex;
use tracing::{debug, trace, warn};

use crate::{
    common::{self, CheckDependencyResult, filenames},
    config::Config,
};

static DATE_TIME_REGEX: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

/// OCRmyPDF Docker image and version
static OCRMYPDF_IMAGE: &str = "docker.io/jbarlow83/ocrmypdf:v16.13.0";

/// Get the current user's UID and GID for use with Docker --user flag.
///
/// This ensures that files created by Docker containers are owned by the current user
/// rather than root.
fn get_current_uid_gid() -> (u32, u32) {
    (Uid::current().as_raw(), Gid::current().as_raw())
}

/// Commands used to process files
mod commands {
    use crate::common::Dependency;

    pub const MAGICK: Dependency = Dependency {
        bin: "magick",
        name: "Imagemagick",
    };
    pub const TIFFCP: Dependency = Dependency {
        bin: "tiffcp",
        name: "libtiff",
    };
    pub const DOCKER: Dependency = Dependency {
        bin: "docker",
        name: "Docker",
    };
}

pub fn check_dependencies() -> CheckDependencyResult {
    common::check_dependencies(&[commands::MAGICK, commands::TIFFCP, commands::DOCKER])
}

/// Prepare dependencies by ensuring the required Docker image is available.
///
/// This function checks if the OCRmyPDF Docker image exists locally and pulls
/// it if necessary.
pub fn prepare_dependencies() -> Result<()> {
    // Check if Docker image exists locally
    trace!("Checking for Docker image {OCRMYPDF_IMAGE}");
    let output = Command::new(commands::DOCKER.bin)
        .arg("image")
        .arg("inspect")
        .arg(OCRMYPDF_IMAGE)
        .output()
        .context("Failed to run `docker image inspect` command")?;
    if output.status.success() {
        debug!("Docker image {OCRMYPDF_IMAGE} already exists locally");
        return Ok(());
    }

    // Image doesn't exist, pull it
    println!("Fetching Docker image {OCRMYPDF_IMAGE}");
    let output = Command::new(commands::DOCKER.bin)
        .arg("pull")
        .arg(OCRMYPDF_IMAGE)
        .output()
        .context("Failed to run `docker pull` command")?;

    if !output.status.success() {
        warn!(
            "docker pull failed with status {}. Stderr: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr),
        );
        return Err(anyhow!("Failed to pull Docker image {OCRMYPDF_IMAGE}"));
    }

    debug!("Successfully pulled Docker image {OCRMYPDF_IMAGE}");
    Ok(())
}

/// Return iterator over unprocessed document directories.
///
/// Parameters:
///   scans_dir:
///     The parent directory to search for unprocessed document directories.
pub fn find_unprocessed_document_dirs(scans_dir: &std::path::Path) -> Result<Vec<PathBuf>> {
    debug!("Finding unprocessed document directories in {scans_dir:?}");

    let date_time_regex = DATE_TIME_REGEX
        .get_or_init(|| Regex::new(r"^\d{8}-\d{6}$").expect("Invalid regex pattern"));

    let entries = fs::read_dir(scans_dir)
        .with_context(|| format!("Failed to read scans directory: {}", scans_dir.display()))?;

    let mut dirs: Vec<_> = entries
        // Filter out any IO errors and unwrap successful entries
        .filter_map(|entry| entry.ok())
        // Convert file system entries to paths
        .map(|entry| entry.path())
        // Keep only directories
        .filter(|path| path.is_dir())
        // Keep only directories with names matching the date-time format
        .filter(move |path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| date_time_regex.is_match(name))
        })
        // Filter out directories that already have processed files
        .filter(|path| {
            !path.join(filenames::PROCESSED_PDF).is_file()
                && !path.join(filenames::PROCESSED_TXT).is_file()
        })
        .collect();

    dirs.sort();
    Ok(dirs)
}

/// Process scanned files in a directory.
///
/// Parameters:
///   directory:
///     The directory to process.
///   config:
///     The application configuration.
///   multiprogress:
///     When defined, this will be used to create progress bars.
pub fn process_document(
    directory: &Path,
    config: &Config,
    multiprogress: Option<&indicatif::MultiProgress>,
) -> Result<()> {
    debug!("Processing directory {directory:?}");

    // Collect all unprocessed TIFF files
    let mut tifs_step0: Vec<String> = fs::read_dir(directory)
        .expect("Failed to read directory")
        .filter_map(|entry| {
            let entry = entry.expect("Failed to read directory entry");
            let filename = entry.file_name().into_string().unwrap();
            if filename.ends_with(".tif") && !filename.contains('_') {
                Some(filename)
            } else {
                None
            }
        })
        .collect();
    tifs_step0.sort();

    // If no TIFF files are found, ask user if they want to delete the directory
    if tifs_step0.is_empty() {
        warn!("No TIFF files found in directory {directory:?}");

        // Ask for confirmation before deleting
        let should_delete = inquire::Confirm::new(&format!(
            "No TIFF files found in {}. Delete directory?",
            directory.display()
        ))
        .with_default(true)
        .prompt()
        .unwrap_or(false); // If prompt fails (e.g., non-interactive), don't delete

        if should_delete {
            fs::remove_dir_all(directory)
                .context("Failed to remove document directory without TIFF files")?;
            return Err(anyhow!("No TIFF files found in directory (deleted)"));
        } else {
            return Err(anyhow!("No TIFF files found in directory (kept)"));
        }
    }

    // Initialize progress bar
    //
    // Calculation of steps:
    // - Initial step: 1 step
    // - Postprocessing of pages: n steps
    // - Combining TIFs: 1 step
    // - Converting to PDF: 1 step
    // - OCRmyPDF: 1 step
    let mut progress = ProgressBar::new(tifs_step0.len() as u64 + 4)
        .with_message(format!("Processing directory {directory:?}"))
        .with_prefix(
            directory
                .file_name()
                .map(|os_str| os_str.to_string_lossy().into_owned())
                .unwrap_or_else(|| "?".to_string()),
        )
        .with_style(ProgressStyle::with_template("{prefix} {bar} {msg}").expect("Invalid style"))
        .with_finish(ProgressFinish::AndLeave);
    if let Some(multiprogress) = multiprogress {
        progress = multiprogress.add(progress);
    }

    // Postprocess with ImageMagick:
    //
    // - Improve contrast
    let mut tifs_step1 = Vec::new();
    // TODO: Parallel processing
    for (i, tif) in tifs_step0.iter().enumerate() {
        progress.set_message(format!(
            "Improving contrast ({}/{})",
            i + 1,
            tifs_step0.len()
        ));
        progress.inc(1);

        let tif_in = directory.join(tif);
        let tif_out = directory.join(tif.replace(".tif", "_processed.tif"));

        // TODO: Tweak parameters
        // TODO: Compress with LZW or something else?
        let output = Command::new(commands::MAGICK.bin)
            .arg(tif_in.as_os_str())
            .arg("-auto-level")
            .arg("-level")
            .arg("10%,90%")
            .arg(tif_out.as_os_str())
            .output()?;
        if !output.status.success() {
            warn!(
                "magick failed with status {}. Stderr: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr),
            );
            return Err(anyhow!("Failed to run `magick` command"));
        }
        tifs_step1.push(tif_out);
    }
    progress.inc(1);

    // Combine TIFs
    progress.set_message("Combining TIFs");
    let tif_combined = directory.join(filenames::COMBINED_TIF);
    let output = Command::new(commands::TIFFCP.bin)
        .arg("-c")
        .arg("lzw")
        .args(&tifs_step1)
        .arg(tif_combined.as_os_str())
        .output()?;
    if !output.status.success() {
        warn!(
            "tiffcp failed with status {}. Stderr: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr),
        );
        return Err(anyhow!("Failed to run `tiffcp` command"));
    }
    progress.inc(1);

    // Convert TIF to PDF
    progress.set_message("Converting to PDF");
    let pdf_out = directory.join(filenames::COMBINED_PDF);
    let output = Command::new(commands::MAGICK.bin)
        .arg(tif_combined.as_os_str())
        .arg("-compress")
        .arg("JPEG")
        .arg(pdf_out.as_os_str())
        .output()?;
    if !output.status.success() {
        warn!(
            "magick failed with status {}. Stderr: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr),
        );
        return Err(anyhow!("Failed to run `magick` command"));
    }
    progress.inc(1);

    // Run OCR and other postprocessing
    progress.set_message("Running OCR and generating PDF/A");

    // Get current UID/GID to ensure output files are owned by the current user
    let (uid, gid) = get_current_uid_gid();

    trace!("OCRmyPDF config: {:?}", &config.tools.ocrmypdf);
    let output = Command::new(commands::DOCKER.bin)
        .arg("run")
        .arg("--rm")
        .arg("--user")
        .arg(format!("{}:{}", uid, gid))
        .arg("-v")
        .arg(format!(
            "{}:/document",
            directory
                .to_str()
                .context("Failed to convert directory path to string")?
        ))
        .arg(OCRMYPDF_IMAGE)
        .arg("--language")
        .arg(&config.tools.ocrmypdf.language)
        .arg("--sidecar")
        .arg(Path::new("/document/").join(filenames::PROCESSED_TXT))
        .arg(
            Path::new("/document/").join(
                pdf_out
                    .file_name()
                    .context("Failed to get output PDF file name")?,
            ),
        )
        .arg(Path::new("/document/").join(filenames::PROCESSED_PDF))
        .output()?;
    if !output.status.success() {
        warn!(
            "ocrmypdf failed with status {}. Stderr: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr),
        );
        return Err(anyhow!("Failed to run `ocrmypdf` command (through Docker)"));
    }
    progress.inc(1);

    progress.set_message("Processing complete");
    progress.finish();

    Ok(())
}
