use std::{
    ffi::OsStr,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    process::Stdio,
    sync::OnceLock,
};

use anyhow::{Context, Result, anyhow, ensure};
use indicatif::{ProgressBar, ProgressFinish, ProgressStyle};
use nix::unistd::{Gid, Uid};
use sha2::{Digest, Sha256};
use tracing::{debug, trace, warn};

use crate::{
    common::{self, CheckDependencyResult, filenames},
    config::Config,
    fs_utils,
};

/// Docker image name (without tag)
static DOCKER_IMAGE_NAME: &str = "arkivisto-deps";

/// Bundled Dockerfile for building at runtime
const DOCKERFILE: &[u8] = include_bytes!("../docker/Dockerfile");

/// Commands used to process files
mod commands {
    use crate::common::Dependency;

    pub const DOCKER: Dependency = Dependency {
        bin: "docker",
        name: "Docker",
    };
}

/// Full Docker image name including tag, derived from a hash of the Dockerfile.
///
/// This ensures that whenever the Dockerfile contents change (e.g. in a new
/// release), a new tag is used, so `prepare_dependencies` correctly detects
/// that a rebuild is needed instead of reusing a stale image under the same
/// tag.
fn docker_image() -> &'static str {
    static TAG: OnceLock<String> = OnceLock::new();
    TAG.get_or_init(|| {
        let hash = Sha256::digest(DOCKERFILE);
        let short_hash: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
        format!("{DOCKER_IMAGE_NAME}:{short_hash}")
    })
}

/// Check if a Docker image exists
fn image_exists(name: &str) -> Result<bool> {
    let status = Command::new(commands::DOCKER.bin)
        .arg("image")
        .arg("inspect")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(status.success())
}

pub fn check_dependencies() -> CheckDependencyResult {
    common::check_dependencies(&[commands::DOCKER])
}

/// Prepare dependencies by ensuring the required Docker image is available.
pub fn prepare_dependencies() -> Result<()> {
    let image = docker_image();

    if image_exists(image)? {
        debug!("Docker image {image} already exists, skipping build");
        return Ok(());
    }

    println!("Building Docker image {image}, this may take a moment...");
    let mut child = Command::new(commands::DOCKER.bin)
        .arg("build")
        .arg("-t")
        .arg(image)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    child
        .stdin
        .take()
        .context("Failed to open `docker build` stdin")?
        .write_all(DOCKERFILE)?;

    let output = child.wait_with_output()?;

    if !output.status.success() {
        warn!(
            "docker build failed with status {}. Stderr: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr),
        );
        return Err(anyhow!("Failed to build Docker image {image}"));
    }

    debug!("Successfully built Docker image {image}");
    Ok(())
}

fn run_in_docker(directory: &Path, cmd: &str, args: &[&OsStr]) -> Result<()> {
    // Sanity check that the directory exists and is passed as an absolute
    // path. Both should be true, but let's verify it since Docker would behave
    // badly otherwise (e.g. it would create the directory owned by root).
    ensure!(
        directory.is_absolute() && directory.exists(),
        "Unexpected directory '{}' passed, this indicates an internal logic bug",
        directory.display()
    );
    let output = Command::new(commands::DOCKER.bin)
        .arg("run")
        .arg("--rm")
        // Use current UID/GID to ensure output files are owned by the current user
        .arg("--user")
        .arg(format!("{}:{}", Uid::current(), Gid::current()))
        // Mount directory to the same path, so paths don't need to be translated
        .arg("-v")
        .arg(format!("{0}:{0}", directory.display()))
        // Hardening: Container doesn't need internet access
        .arg("--network")
        .arg("none")
        .arg(docker_image())
        .arg(cmd)
        .args(args)
        .output()?;

    if !output.status.success() {
        warn!(
            "{} failed with status {}. Stderr: {}",
            &cmd,
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr),
        );
        return Err(anyhow!("Failed to run `{}` in docker", cmd));
    }

    Ok(())
}

/// Whether a directory has already been processed.
///
/// A directory is considered processed if it contains a processed PDF or OCR
/// text sidecar file.
pub fn is_processed_document_dir(path: &Path) -> bool {
    path.join(filenames::PROCESSED_PDF).is_file() || path.join(filenames::PROCESSED_TXT).is_file()
}

/// Return iterator over unprocessed document directories.
///
/// Parameters:
///   scans_dir:
///     The parent directory to search for unprocessed document directories.
pub fn find_unprocessed_document_dirs(scans_dir: &std::path::Path) -> Result<Vec<PathBuf>> {
    debug!("Finding unprocessed document directories in {scans_dir:?}");

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
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(fs_utils::is_timestamp_dir_name)
        })
        // Filter out directories that already have processed files
        .filter(|path| !is_processed_document_dir(path))
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
        run_in_docker(
            directory,
            "magick",
            &[
                tif_in.as_ref(),
                "-auto-level".as_ref(),
                "-level".as_ref(),
                "10%,90%".as_ref(),
                tif_out.as_ref(),
            ],
        )?;
        tifs_step1.push(tif_out);
    }
    progress.inc(1);

    // Combine TIFs
    progress.set_message("Combining TIFs");
    let mut args: Vec<&OsStr> = vec!["-c".as_ref(), "lzw".as_ref()];
    for tif_out in &tifs_step1 {
        args.push(tif_out.as_ref());
    }
    let tif_combined = directory.join(filenames::COMBINED_TIF);
    args.push(tif_combined.as_ref());
    run_in_docker(directory, "tiffcp", &args)?;
    progress.inc(1);

    // Convert TIF to PDF
    progress.set_message("Converting to PDF");
    let pdf_out = directory.join(filenames::COMBINED_PDF);
    run_in_docker(
        directory,
        "magick",
        &[
            tif_combined.as_ref(),
            "-compress".as_ref(),
            "JPEG".as_ref(),
            pdf_out.as_ref(),
        ],
    )?;
    progress.inc(1);

    // Run OCR and other postprocessing
    progress.set_message("Running OCR and generating PDF/A");

    trace!("OCRmyPDF config: {:?}", &config.tools.ocrmypdf);
    let processed_txt = directory.join(filenames::PROCESSED_TXT);
    let processed_pdf = directory.join(filenames::PROCESSED_PDF);
    run_in_docker(
        directory,
        "ocrmypdf",
        &[
            "--language".as_ref(),
            config.tools.ocrmypdf.language.as_ref(),
            "--sidecar".as_ref(),
            processed_txt.as_ref(),
            pdf_out.as_ref(),
            processed_pdf.as_ref(),
        ],
    )?;
    progress.inc(1);

    progress.set_message("Processing complete");
    progress.finish();

    Ok(())
}
