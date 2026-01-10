use std::process::Command;

/// Special filenames used during document processing
pub mod filenames {
    /// Intermediate combined TIFF file
    pub const COMBINED_TIF: &str = "_combined.tif";
    /// Intermediate combined PDF file (before OCR)
    pub const COMBINED_PDF: &str = "_combined.pdf";
    /// Final output PDF after OCR processing
    pub const PROCESSED_PDF: &str = "_processed.pdf";
    /// OCR text sidecar file
    pub const PROCESSED_TXT: &str = "_processed.txt";
    /// Final output PDF
    pub const FINAL_PDF: &str = "_final.pdf";
    /// Preview PDF filename in the scans directory (during archiving)
    pub const PREVIEW_PDF: &str = "preview.pdf";
}

#[derive(Debug, Hash, PartialEq, Copy, Clone)]
pub struct Dependency {
    /// Binary name
    pub bin: &'static str,
    /// Name of the dependency
    pub name: &'static str,
}

#[derive(Debug, PartialEq)]
pub enum CheckDependencyResult {
    AllAvailable,
    SomeMissing(Vec<Dependency>),
}

impl CheckDependencyResult {
    pub fn merge(&mut self, other: CheckDependencyResult) {
        match (&mut *self, other) {
            (Self::AllAvailable, Self::AllAvailable) => {}
            (Self::AllAvailable, other @ Self::SomeMissing(_)) => *self = other,
            (Self::SomeMissing(_), Self::AllAvailable) => {}
            (Self::SomeMissing(self_missing), Self::SomeMissing(other_missing)) => {
                self_missing.extend(other_missing);
            }
        }
    }
}

/// Check for dependencies, return missing dependencies
pub fn check_dependencies(dependencies: &[Dependency]) -> CheckDependencyResult {
    let mut missing = vec![];
    for dependency in dependencies {
        // Try to spawn the command. `.status()`` returns Err only if the binary cannot be found/executed, not
        // if it exits with a non-zero code.
        let exists = Command::new(dependency.bin)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .status()
            .is_ok();
        if !exists {
            missing.push(*dependency);
        }
    }
    if missing.is_empty() {
        CheckDependencyResult::AllAvailable
    } else {
        CheckDependencyResult::SomeMissing(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::{CheckDependencyResult, Dependency};

    const DEP_A: Dependency = Dependency {
        bin: "a",
        name: "Dep A",
    };
    const DEP_B: Dependency = Dependency {
        bin: "b",
        name: "Dep B",
    };

    mod merge {
        use super::*;

        #[test]
        fn both_all_available() {
            let mut result = CheckDependencyResult::AllAvailable;
            result.merge(CheckDependencyResult::AllAvailable);
            assert_eq!(result, CheckDependencyResult::AllAvailable);
        }

        #[test]
        fn self_all_available_other_some_missing() {
            let mut result = CheckDependencyResult::AllAvailable;
            result.merge(CheckDependencyResult::SomeMissing(vec![DEP_A]));
            assert_eq!(result, CheckDependencyResult::SomeMissing(vec![DEP_A]));
        }

        #[test]
        fn self_some_missing_other_all_available() {
            let mut result = CheckDependencyResult::SomeMissing(vec![DEP_A]);
            result.merge(CheckDependencyResult::AllAvailable);
            assert_eq!(result, CheckDependencyResult::SomeMissing(vec![DEP_A]));
        }

        #[test]
        fn both_some_missing() {
            let mut result = CheckDependencyResult::SomeMissing(vec![DEP_A]);
            result.merge(CheckDependencyResult::SomeMissing(vec![DEP_B]));
            assert_eq!(
                result,
                CheckDependencyResult::SomeMissing(vec![DEP_A, DEP_B])
            );
        }
    }
}
