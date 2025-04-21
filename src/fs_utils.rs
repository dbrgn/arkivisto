use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};

/// Ensure that a directory exists and is empty
///
/// If the directory exists, it will be removed and recreated.
///
/// Note that the parent directory must exist.
pub fn ensure_empty_dir_exists(path: &Path) -> Result<()> {
    if path.exists() {
        ensure!(
            path.is_dir(),
            "Target path {:?} exists and is not a directory",
            path
        );
        fs::remove_dir_all(path).context("Failed to remove existing directory")?;
    }
    fs::create_dir(path).context("Failed to create directory")?;
    Ok(())
}

/// Copy the contents of a directory non-recursively to another directory.
///
/// Only files are copied, not directories. The destination directory must exist
/// and be a directory.
pub fn copy_dir_file_contents(src: &Path, dst: &Path) -> Result<()> {
    ensure!(
        dst.exists() && dst.is_dir(),
        "Destination path {:?} does not exist or is not a directory",
        dst
    );
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_file() {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    mod ensure_empty_dir_exists {
        use super::*;

        use std::fs::File;

        use tempfile::TempDir;

        /// Ensure that the function correctly returns an error when the parent
        /// directory does not exist.
        #[test]
        fn parent_dir_does_not_exist() {
            let temp_dir = TempDir::new().unwrap();
            let nonexistent_parent = temp_dir.path().join("nonexistent");
            let target_dir = nonexistent_parent.join("target");

            let result = ensure_empty_dir_exists(&target_dir);
            assert!(result.is_err());
        }

        /// Ensure that the function can create a new directory when it doesn't
        /// already exist.
        #[test]
        fn test_create_new_dir() {
            let temp_dir = TempDir::new().unwrap();
            let target_dir = temp_dir.path().join("target");

            assert!(!target_dir.exists());
            ensure_empty_dir_exists(&target_dir).expect("Test failed");
            assert!(target_dir.exists());
            assert!(target_dir.is_dir());
        }

        /// Ensure that the function properly handles an already existing empty
        /// directory.
        #[test]
        fn test_empty_dir_exists() {
            let temp_dir = TempDir::new().unwrap();
            let target_dir = temp_dir.path().join("target");

            fs::create_dir(&target_dir).unwrap();

            assert!(target_dir.exists());
            ensure_empty_dir_exists(&target_dir).expect("Test failed");
            assert!(target_dir.exists());
            assert!(target_dir.is_dir());
        }

        /// Ensure that the function correctly removes all contents when the
        /// target directory exists and contains files.
        #[test]
        fn test_dir_with_files_exists() {
            let temp_dir = TempDir::new().unwrap();
            let target_dir = temp_dir.path().join("target");

            // Create the directory and a file inside it
            fs::create_dir(&target_dir).unwrap();
            let file_path = target_dir.join("test_file.txt");
            File::create(&file_path).unwrap();

            // Verify file exists
            assert!(file_path.exists());

            ensure_empty_dir_exists(&target_dir).expect("Test failed");
            assert!(target_dir.exists());
            assert!(target_dir.is_dir());
            assert!(!file_path.exists()); // File should be gone
        }

        /// Ensure that the function returns an error in the case where the
        /// target path exists but is a file instead of a directory.
        #[test]
        fn test_path_is_a_file() {
            let temp_dir = TempDir::new().unwrap();
            let target_path = temp_dir.path().join("target");

            // Create a regular file at the target path
            File::create(&target_path).unwrap();
            assert!(target_path.exists());
            assert!(!target_path.is_dir());

            let result = ensure_empty_dir_exists(&target_path);
            assert!(result.is_err());
            assert!(target_path.exists());
            assert!(!target_path.is_dir());
        }
    }
}
