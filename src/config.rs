use std::{
    fmt::Display,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::DocumentMut;
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
    ///
    /// Use `scanimage -L` to list all available scanners.
    pub device_name: String,

    /// Additional arguments passed to scanimage
    #[serde(default)]
    pub additional_args: Vec<String>,

    /// ADF single-sided source (if available)
    ///
    /// Use `scanimage --help -d <device> 2>&1 | grep source` to view all available sources.
    #[serde(default)]
    pub source_adf_single: Option<String>,

    /// ADF duplex source (if available)
    ///
    /// Use `scanimage --help -d <device> 2>&1 | grep source` to view all available sources.
    #[serde(default)]
    pub source_adf_duplex: Option<String>,

    /// Flatbed source (if available)
    ///
    /// Use `scanimage --help -d <device> 2>&1 | grep source` to view all available sources.
    #[serde(default)]
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
                self.id
            );
        }
        Ok(())
    }
}

impl Display for Scanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.id, self.device_name)
    }
}

/// Helper function to convert a serializable struct to a toml_edit Table
fn struct_to_toml_table<T: Serialize>(value: &T) -> Result<toml_edit::Table> {
    // Serialize the struct to a TOML string
    let toml_string = toml::to_string(value).context("Failed to serialize struct to TOML")?;

    // Parse it back with toml_edit to get a document
    let doc = toml_string
        .parse::<DocumentMut>()
        .context("Failed to parse serialized TOML")?;

    // The serialized struct is the root table
    Ok(doc.as_table().clone())
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

        // Validate scanners
        for scanner in &config.scanners {
            scanner
                .validate()
                .context("Invalid scanner configuration")?;
        }

        Ok(config)
    }

    /// Create backup if file exists
    fn create_backup(config_path: &Path) -> Result<()> {
        if config_path.exists() {
            let backup_path = config_path.with_extension("toml~");
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

    /// Append a new author to the config file, preserving formatting
    ///
    /// This method updates both the in-memory config and the config file on disk.
    pub fn append_author(&mut self, author: Author) -> Result<()> {
        let config_path = Self::config_path()?;

        // Create backup if file exists
        Self::create_backup(&config_path)?;

        // Read and parse config file with toml_edit
        let config_string = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
        let mut doc = config_string
            .parse::<DocumentMut>()
            .context("Failed to parse config file as TOML")?;

        // Convert author to toml_edit table
        let author_table =
            struct_to_toml_table(&author).context("Failed to convert author to TOML")?;

        // Get or create authors array
        if !doc.contains_key("authors") {
            doc["authors"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
        }

        // Append to authors array
        let authors = doc["authors"]
            .as_array_of_tables_mut()
            .context("'authors' must be an array of tables")?;

        authors.push(author_table);

        // Write back to file
        fs::write(&config_path, doc.to_string())
            .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;

        // Update in-memory config
        self.authors.push(author);

        debug!("Appended author to config at {:?}", config_path);
        Ok(())
    }

    /// Append a new document type to an author in the config file, preserving formatting
    ///
    /// This method updates both the in-memory config and the config file on disk.
    pub fn append_document_type(
        &mut self,
        author_name: &str,
        document_type: DocumentType,
    ) -> Result<()> {
        let config_path = Self::config_path()?;

        // Create backup if file exists
        Self::create_backup(&config_path)?;

        // Read and parse config file with toml_edit
        let config_string = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
        let mut doc = config_string
            .parse::<DocumentMut>()
            .context("Failed to parse config file as TOML")?;

        // Convert document type to toml_edit table
        let doc_type_table = struct_to_toml_table(&document_type)
            .context("Failed to convert document type to TOML")?;

        // Find the author in the authors array
        let authors = doc["authors"]
            .as_array_of_tables_mut()
            .context("'authors' must be an array of tables")?;

        let mut found = false;
        for author in authors.iter_mut() {
            if let Some(name) = author.get("name").and_then(|v| v.as_str())
                && name == author_name
            {
                // Get or create document_types array
                if !author.contains_key("document_types") {
                    author["document_types"] =
                        toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
                }

                // Append to document_types array
                let doc_types = author["document_types"]
                    .as_array_of_tables_mut()
                    .context("'document_types' must be an array of tables")?;

                doc_types.push(doc_type_table);

                found = true;
                break;
            }
        }

        if !found {
            anyhow::bail!("Author '{}' not found in config", author_name);
        }

        // Write back to file
        fs::write(&config_path, doc.to_string())
            .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;

        // Update in-memory config
        if let Some(author) = self.authors.iter_mut().find(|a| a.name == author_name) {
            author.document_types.push(document_type);
        }

        debug!(
            "Appended document type to author '{}' in config at {:?}",
            author_name, config_path
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let config: Config = toml::from_str(
            r#"
            outdir = "/tmp/foo"

            [[scanners]]
            id = "brother"
            device_name = "brother3:net1;dev0"
            source_adf_single = "Automatic Document Feeder(centrally aligned)"
            source_flatbed = "FlatBed"
            "#,
        )
        .context("Failed to parse config file")
        .unwrap();
        insta::assert_yaml_snapshot!(config);
    }

    #[test]
    fn scanner_without_sources_fails_validation() {
        let scanner = Scanner {
            id: "test".to_string(),
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
            id: "test".to_string(),
            device_name: "test:device".to_string(),
            additional_args: vec![],
            source_adf_single: Some("ADF".to_string()),
            source_adf_duplex: None,
            source_flatbed: None,
        };

        assert!(scanner.validate().is_ok());
    }

    mod struct_to_toml_table {
        use super::*;

        #[test]
        fn author() {
            let author = Author {
                name: "Test".to_string(),
                include_keywords: vec!["key1".to_string(), "key2".to_string()],
                exclude_keywords: vec![],
                directory: "test".to_string(),
                pdf_keywords: vec![],
                document_types: vec![],
            };

            let table = struct_to_toml_table(&author).unwrap();
            assert!(table.contains_key("name"));
            assert!(table.contains_key("directory"));
            assert!(table.contains_key("include_keywords"));
        }

        #[test]
        fn document_type() {
            let doc_type = DocumentType {
                name: "Invoice".to_string(),
                include_keywords: vec!["invoice".to_string()],
                exclude_keywords: vec![],
                directory: "invoices".to_string(),
                pdf_title_regex: None,
                pdf_title_pattern: None,
                pdf_date_regex: None,
                pdf_keywords: vec![],
            };

            let table = struct_to_toml_table(&doc_type).unwrap();
            assert!(table.contains_key("name"));
            assert!(table.contains_key("directory"));
            assert_eq!(table.get("name").unwrap().as_str().unwrap(), "Invoice");
        }
    }
}
