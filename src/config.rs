use std::{fmt::Display, path::PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Output directory for scanned files
    pub outdir: PathBuf,
    /// Scanner configuration
    pub scanners: Vec<Scanner>,
}

#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
pub struct ScannerSources {
    /// ADF single-sided source
    pub adf_single: Option<String>,

    /// ADF duplex source
    pub adf_duplex: Option<String>,

    /// Flatbed source
    pub flatbed: Option<String>,
}

impl Config {
    pub fn load() -> Result<Self> {
        // Determine the XDG app config directory, creating it if it doesn't exist
        let config_dir = app_dirs::app_root(app_dirs::AppDataType::UserConfig, &super::APP_INFO)
            .context("Could not determine XDG app config directory")?;

        // Check if file exists
        let config_path = config_dir.join("config.toml");
        if !config_path.exists() {
            anyhow::bail!(
                "Config file does not exist. Please create a config file at: {}",
                config_path.display()
            );
        }

        // Read and parse config file
        let config_string = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
        let config: Self = toml::from_str(&config_string).context("Failed to parse config file")?;

        Ok(config)
    }
}
