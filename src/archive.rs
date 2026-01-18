//! Archive module for organizing and storing processed documents.
//!
//! This module handles the final step of the document workflow: assigning metadata
//! to processed PDFs (author, document type, title, date, keywords), embedding
//! that metadata into the PDF, and moving the final file to an organized directory
//! structure.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use chrono::NaiveDate;
use regex::Regex;
use tracing::{debug, warn};

use crate::{
    common::filenames,
    config::{Author, Config, DocumentType},
};

/// German month name prefixes for date parsing
const MONTHS: [&str; 12] = [
    "jan", "feb", "mär", "apr", "mai", "jun", "jul", "aug", "sep", "okt", "nov", "dez",
];

/// OCR text extracted from a scanned document.
///
/// This newtype wraps the raw OCR text and provides methods for extracting
/// structured information like dates, keywords, and metadata patterns.
#[derive(Debug, Clone)]
pub struct OcrText {
    /// The raw OCR text
    text: String,
    /// Lowercased version for case-insensitive matching
    text_lower: String,
}

impl OcrText {
    /// Create a new OcrText from a string
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let text_lower = text.to_lowercase();
        Self { text, text_lower }
    }

    /// Load OcrText from a file
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("Failed to read OCR text file: {}", path.display()))?;
        Ok(Self::new(text))
    }

    /// Find the first date in the text.
    ///
    /// Supports German date formats:
    /// - `4.1.23` or `04.01.2023` (numeric)
    /// - `4. Januar 2023` or `04. Jan. 23` (with month names)
    ///
    /// Returns the parsed date and the matched string.
    pub fn find_date(&self) -> Option<(NaiveDate, String)> {
        // Pattern 1: Numeric dates like "1.1.20", "01. 01. 2000"
        let numeric_pattern =
            Regex::new(r"(?P<d>[0-3]?\d)\.\s?(?P<m>[0-1]?\d)\.\s?(?P<y>\d{2,4})").unwrap();

        // Pattern 2: Named month dates like "1. Jan. 20", "01. Januar 2000"
        let named_pattern = Regex::new(
            r"(?i)(?P<d>[0-3]?\d)\.\s?(?P<m>(?:jan|feb|mär|apr|mai|jun|jul|aug|sep|okt|nov|dez)\S*?)\.?\s?(?P<y>\d{2,4})",
        )
        .unwrap();

        // Collect all matches and sort by position
        let mut matches: Vec<(usize, NaiveDate, String)> = Vec::new();

        for cap in numeric_pattern.captures_iter(&self.text) {
            if let Some(date) = Self::parse_numeric_date(&cap) {
                let matched = cap.get(0).unwrap();
                matches.push((matched.start(), date, matched.as_str().to_string()));
            }
        }

        for cap in named_pattern.captures_iter(&self.text) {
            if let Some(date) = Self::parse_named_date(&cap) {
                let matched = cap.get(0).unwrap();
                matches.push((matched.start(), date, matched.as_str().to_string()));
            }
        }

        // Return the first match by position
        matches.sort_by_key(|(pos, _, _)| *pos);
        matches.into_iter().next().map(|(_, date, s)| (date, s))
    }

    /// Find a date within text matching a specific regex pattern.
    ///
    /// If the regex matches, the matched region is searched for a date.
    /// Falls back to `find_date()` if no regex is provided or if the regex doesn't match.
    pub fn find_date_with_regex(&self, regex: Option<&str>) -> Option<(NaiveDate, String)> {
        if let Some(pattern) = regex {
            match Regex::new(pattern) {
                Ok(re) => {
                    if let Some(m) = re.find(&self.text) {
                        let subset = OcrText::new(m.as_str());
                        if let Some(result) = subset.find_date() {
                            return Some(result);
                        }
                    }
                    warn!("Date regex did not match: {}", pattern);
                }
                Err(e) => {
                    warn!("Invalid date regex '{}': {}", pattern, e);
                }
            }
        }
        self.find_date()
    }

    /// Parse a numeric date from regex captures
    fn parse_numeric_date(cap: &regex::Captures) -> Option<NaiveDate> {
        let day: u32 = cap.name("d")?.as_str().parse().ok()?;
        let month: u32 = cap.name("m")?.as_str().parse().ok()?;
        let mut year: i32 = cap.name("y")?.as_str().parse().ok()?;

        if year < 100 {
            year += 2000;
        }

        NaiveDate::from_ymd_opt(year, month, day)
    }

    /// Parse a named month date from regex captures
    fn parse_named_date(cap: &regex::Captures) -> Option<NaiveDate> {
        let day: u32 = cap.name("d")?.as_str().parse().ok()?;
        let month_str = cap.name("m")?.as_str().to_lowercase();
        let mut year: i32 = cap.name("y")?.as_str().parse().ok()?;

        if year < 100 {
            year += 2000;
        }

        // Find month by prefix
        let month = MONTHS
            .iter()
            .position(|&prefix| month_str.starts_with(prefix))
            .map(|i| i as u32 + 1)?;

        NaiveDate::from_ymd_opt(year, month, day)
    }

    /// Check if the text matches the given keyword rules.
    ///
    /// All `include` keywords must be present (case-insensitive), and
    /// no `exclude` keywords may be present.
    pub fn matches_keywords(&self, include: &[String], exclude: &[String]) -> bool {
        // All include keywords must be present
        for keyword in include {
            if !self.text_lower.contains(&keyword.to_lowercase()) {
                return false;
            }
        }

        // No exclude keywords may be present
        for keyword in exclude {
            if self.text_lower.contains(&keyword.to_lowercase()) {
                return false;
            }
        }

        true
    }

    /// Find all authors that match the OCR text based on keyword rules.
    pub fn matching_authors<'a>(&self, authors: &'a [Author]) -> Vec<&'a Author> {
        authors
            .iter()
            .filter(|author| {
                self.matches_keywords(&author.include_keywords, &author.exclude_keywords)
            })
            .collect()
    }

    /// Find all document types that match the OCR text based on keyword rules.
    pub fn matching_document_types<'a>(
        &self,
        document_types: &'a [DocumentType],
    ) -> Vec<&'a DocumentType> {
        document_types
            .iter()
            .filter(|dt| self.matches_keywords(&dt.include_keywords, &dt.exclude_keywords))
            .collect()
    }

    /// Extract a title using a regex pattern and replacement pattern.
    ///
    /// If both `regex` and `pattern` are provided, the regex is searched in the text
    /// and the pattern is used as a replacement (supporting capture groups).
    pub fn extract_title(&self, regex: Option<&str>, pattern: Option<&str>) -> Option<String> {
        let regex_str = regex?;
        let pattern_str = pattern?;

        let re = match Regex::new(regex_str) {
            Ok(r) => r,
            Err(e) => {
                warn!("Invalid title regex '{}': {}", regex_str, e);
                return None;
            }
        };

        if let Some(cap) = re.captures(&self.text) {
            // Apply the replacement pattern to the full match
            let matched = cap.get(0)?;
            let result = re.replace(matched.as_str(), pattern_str);
            Some(result.into_owned())
        } else {
            warn!("Title regex did not match: {}", regex_str);
            None
        }
    }
}

impl AsRef<str> for OcrText {
    fn as_ref(&self) -> &str {
        &self.text
    }
}

/// Prompt user to select an author from the list of authors.
///
/// Matching authors (based on OCR text) are shown first. The user can also
/// skip the document or create a new author.
///
/// Returns `None` if the user chooses to skip the document.
pub fn select_author(config: &mut Config, ocr_text: &OcrText) -> Result<Option<Author>> {
    let matching = ocr_text.matching_authors(&config.authors);

    // Build options list
    let mut options: Vec<String> = Vec::new();
    options.push("Skip this document".to_string());
    options.push("Create new author".to_string());

    // Add matching authors first
    for author in &matching {
        options.push(format!("{} (matched)", author.name));
    }

    // Add non-matching authors
    for author in &config.authors {
        if !matching.iter().any(|m| m.name == author.name) {
            options.push(author.name.clone());
        }
    }

    // Determine default selection
    let default_index = if !matching.is_empty() { 2 } else { 0 };

    let selection = inquire::Select::new("Select author:", options)
        .with_starting_cursor(default_index)
        .prompt()?;

    if selection == "Skip this document" {
        return Ok(None);
    }

    if selection == "Create new author" {
        let author = create_author(config)?;
        return Ok(Some(author));
    }

    // Find selected author
    let author_name = selection.trim_end_matches(" (matched)");
    let author = config
        .authors
        .iter()
        .find(|a| a.name == author_name)
        .cloned()
        .ok_or_else(|| anyhow!("Author not found: {}", author_name))?;

    Ok(Some(author))
}

/// Prompt user to create a new author and add it to the config.
fn create_author(config: &mut Config) -> Result<Author> {
    let name = inquire::Text::new("Author name:")
        .with_validator(|input: &str| {
            if input.trim().is_empty() {
                Ok(inquire::validator::Validation::Invalid(
                    "Name cannot be empty".into(),
                ))
            } else {
                Ok(inquire::validator::Validation::Valid)
            }
        })
        .prompt()?;

    let default_keywords = name.split_whitespace().collect::<Vec<_>>().join(",");
    let include_keywords_str = inquire::Text::new("Include keywords (comma-separated, AND):")
        .with_default(&default_keywords)
        .prompt()?;
    let include_keywords: Vec<String> = include_keywords_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let exclude_keywords_str =
        inquire::Text::new("Exclude keywords (comma-separated, OR):").prompt()?;
    let exclude_keywords: Vec<String> = exclude_keywords_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let default_dir = name.replace(' ', "_");
    let directory = inquire::Text::new("Output directory name:")
        .with_default(&default_dir)
        .prompt()?;

    let pdf_keywords_str = inquire::Text::new("PDF keywords (comma-separated):")
        .with_default(&default_keywords)
        .prompt()?;
    let pdf_keywords: HashSet<String> = pdf_keywords_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let author = Author {
        name,
        include_keywords,
        exclude_keywords,
        directory,
        pdf_keywords,
        document_types: Vec::new(),
    };

    config.authors.push(author.clone());
    config
        .save(None)
        .context("Failed to save new author to config")?;
    println!("Author successfully added to config file!");

    Ok(author)
}

/// Prompt user to select a document type for the given author.
///
/// Matching document types (based on OCR text) are shown first. The user can also
/// use a one-time document type or create a new one.
pub fn select_document_type(
    config: &mut Config,
    author: &Author,
    ocr_text: &OcrText,
) -> Result<DocumentType> {
    let matching = ocr_text.matching_document_types(&author.document_types);

    // Build options list
    let mut options: Vec<String> = Vec::new();
    options.push("One-time document (no config)".to_string());
    options.push("Create new document type".to_string());

    // Add matching document types first
    for dt in &matching {
        options.push(format!("{} (matched)", dt.name));
    }

    // Add non-matching document types
    for dt in &author.document_types {
        if !matching.iter().any(|m| m.name == dt.name) {
            options.push(dt.name.clone());
        }
    }

    // Determine default selection
    let default_index = if !matching.is_empty() { 2 } else { 0 };

    let selection = inquire::Select::new("Select document type:", options)
        .with_starting_cursor(default_index)
        .prompt()?;

    if selection == "One-time document (no config)" {
        return Ok(DocumentType {
            name: String::new(),
            include_keywords: Vec::new(),
            exclude_keywords: Vec::new(),
            directory: String::new(),
            pdf_title_regex: None,
            pdf_title_pattern: None,
            pdf_date_regex: None,
            pdf_keywords: HashSet::new(),
        });
    }

    if selection == "Create new document type" {
        let document_type = create_document_type(config, author)?;
        return Ok(document_type);
    }

    // Find selected document type
    let dt_name = selection.trim_end_matches(" (matched)");
    let document_type = author
        .document_types
        .iter()
        .find(|dt| dt.name == dt_name)
        .cloned()
        .ok_or_else(|| anyhow!("Document type not found: {}", dt_name))?;

    Ok(document_type)
}

/// Prompt user to create a new document type and add it to the author in config.
fn create_document_type(config: &mut Config, author: &Author) -> Result<DocumentType> {
    let name = inquire::Text::new("Document type name:")
        .with_validator(|input: &str| {
            if input.trim().is_empty() {
                Ok(inquire::validator::Validation::Invalid(
                    "Name cannot be empty".into(),
                ))
            } else {
                Ok(inquire::validator::Validation::Valid)
            }
        })
        .prompt()?;

    let default_keywords = name.split_whitespace().collect::<Vec<_>>().join(",");
    let include_keywords_str = inquire::Text::new("Include keywords (comma-separated, AND):")
        .with_default(&default_keywords)
        .prompt()?;
    let include_keywords: Vec<String> = include_keywords_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let exclude_keywords_str =
        inquire::Text::new("Exclude keywords (comma-separated, OR):").prompt()?;
    let exclude_keywords: Vec<String> = exclude_keywords_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let directory = inquire::Text::new("Output subdirectory name (leave empty for none):")
        .prompt()?
        .trim()
        .to_string();

    let pdf_title_regex = inquire::Text::new("PDF title regex (leave empty for none):")
        .prompt()?
        .trim()
        .to_string();
    let pdf_title_regex = if pdf_title_regex.is_empty() {
        None
    } else {
        Some(pdf_title_regex)
    };

    let pdf_title_pattern = inquire::Text::new("PDF title pattern (leave empty for none):")
        .prompt()?
        .trim()
        .to_string();
    let pdf_title_pattern = if pdf_title_pattern.is_empty() {
        None
    } else {
        Some(pdf_title_pattern)
    };

    let pdf_date_regex = inquire::Text::new("PDF date regex (leave empty for none):")
        .prompt()?
        .trim()
        .to_string();
    let pdf_date_regex = if pdf_date_regex.is_empty() {
        None
    } else {
        Some(pdf_date_regex)
    };

    let pdf_keywords_str = inquire::Text::new("PDF keywords (comma-separated):")
        .with_default(&default_keywords)
        .prompt()?;
    let pdf_keywords: HashSet<String> = pdf_keywords_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let document_type = DocumentType {
        name,
        include_keywords,
        exclude_keywords,
        directory,
        pdf_title_regex,
        pdf_title_pattern,
        pdf_date_regex,
        pdf_keywords,
    };

    // Add to config
    if let Some(author_in_config) = config.authors.iter_mut().find(|a| a.name == author.name) {
        author_in_config.document_types.push(document_type.clone());
    }
    config
        .save(None)
        .context("Failed to save new document type to config")?;
    println!("Document type successfully added to config file!");

    Ok(document_type)
}

/// Prompt user to enter or confirm the document title.
pub fn get_title(document_type: &DocumentType, ocr_text: &OcrText) -> Result<String> {
    // Try to extract title from regex pattern
    let default_title = ocr_text
        .extract_title(
            document_type.pdf_title_regex.as_deref(),
            document_type.pdf_title_pattern.as_deref(),
        )
        .or_else(|| document_type.pdf_title_pattern.clone())
        .unwrap_or_default();

    let title = inquire::Text::new("Document title:")
        .with_default(&default_title)
        .with_validator(|input: &str| {
            if input.trim().is_empty() {
                Ok(inquire::validator::Validation::Invalid(
                    "Title cannot be empty".into(),
                ))
            } else {
                Ok(inquire::validator::Validation::Valid)
            }
        })
        .prompt()?;

    Ok(title)
}

/// Prompt user to enter or confirm the document date.
pub fn get_date(document_type: &DocumentType, ocr_text: &OcrText) -> Result<NaiveDate> {
    // Try to extract date from text
    let (default_date, _) = ocr_text
        .find_date_with_regex(document_type.pdf_date_regex.as_deref())
        .unwrap_or_else(|| {
            warn!("No date found in OCR text, using today's date");
            (chrono::Local::now().date_naive(), String::new())
        });

    let date_format = "%d.%m.%Y";
    let default_date_str = default_date.format(date_format).to_string();

    let date_str = inquire::Text::new("Date:")
        .with_default(&default_date_str)
        .with_validator(|input: &str| {
            let text = OcrText::new(input);
            if text.find_date().is_some() {
                Ok(inquire::validator::Validation::Valid)
            } else {
                Ok(inquire::validator::Validation::Invalid(
                    "Invalid date format. Use DD.MM.YYYY".into(),
                ))
            }
        })
        .prompt()?;

    // Parse the entered date
    let text = OcrText::new(&date_str);
    let (date, _) = text
        .find_date()
        .ok_or_else(|| anyhow!("Invalid date: {}", date_str))?;

    Ok(date)
}

/// Collect all PDF keywords from author and document type.
///
/// If the document type keywords are empty, then ask user to specify keywords.
pub fn get_keywords(author: &Author, document_type: &DocumentType) -> Result<HashSet<String>> {
    let mut keywords = HashSet::new();

    // Add document type keywords
    if document_type.pdf_keywords.is_empty() {
        let keywords_str = inquire::Text::new("PDF keywords (comma-separated):").prompt()?;
        let user_keywords: Vec<String> = keywords_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        keywords.extend(user_keywords);
    } else {
        keywords.extend(document_type.pdf_keywords.clone());
    }

    // Add author keywords
    keywords.extend(author.pdf_keywords.clone());

    Ok(keywords)
}

/// Generate a sanitized filename from title and date.
///
/// Format: `{date}-{title}.pdf`
///
/// Sanitization rules:
/// - Lowercase
/// - Spaces and slashes replaced with hyphens
/// - German umlauts converted: ae, oe, ue
/// - Multiple hyphens collapsed
pub fn generate_filename(title: &str, date: NaiveDate) -> String {
    let date_str = date.format("%Y-%m-%d").to_string();

    let mut filename = format!("{}-{}.pdf", date_str, title);

    // Lowercase
    filename = filename.to_lowercase();

    // Replace German umlauts
    filename = filename
        .replace('ä', "ae")
        .replace('ö', "oe")
        .replace('ü', "ue")
        .replace('ß', "ss");

    // Replace spaces and slashes with hyphens
    filename = filename.replace([' ', '/', '_'], "-");

    // Collapse multiple hyphens
    while filename.contains("--") {
        filename = filename.replace("--", "-");
    }

    filename
}

/// Build the full output path for the archived PDF.
pub fn build_output_path(
    config: &Config,
    author: &Author,
    document_type: &DocumentType,
    filename: &str,
) -> PathBuf {
    let mut path = config.output_directory.clone();
    path.push(&author.directory);
    if !document_type.directory.is_empty() {
        path.push(&document_type.directory);
    }
    path.push(filename);
    path
}

/// PDF metadata to embed in the document
#[derive(Debug, Clone)]
pub struct PdfMetadata {
    pub title: String,
    pub author: String,
    pub creator: String,
    pub create_date: NaiveDate,
    pub keywords: Vec<String>,
}

/// Set PDF metadata using lopdf and save to a new file.
pub fn set_pdf_metadata(
    input_path: &Path,
    output_path: &Path,
    metadata: &PdfMetadata,
) -> Result<()> {
    use lopdf::{Document, Object, StringFormat};

    debug!(
        "Setting PDF metadata: {:?} -> {:?}",
        input_path, output_path
    );

    let mut doc = Document::load(input_path)
        .with_context(|| format!("Failed to load PDF: {}", input_path.display()))?;

    // Create or get Info dictionary
    let info_dict = if let Ok(info_ref) = doc.trailer.get(b"Info") {
        if let Ok(Object::Reference(id)) = info_ref.as_reference().map(Object::Reference) {
            // Get existing info dictionary
            if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(id) {
                dict
            } else {
                return Err(anyhow!("Info object is not a dictionary"));
            }
        } else {
            return Err(anyhow!("Info is not a reference"));
        }
    } else {
        // Create new Info dictionary
        let info_id = doc.add_object(lopdf::Dictionary::new());
        doc.trailer.set("Info", Object::Reference(info_id));
        if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(info_id) {
            dict
        } else {
            return Err(anyhow!("Failed to create Info dictionary"));
        }
    };

    // Set metadata fields
    info_dict.set(
        "Title",
        Object::String(metadata.title.as_bytes().to_vec(), StringFormat::Literal),
    );
    info_dict.set(
        "Author",
        Object::String(metadata.author.as_bytes().to_vec(), StringFormat::Literal),
    );
    info_dict.set(
        "Creator",
        Object::String(metadata.creator.as_bytes().to_vec(), StringFormat::Literal),
    );

    // Set creation date in PDF format: D:YYYYMMDDHHmmSS
    let date_str = format!("D:{}000000", metadata.create_date.format("%Y%m%d"));
    info_dict.set(
        "CreationDate",
        Object::String(date_str.as_bytes().to_vec(), StringFormat::Literal),
    );

    // Set keywords
    let keywords_str = metadata.keywords.join(", ");
    info_dict.set(
        "Keywords",
        Object::String(keywords_str.as_bytes().to_vec(), StringFormat::Literal),
    );

    // Save to output path
    doc.save(output_path)
        .with_context(|| format!("Failed to save PDF: {}", output_path.display()))?;

    Ok(())
}

/// Create a preview copy of the PDF in the scans directory.
pub fn create_preview(pdf_path: &Path, scans_dir: &Path) -> Result<PathBuf> {
    let preview_path = scans_dir.join(filenames::PREVIEW_PDF);
    fs::copy(pdf_path, &preview_path)
        .with_context(|| format!("Failed to create preview at {}", preview_path.display()))?;
    Ok(preview_path)
}

/// Find all document directories that are ready for archiving.
///
/// A directory is ready for archiving if it contains both `_processed.pdf` and `_processed.txt`.
pub fn find_archivable_document_dirs(scans_dir: &Path) -> Result<Vec<PathBuf>> {
    let date_time_regex = Regex::new(r"^\d{8}-\d{6}$").expect("Invalid regex pattern");

    debug!("Searching {} for archivable documents", scans_dir.display());
    let entries = fs::read_dir(scans_dir)
        .with_context(|| format!("Failed to read scans directory: {}", scans_dir.display()))?;

    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| date_time_regex.is_match(name))
        })
        .filter(|path| {
            // Must have both _final.pdf and _final.txt
            path.join(filenames::PROCESSED_PDF).is_file()
                && path.join(filenames::PROCESSED_TXT).is_file()
        })
        .collect();

    dirs.sort();
    Ok(dirs)
}

/// Archive a single processed document.
///
/// This function handles the complete archive workflow:
///
/// 1. Read OCR text from sidecar file
/// 2. Create preview PDF
/// 3. Select author and document type
/// 4. Extract/confirm title and date
/// 5. Collect keywords
/// 6. Generate filename and output path
/// 7. Embed metadata and save to final location
/// 8. Clean up processed directory
pub fn archive_document(config: &mut Config, document_dir: &Path, scans_dir: &Path) -> Result<()> {
    // Security: Verify document_dir is actually within scans_dir to prevent path traversal
    {
        let document_dir_canonical = document_dir.canonicalize().with_context(|| {
            format!(
                "Failed to canonicalize document directory: {}",
                document_dir.display()
            )
        })?;
        let scans_dir_canonical = scans_dir.canonicalize().with_context(|| {
            format!(
                "Failed to canonicalize scans directory: {}",
                scans_dir.display()
            )
        })?;
        if !document_dir_canonical.starts_with(&scans_dir_canonical) {
            return Err(anyhow!(
                "Security check failed: document directory '{}' is not within scans directory '{}'",
                document_dir.display(),
                scans_dir.display()
            ));
        }
    }

    // Prepare paths
    let pdf_path = document_dir.join(filenames::PROCESSED_PDF);
    let txt_path = document_dir.join(filenames::PROCESSED_TXT);
    if !pdf_path.exists() {
        debug!(
            "Skipping {:?}: no {} found",
            document_dir,
            filenames::PROCESSED_PDF
        );
        return Ok(());
    }
    if !txt_path.exists() {
        warn!(
            "Skipping {:?}: no {} found (OCR text required for archiving)",
            document_dir,
            filenames::PROCESSED_TXT
        );
        return Ok(());
    }

    println!("\nArchiving {}...", document_dir.display());

    // Load OCR text
    let ocr_text = OcrText::from_file(&txt_path)?;

    // Create preview
    let preview_path = create_preview(&pdf_path, scans_dir)?;
    println!("Preview available at: {}", preview_path.display());

    // Select author
    let author = match select_author(config, &ocr_text)? {
        Some(a) => a,
        None => {
            println!("=== SKIPPED ===\n");
            return Ok(());
        }
    };

    // Select document type
    let document_type = select_document_type(config, &author, &ocr_text)?;

    // Get title
    let title = get_title(&document_type, &ocr_text)?;

    // Get date
    let date = get_date(&document_type, &ocr_text)?;

    // Get keywords
    let keywords = get_keywords(&author, &document_type)?;

    // Generate filename and path
    let filename = generate_filename(&title, date);
    let output_path = build_output_path(config, &author, &document_type, &filename);

    // Prepare metadata
    let metadata = PdfMetadata {
        title,
        author: author.name.clone(),
        creator: author.name.clone(),
        create_date: date,
        keywords: Vec::from_iter(keywords),
    };

    // Create output PDF with metadata
    let final_output = document_dir.join(filenames::FINAL_PDF);
    set_pdf_metadata(&pdf_path, &final_output, &metadata)?;

    // Update preview with final metadata
    fs::copy(&final_output, &preview_path)?;
    println!(
        "Preview updated with metadata at: {}",
        preview_path.display()
    );

    // Confirm save
    println!("Output path: {}", output_path.display());
    let confirm = inquire::Confirm::new(&format!("Save to {}?", output_path.display()))
        .with_default(true)
        .prompt()?;

    if !confirm {
        println!("=== SKIPPED ===\n");
        return Ok(());
    }

    // Check if file already exists
    if output_path.exists() {
        let overwrite = inquire::Confirm::new("File already exists. Overwrite?")
            .with_default(false)
            .prompt()?;
        if !overwrite {
            println!("=== SKIPPED ===\n");
            return Ok(());
        }
    }

    // Create output directory if needed
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create output directory: {}", parent.display()))?;
    }

    // Copy to output path
    fs::copy(&final_output, &output_path)
        .with_context(|| format!("Failed to copy PDF to {}", output_path.display()))?;

    // Remove processed directory
    fs::remove_dir_all(document_dir).with_context(|| {
        format!(
            "Failed to remove processed directory: {}",
            document_dir.display()
        )
    })?;

    println!("=== SUCCESS ===\n");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    mod ocr_text {
        use super::*;

        mod find_date {
            use super::*;

            #[test]
            fn numeric_very_short() {
                let text = OcrText::new("4.1.23");
                let result = text.find_date();
                assert_eq!(
                    result,
                    Some((
                        NaiveDate::from_ymd_opt(2023, 1, 4).unwrap(),
                        "4.1.23".to_string()
                    ))
                );
            }

            #[test]
            fn numeric_short() {
                let text = OcrText::new("Foo\nBar 4. 5. 22 Foo\nBar");
                let result = text.find_date();
                assert_eq!(
                    result,
                    Some((
                        NaiveDate::from_ymd_opt(2022, 5, 4).unwrap(),
                        "4. 5. 22".to_string()
                    ))
                );
            }

            #[test]
            fn numeric_short_no_spaces() {
                let text = OcrText::new("Foo\nBar 4.5.22 Foo\nBar");
                let result = text.find_date();
                assert_eq!(
                    result,
                    Some((
                        NaiveDate::from_ymd_opt(2022, 5, 4).unwrap(),
                        "4.5.22".to_string()
                    ))
                );
            }

            #[test]
            fn numeric_with_leading_zeros() {
                let text = OcrText::new("Foo\nBar 04. 05. 2022 Foo\nBar");
                let result = text.find_date();
                assert_eq!(
                    result,
                    Some((
                        NaiveDate::from_ymd_opt(2022, 5, 4).unwrap(),
                        "04. 05. 2022".to_string()
                    ))
                );
            }

            #[test]
            fn named_month_long() {
                let text = OcrText::new("Foo\nBar 4. Oktober 2022 Foo\nBar");
                let result = text.find_date();
                assert_eq!(
                    result,
                    Some((
                        NaiveDate::from_ymd_opt(2022, 10, 4).unwrap(),
                        "4. Oktober 2022".to_string()
                    ))
                );
            }

            #[test]
            fn multiple_dates_returns_first() {
                let text = OcrText::new("4. Mai 2022 6. 7. 23");
                let result = text.find_date();
                assert_eq!(
                    result,
                    Some((
                        NaiveDate::from_ymd_opt(2022, 5, 4).unwrap(),
                        "4. Mai 2022".to_string()
                    ))
                );
            }

            #[test]
            fn invalid_date_returns_none() {
                let text = OcrText::new("Foo\nBar 4. 5. Foo\nBar");
                let result = text.find_date();
                assert_eq!(result, None);
            }
        }

        mod matches_keywords {
            use super::*;

            #[test]
            fn all_include_present() {
                let text = OcrText::new("Hello World Test");
                assert!(text.matches_keywords(&["hello".to_string(), "world".to_string()], &[]));
            }

            #[test]
            fn include_missing() {
                let text = OcrText::new("Hello World Test");
                assert!(!text.matches_keywords(&["hello".to_string(), "missing".to_string()], &[]));
            }

            #[test]
            fn exclude_present() {
                let text = OcrText::new("Hello World Test");
                assert!(!text.matches_keywords(&["hello".to_string()], &["test".to_string()]));
            }

            #[test]
            fn case_insensitive() {
                let text = OcrText::new("HELLO world TeSt");
                assert!(text.matches_keywords(&["Hello".to_string(), "WORLD".to_string()], &[]));
            }
        }
    }

    mod generate_filename {
        use super::*;

        #[test]
        fn basic() {
            let date = NaiveDate::from_ymd_opt(2023, 5, 15).unwrap();
            let result = generate_filename("Test Document", date);
            assert_eq!(result, "2023-05-15-test-document.pdf");
        }

        #[test]
        fn german_umlauts() {
            let date = NaiveDate::from_ymd_opt(2023, 5, 15).unwrap();
            let result = generate_filename("Ärztliche Überweisung für Öffentlichkeit", date);
            assert_eq!(
                result,
                "2023-05-15-aerztliche-ueberweisung-fuer-oeffentlichkeit.pdf"
            );
        }

        #[test]
        fn slashes_and_underscores() {
            let date = NaiveDate::from_ymd_opt(2023, 5, 15).unwrap();
            let result = generate_filename("Test/Document_Name", date);
            assert_eq!(result, "2023-05-15-test-document-name.pdf");
        }

        #[test]
        fn multiple_spaces() {
            let date = NaiveDate::from_ymd_opt(2023, 5, 15).unwrap();
            let result = generate_filename("Test   Multiple   Spaces", date);
            assert_eq!(result, "2023-05-15-test-multiple-spaces.pdf");
        }
    }
}
