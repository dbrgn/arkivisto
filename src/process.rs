use std::{fs, path::Path, process::Command};

use anyhow::{Context, Result, anyhow};
use indicatif::{ProgressBar, ProgressFinish, ProgressStyle};
use tracing::{debug, warn};

/// Process scanned files in a directory.
pub fn process_document(directory: &Path) -> Result<()> {
    debug!("Processing directory {directory:?}");

    // TODO: Check dependencies at setup time

    // Collect all unprocessed TIFF files
    let tifs_step0: Vec<String> = fs::read_dir(directory)
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
    let progress = ProgressBar::new(tifs_step0.len() as u64 + 4)
        .with_message(format!("Processing directory {directory:?}"))
        .with_style(ProgressStyle::with_template("{bar} {msg}").expect("Invalid style"))
        .with_finish(ProgressFinish::AndLeave);

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
        let output = Command::new("magick")
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
    let tif_combined = directory.join("_combined.tif");
    let output = Command::new("tiffcp")
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
    let pdf_out = directory.join("_combined.pdf");
    let output = Command::new("magick")
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
    progress.set_message("Running OCR and generate PDF/A");
    let output = Command::new("docker")
        .arg("run")
        .arg("--rm")
        .arg("-v")
        .arg(format!(
            "{}:/document",
            directory
                .to_str()
                .context("Failed to convert directory path to string")?
        ))
        .arg("docker.io/jbarlow83/ocrmypdf:v16.10.0") // TODO: Extract version
        .arg(
            Path::new("/document/").join(
                pdf_out
                    .file_name()
                    .context("Failed to get output PDF file name")?,
            ),
        )
        .arg(Path::new("/document/_final.pdf"))
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

    progress.finish();

    Ok(())
}
