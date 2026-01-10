use std::{fmt::Display, fs, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// Default output directory for archived files
    pub outdir: PathBuf,
    /// Scanner configuration
    pub scanners: Vec<Scanner>,
    /// Author configuration for archiving
    #[serde(default)]
    pub authors: Vec<Author>,
    /// Tool-specific configuration
    #[serde(default)]
    pub tools: Tools,
}

/// Configuration for external tools
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Tools {
    /// OCRmyPDF configuration
    #[serde(default)]
    pub ocrmypdf: OcrmypdfConfig,
}

/// Configuration for OCRmyPDF
#[derive(Debug, Clone, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Author {
    /// Display name of the author
    pub name: String,
    /// Keywords that must ALL be present in OCR text for auto-match (case-insensitive)
    #[serde(default)]
    pub include_keywords: Vec<String>,
    /// Keywords that must NOT be present for auto-match (case-insensitive)
    #[serde(default)]
    pub exclude_keywords: Vec<String>,
    /// Directory name for this author's files (relative to outdir, or an absolute path)
    pub directory: String,
    /// Keywords to embed in PDF metadata for this author
    #[serde(default)]
    pub pdf_keywords: Vec<String>,
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
#[derive(Debug, Clone, Deserialize, Serialize)]
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
    #[serde(default)]
    pub directory: String,
    /// Regex pattern to extract title from OCR text
    #[serde(default)]
    pub pdf_title_regex: Option<String>,
    /// Replacement pattern for title (can use regex capture groups)
    #[serde(default)]
    pub pdf_title_pattern: Option<String>,
    /// Regex pattern to limit date search to a specific region of OCR text
    #[serde(default)]
    pub pdf_date_regex: Option<String>,
    /// Additional keywords to embed in PDF metadata for this document type
    #[serde(default)]
    pub pdf_keywords: Vec<String>,
}

impl Display for DocumentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Scanner {
    /// Identifier
    pub id: String,

    /// Name of the scanner as indicated by SANE (e.g. "airscan:e1:HP ScanJet Flow N7000 snw1")
    pub device_name: String,

    /// Additional arguments passed to scanimage
    #[serde(default)]
    pub additional_args: Vec<String>,

    /// Configure scan sources
    pub sources: ScannerSources,
}

impl Display for Scanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.id, self.device_name)
    }
}

/// Configure the possible sources of a scanner
///
/// For example, one scanner might call the ADF scan source "ADF", while another
/// might call it "Automatic Document Feeder(centrally aligned)".
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScannerSources {
    /// ADF single-sided source
    pub adf_single: Option<String>,

    /// ADF duplex source
    pub adf_duplex: Option<String>,

    /// Flatbed source
    pub flatbed: Option<String>,
}

impl Config {
    /// Get the path to the config file
    fn config_path() -> Result<PathBuf> {
        let config_dir = app_dirs::app_root(app_dirs::AppDataType::UserConfig, &super::APP_INFO)
            .context("Could not determine XDG app config directory")?;
        Ok(config_dir.join("config.toml"))
    }

    /// Load config from the default config file location
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;
        trace!("Config path: {:?}", config_path);

        // Check if file exists
        if !config_path.exists() {
            anyhow::bail!(
                "Config file does not exist. Please create a config file at: {}",
                config_path.display()
            );
        }

        // Read and parse config file
        debug!("Loading config from {:?}", config_path);
        let config_string = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
        let config: Self = toml::from_str(&config_string).context("Failed to parse config file")?;

        Ok(config)
    }

    /// Save config to the default config file location
    ///
    /// Creates a backup of the existing config file before overwriting.
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;

        // Create backup if file exists
        if config_path.exists() {
            let backup_path = config_path.with_extension("toml~");
            fs::copy(&config_path, &backup_path).with_context(|| {
                format!(
                    "Failed to create backup of config file: {}",
                    config_path.display()
                )
            })?;
            debug!("Created config backup at {:?}", backup_path);
        }

        // Serialize and write config
        let config_string =
            toml::to_string_pretty(self).context("Failed to serialize config to TOML")?;
        fs::write(&config_path, config_string)
            .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;

        debug!("Saved config to {:?}", config_path);
        Ok(())
    }
}
