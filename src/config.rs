use std::{
    collections::HashSet,
    fmt::Display,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Config {
    /// Default output directory for archived files
    pub output_directory: PathBuf,

    /// Tool-specific configuration
    #[serde(default)]
    pub tools: Tools,

    /// Scanner configuration
    pub scanners: Vec<Scanner>,

    /// Author configuration for archiving
    #[serde(default)]
    pub authors: Vec<Author>,
}

/// Configuration for external tools
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
pub struct Tools {
    /// OCRmyPDF configuration
    #[serde(default)]
    pub ocrmypdf: OcrmypdfConfig,
}

/// Configuration for OCRmyPDF
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct OcrmypdfConfig {
    /// Language(s) for OCR (e.g., "eng" or "deu+eng")
    ///
    /// Available languages: eng, chi-sim, deu, fra, osd, por, spa
    #[serde(default = "default_ocrmypdf_language")]
    pub language: String,
}

impl Default for OcrmypdfConfig {
    fn default() -> Self {
        Self {
            language: default_ocrmypdf_language(),
        }
    }
}

fn default_ocrmypdf_language() -> String {
    "eng".to_string()
}

/// An author represents a person or organization that documents can be attributed to.
///
/// Authors are used during archiving to categorize documents and determine the output
/// directory structure.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Author {
    /// Display name of the author
    pub name: String,
    /// Keywords that must ALL be present in OCR text for auto-match (case-insensitive)
    #[serde(default)]
    pub include_keywords: Vec<String>,
    /// Keywords that must NOT be present for auto-match (case-insensitive)
    #[serde(default)]
    pub exclude_keywords: Vec<String>,
    /// Directory name for this author's files (relative to output_directory, or an absolute path)
    pub directory: String,
    /// Keywords to embed in PDF metadata for this author
    #[serde(default)]
    pub pdf_keywords: HashSet<String>,
    /// Document types for this author
    #[serde(default)]
    pub document_types: Vec<DocumentType>,
}

impl Display for Author {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// A document type represents a category of documents within an author.
///
/// Document types allow for finer-grained organization and metadata extraction
/// based on the content of the document.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct DocumentType {
    /// Display name of the document type
    pub name: String,
    /// Keywords that must ALL be present in OCR text for auto-match (case-insensitive)
    #[serde(default)]
    pub include_keywords: Vec<String>,
    /// Keywords that must NOT be present for auto-match (case-insensitive)
    #[serde(default)]
    pub exclude_keywords: Vec<String>,
    /// Directory name for this document type's files (relative to author dir, or an absolute path)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub directory: String,
    /// Regex pattern to extract title from OCR text
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdf_title_regex: Option<String>,
    /// Replacement pattern for title (can use regex capture groups)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdf_title_pattern: Option<String>,
    /// Regex pattern to limit date search to a specific region of OCR text
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdf_date_regex: Option<String>,
    /// Additional keywords to embed in PDF metadata for this document type
    #[serde(default)]
    pub pdf_keywords: HashSet<String>,
}

impl Display for DocumentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Scanner {
    /// Human-readable name of the scanner
    pub name: String,

    /// Name of the scanner as indicated by SANE (e.g. "airscan:e1:HP ScanJet Flow N7000 snw1")
    ///
    /// Use `scanimage -L` to list all available scanners.
    pub device_name: String,

    /// Additional arguments passed to scanimage
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_args: Vec<String>,

    /// ADF single-sided source (if available)
    ///
    /// Use `scanimage --help -d <device> 2>&1 | grep source` to view all available sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_adf_single: Option<String>,

    /// ADF duplex source (if available)
    ///
    /// Use `scanimage --help -d <device> 2>&1 | grep source` to view all available sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_adf_duplex: Option<String>,

    /// Flatbed source (if available)
    ///
    /// Use `scanimage --help -d <device> 2>&1 | grep source` to view all available sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_flatbed: Option<String>,
}

impl Scanner {
    /// Validate that at least one scan source is configured
    fn validate(&self) -> Result<()> {
        if self.source_adf_single.is_none()
            && self.source_adf_duplex.is_none()
            && self.source_flatbed.is_none()
        {
            anyhow::bail!(
                "Scanner '{}' must have at least one scan source configured (source_adf_single, source_adf_duplex, or source_flatbed)",
                self.name
            );
        }
        Ok(())
    }
}

impl Display for Scanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Config {
    /// Get the path to the config file
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = app_dirs::app_root(app_dirs::AppDataType::UserConfig, &super::APP_INFO)
            .context("Could not determine XDG app config directory")?;
        Ok(config_dir.join("config.yml"))
    }

    /// Load config from a specific path
    pub fn load_from_path(config_path: &Path) -> Result<Self> {
        trace!("Config path: {:?}", config_path);

        // Check if file exists
        if !config_path.exists() {
            anyhow::bail!(
                "Config file not found at: {}\n\nTo generate a config file, run:\n\n    arkivisto init-config",
                config_path.display()
            );
        }

        // Read and parse config file
        debug!("Loading config from {:?}", config_path);
        let config_string = fs::read_to_string(config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
        let config: Self =
            serde_saphyr::from_str(&config_string).context("Failed to parse config file")?;

        // Validate scanners
        for scanner in &config.scanners {
            scanner
                .validate()
                .context("Invalid scanner configuration")?;
        }

        Ok(config)
    }

    /// Load config from the default config file location
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;
        Self::load_from_path(&config_path)
            .context(format!("Failed to load config from {:?}", &config_path))
    }

    /// Create backup if file exists
    fn create_backup(config_path: &Path) -> Result<()> {
        if config_path.exists() {
            let backup_path = config_path.with_extension("yml~");
            fs::copy(config_path, &backup_path).with_context(|| {
                format!(
                    "Failed to create backup of config file: {}",
                    config_path.display()
                )
            })?;
            debug!("Created config backup at {:?}", backup_path);
        }
        Ok(())
    }

    /// Save the config to disk
    ///
    /// This method serializes the current in-memory config to YAML and writes it to disk.
    /// A backup of the existing file is created before writing.
    ///
    /// Parameters:
    ///   config_path:
    ///     If set, this config path will be used. Otherwise, the default config path will be used.
    pub fn save(&self, config_path: Option<&Path>) -> Result<()> {
        let config_path = if let Some(path) = config_path {
            path.to_path_buf()
        } else {
            Self::config_path()?
        };

        // Create backup if file exists
        Self::create_backup(&config_path)?;

        // Serialize to YAML
        let yaml = serde_saphyr::to_string(self).context("Failed to serialize config to YAML")?;

        // Write to file
        fs::write(&config_path, yaml)
            .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;

        debug!("Saved config to {:?}", config_path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn parse_minimal_config() {
        let config: Config = serde_saphyr::from_str(
            r#"
output_directory: /tmp/foo

scanners:
  - name: Brother MFC
    device_name: "brother3:net1;dev0"
    source_adf_single: Automatic Document Feeder(centrally aligned)
    source_flatbed: FlatBed
            "#,
        )
        .context("Failed to parse config file")
        .unwrap();

        insta::assert_yaml_snapshot!(config);
    }

    #[test]
    fn load_config() {
        // Write a minimal valid config
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yml");
        let config_content = r#"
output_directory: /tmp/archive

scanners:
  - name: Test Scanner
    device_name: "test:scanner:device"
    source_flatbed: Flatbed
    source_adf_single: ADF
"#;
        fs::write(&config_path, config_content).unwrap();

        // Load the config from the temporary path
        let config = Config::load_from_path(&config_path).unwrap();

        // Verify the config was loaded correctly
        insta::assert_yaml_snapshot!(config);
    }

    mod validation {
        use super::*;

        #[test]
        fn scanner_without_sources_fails_validation() {
            let scanner = Scanner {
                name: "Test Scanner".to_string(),
                device_name: "test:device".to_string(),
                additional_args: vec![],
                source_adf_single: None,
                source_adf_duplex: None,
                source_flatbed: None,
            };

            let result = scanner.validate();
            assert!(result.is_err());
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("at least one scan source")
            );
        }

        #[test]
        fn scanner_with_one_source_passes_validation() {
            let scanner = Scanner {
                name: "Test Scanner".to_string(),
                device_name: "test:device".to_string(),
                additional_args: vec![],
                source_adf_single: Some("ADF".to_string()),
                source_adf_duplex: None,
                source_flatbed: None,
            };

            assert!(scanner.validate().is_ok());
        }

        #[test]
        fn scanner_without_sources_fails_loading() {
            // Write a minimal config without source
            let temp_dir = TempDir::new().unwrap();
            let config_path = temp_dir.path().join("config.yml");
            let config_content = r#"
output_directory: /tmp/archive

scanners:
  - name: Test Scanner
    device_name: "test:scanner:device"
"#;
            fs::write(&config_path, config_content).unwrap();

            // Load the config from the temporary path
            let err = Config::load_from_path(&config_path).unwrap_err();
            insta::assert_debug_snapshot!(err);
        }
    }

    mod save {
        use super::*;

        #[test]
        fn modify_and_save() {
            // Write a minimal valid config
            let temp_dir = TempDir::new().unwrap();
            let config_path = temp_dir.path().join("config.yml");
            let config_content = r#"
output_directory: /tmp/archive

# Scanner configuration
scanners:
  - name: Test Scanner
    device_name: "test:scanner:device"
    source_flatbed: Flatbed
"#;
            fs::write(&config_path, config_content).unwrap();

            // Load config
            let mut config = Config::load_from_path(&config_path).unwrap();

            // Add an author
            let author = Author {
                name: "Musterfirma".into(),
                include_keywords: vec![],
                exclude_keywords: vec![],
                directory: "musterfirma".into(),
                pdf_keywords: HashSet::new(),
                document_types: vec![],
            };
            config.authors.push(author.clone());

            // Add a document type to the author
            let document_type = DocumentType {
                name: "Invoice".into(),
                include_keywords: vec![],
                exclude_keywords: vec![],
                directory: "".into(),
                pdf_title_regex: None,
                pdf_title_pattern: None,
                pdf_date_regex: None,
                pdf_keywords: HashSet::new(),
            };
            config.authors[0].document_types.push(document_type.clone());

            // Save the config
            config.save(Some(&config_path)).unwrap();

            // Verify a backup was created
            let backup_path = config_path.with_extension("yml~");
            assert!(backup_path.exists());

            // Load the config again and verify it was saved correctly
            let loaded_config = Config::load_from_path(&config_path).unwrap();
            assert_eq!(config, loaded_config);

            // Snapshot format used for writing config
            let config_string = fs::read_to_string(config_path).unwrap();
            insta::assert_snapshot!(config_string);
        }
    }
}
