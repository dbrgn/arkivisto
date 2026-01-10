use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow};
use indicatif::{ProgressBar, ProgressFinish, ProgressStyle};
use regex::Regex;
use tracing::{debug, warn};

use crate::common::{self, CheckDependencyResult};

static DATE_TIME_REGEX: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

/// OCRmyPDF Docker image version
static OCRMYPDF_VERSION: &str = "v16.13.0";

/// Special filenames used during document processing
mod filenames {
    /// Intermediate combined TIFF file
    pub const COMBINED_TIF: &str = "_combined.tif";
    /// Intermediate combined PDF file (before OCR)
    pub const COMBINED_PDF: &str = "_combined.pdf";
    /// Final output PDF after OCR processing
    pub const FINAL_PDF: &str = "_final.pdf";
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

/// Return iterator over unprocessed document directories.
///
/// Parameters:
///   scans_dir:
///     The parent directory to search for unprocessed document directories.
pub fn find_unprocessed_document_dirs(
    scans_dir: &std::path::Path,
) -> Result<impl Iterator<Item = PathBuf>> {
    debug!("Finding unprocessed document directories in {scans_dir:?}");

    let date_time_regex = DATE_TIME_REGEX
        .get_or_init(|| Regex::new(r"^\d{8}-\d{6}$").expect("Invalid regex pattern"));

    let entries = fs::read_dir(scans_dir)
        .with_context(|| format!("Failed to read scans directory: {}", scans_dir.display()))?;

    Ok(entries
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
        // Filter out directories that already have a processed file
        .filter(|path| !path.join(filenames::FINAL_PDF).is_file()))
}

/// Process scanned files in a directory.
///
/// Parameters:
///   directory:
///     The directory to process.
///   multiprogress:
///     When defined, this will be used to create progress bars.
pub fn process_document(
    directory: &Path,
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

    // If no TIFF files are found, delete directory and return error
    if tifs_step0.is_empty() {
        warn!("No TIFF files found in directory {directory:?}, removing directory");
        fs::remove_dir_all(directory)
            .context("Failed to remove document directory without TIFF files")?;
        return Err(anyhow!("No TIFF files found in directory"));
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
    // TODO: Download docker image at setup time
    progress.set_message("Running OCR and generating PDF/A");
    let output = Command::new(commands::DOCKER.bin)
        .arg("run")
        .arg("--rm")
        .arg("-v")
        .arg(format!(
            "{}:/document",
            directory
                .to_str()
                .context("Failed to convert directory path to string")?
        ))
        .arg(format!("docker.io/jbarlow83/ocrmypdf:{OCRMYPDF_VERSION}"))
        .arg(
            Path::new("/document/").join(
                pdf_out
                    .file_name()
                    .context("Failed to get output PDF file name")?,
            ),
        )
        .arg(Path::new("/document/").join(filenames::FINAL_PDF))
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
