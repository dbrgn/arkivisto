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
    process::{Command, Stdio},
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::NaiveDate;
use inquire::error::InquireError;
use regex::Regex;
use tracing::{debug, trace, warn};

use crate::{
    common::filenames,
    config::{Author, Config, DocumentType},
    metadata::{PdfMetadata, set_pdf_metadata},
};

/// German month name prefixes for date parsing
const MONTHS: [&str; 12] = [
    "jan", "feb", "mär", "apr", "mai", "jun", "jul", "aug", "sep", "okt", "nov", "dez",
];

/// Resolution strategy when the target file already exists.
enum ConflictResolution {
    /// Rename the file by appending a number (e.g., .2, .3, etc.)
    RenameTo(PathBuf),
    /// Overwrite the existing file
    Overwrite,
    /// Discard the scan and delete the scan directory
    Discard,
    /// Skip archiving and preserve the scan directory for later
    Skip,
}

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

        tracing::trace!(?include, ?exclude, "OcrText mext matches",);
        true
    }

    /// Find all authors that match the OCR text based on keyword rules.
    pub fn matching_authors<'a>(&self, authors: &'a [Author]) -> Vec<&'a Author> {
        authors
            .iter()
            .filter(|author| !author.include_keywords.is_empty()) // Include keywords must be set for a match
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
            .filter(|dt| !dt.include_keywords.is_empty()) // Include keywords must be set for a match
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
            trace!(?cap, "Extracting title: Found capture");

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

const AUTHOR_OPTION_SEPARATOR: &str = "───";
const AUTHOR_OPTION_SKIP: &str = "Skip this document";
const AUTHOR_OPTION_CREATE: &str = "Create new author";

/// Build author selection options and determine default selection index.
///
/// Returns a tuple of (options, default_index) where:
/// - options is a list of display strings for the selection menu
/// - default_index is the suggested cursor position (0-based)
fn build_author_options(config: &Config, ocr_text: &OcrText) -> Result<(Vec<String>, usize)> {
    let mut matching = ocr_text.matching_authors(&config.authors);

    // Data validation
    if config
        .authors
        .iter()
        .any(|author| author.name == AUTHOR_OPTION_SEPARATOR)
    {
        bail!(
            "Author config cannot contain entry with name matching the separator {:?}",
            AUTHOR_OPTION_SEPARATOR
        );
    }

    // Build options list
    let mut options: Vec<String> = Vec::new();
    options.push(AUTHOR_OPTION_SKIP.to_string());
    options.push(AUTHOR_OPTION_CREATE.to_string());

    // Add matching authors first (sorted by name)
    matching.sort_by_key(|author| author.name.to_lowercase());
    if !matching.is_empty() {
        options.push(AUTHOR_OPTION_SEPARATOR.to_string());
        for author in &matching {
            options.push(format!("{} (matched)", author.name));
        }
    }

    // Add non-matching authors (sorted by name)
    let mut non_matching_authors = config
        .authors
        .iter()
        .filter(|author| !matching.contains(author))
        .map(|author| author.name.clone())
        .collect::<Vec<_>>();
    non_matching_authors.sort_by_key(|name| name.to_lowercase());
    if !non_matching_authors.is_empty() {
        options.push(AUTHOR_OPTION_SEPARATOR.to_string());
        options.extend_from_slice(&non_matching_authors);
    }

    // Determine default selection
    let default_index = if !matching.is_empty() { 3 } else { 0 };

    Ok((options, default_index))
}

/// Prompt user to select an author from the list of authors.
///
/// Matching authors (based on OCR text) are shown first. The user can also
/// skip the document or create a new author.
///
/// Returns `None` if the user chooses to skip the document.
pub fn select_author(config: &mut Config, ocr_text: &OcrText) -> Result<Option<Author>> {
    let (options, default_index) = build_author_options(config, ocr_text)?;

    let selection = inquire::Select::new("Select author:", options)
        .with_starting_cursor(default_index)
        .with_page_size(12)
        .prompt()?;

    if selection == AUTHOR_OPTION_SEPARATOR {
        // Currently we cannot prevent this
        return select_author(config, ocr_text);
    }

    if selection == AUTHOR_OPTION_SKIP {
        return Ok(None);
    }
    if selection == AUTHOR_OPTION_CREATE {
        match create_author(config) {
            Ok(author) => return Ok(Some(author)),
            Err(e) if is_cancel_error(&e) => return select_author(config, ocr_text),
            Err(e) => return Err(e),
        }
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

/// Check if an error is an `OperationCanceled` error from inquire.
fn is_cancel_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<InquireError>()
        .map(|e| matches!(e, InquireError::OperationCanceled))
        .unwrap_or(false)
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
    let directory = inquire::Text::new("Output directory name (or path):")
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
        match create_document_type(config, author) {
            Ok(document_type) => return Ok(document_type),
            Err(e) if is_cancel_error(&e) => return select_document_type(config, author, ocr_text),
            Err(e) => return Err(e),
        }
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

    let directory = inquire::Text::new("Output subdirectory name or path (leave empty for none):")
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
        .or_else(|| document_type.pdf_title_pattern.clone());

    let mut prompt = inquire::Text::new("Document title:").with_validator(|input: &str| {
        if input.trim().is_empty() {
            Ok(inquire::validator::Validation::Invalid(
                "Title cannot be empty".into(),
            ))
        } else {
            Ok(inquire::validator::Validation::Valid)
        }
    });

    // Only set default if we have a non-empty value
    if let Some(title) = default_title.as_ref().filter(|s| !s.is_empty()) {
        prompt = prompt.with_default(title);
    }

    let title = prompt.prompt()?;

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

/// Given a path that already exists, returns a new path with a number appended.
///
/// For example: `/foo/myfile.pdf` -> `/foo/myfile.2.pdf`
///
/// If the numbered path also exists, increments the number until a free slot is found.
fn next_available_path(path: &Path) -> PathBuf {
    let parent = path.parent();
    let extension = path.extension();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

    let mut counter = 2;
    loop {
        let new_name = match extension {
            Some(ext) => format!("{}.{}.{}", stem, counter, ext.to_string_lossy()),
            None => format!("{}.{}", stem, counter),
        };

        let new_path = if let Some(p) = parent {
            p.join(new_name)
        } else {
            PathBuf::from(new_name)
        };

        if !new_path.exists() {
            return new_path;
        }

        counter += 1;
    }
}

/// Prompt the user for a conflict resolution strategy when the target file already exists.
fn resolve_conflict(output_path: &Path) -> Result<ConflictResolution> {
    let original_filename = output_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("Could not determine original filename"))?;
    let renamed_path = next_available_path(output_path);
    let renamed_filename = renamed_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("Could not determine renamed filename"))?;

    let option_rename = format!("Save as \"{}\"", renamed_filename);
    let option_overwrite = format!("Overwrite \"{}\"", original_filename);
    let option_discard = "Discard scan".to_string();
    let option_skip = "Skip scan (preserve for later)".to_string();

    let selection = inquire::Select::new(
        "File already exists. What do you want to do?",
        vec![
            option_rename.clone(),
            option_overwrite.clone(),
            option_discard.clone(),
            option_skip.clone(),
        ],
    )
    .prompt()?;

    match selection {
        s if s == option_rename => Ok(ConflictResolution::RenameTo(renamed_path)),
        s if s == option_overwrite => Ok(ConflictResolution::Overwrite),
        s if s == option_discard => Ok(ConflictResolution::Discard),
        s if s == option_skip => Ok(ConflictResolution::Skip),
        other => bail!(format!("Invalid selection: {:?}", other)),
    }
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

/// Create a preview copy of the PDF in the scans directory.
pub fn create_preview(pdf_path: &Path, scans_dir: &Path) -> Result<PathBuf> {
    let preview_path = scans_dir.join(filenames::PREVIEW_PDF);
    fs::copy(pdf_path, &preview_path)
        .with_context(|| format!("Failed to create preview at {}", preview_path.display()))?;
    Ok(preview_path)
}

/// Whether a directory is ready for archiving.
///
/// A directory is ready for archiving if its name matches the timestamp format
/// and it contains both `_processed.pdf` and `_processed.txt`.
pub fn is_archivable_document_dir(path: &Path) -> bool {
    path.is_dir()
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(crate::fs_utils::is_timestamp_dir_name)
        && path.join(filenames::PROCESSED_PDF).is_file()
        && path.join(filenames::PROCESSED_TXT).is_file()
}

/// Find all document directories that are ready for archiving.
///
/// A directory is ready for archiving if it contains both `_processed.pdf` and `_processed.txt`.
pub fn find_archivable_document_dirs(scans_dir: &Path) -> Result<Vec<PathBuf>> {
    debug!("Searching {} for archivable documents", scans_dir.display());
    let entries = fs::read_dir(scans_dir)
        .with_context(|| format!("Failed to read scans directory: {}", scans_dir.display()))?;

    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_archivable_document_dir(path))
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
pub fn archive_document(
    config: &mut Config,
    document_dir: &Path,
    scans_dir: &Path,
    offer_preview_open: bool,
) -> Result<()> {
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

    // Open preview
    if offer_preview_open {
        let open_preview = inquire::Confirm::new(&format!(
            "Open preview with '{}'?",
            &config.tools.pdf_viewer,
        ))
        .with_default(true)
        .prompt()?;
        if open_preview {
            let spawned = Command::new(&config.tools.pdf_viewer)
                .arg(&preview_path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if spawned.is_err() {
                eprintln!("Failed to spawn PDF viewer: {:?}", spawned);
            }
        }
    }

    // Select author and document type in a loop to allow going back
    loop {
        let author = match select_author(config, &ocr_text)? {
            Some(a) => a,
            None => {
                println!("Document skipped");
                return Ok(());
            }
        };

        let document_type = match select_document_type(config, &author, &ocr_text) {
            Ok(dt) => dt,
            Err(e) if is_cancel_error(&e) => {
                continue;
            }
            Err(e) => return Err(e),
        };

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
            title: title.clone(),
            create_date: date,
            keywords: Vec::from_iter(keywords),
        };

        // Create output PDF with metadata
        let final_output = document_dir.join(filenames::FINAL_PDF);
        set_pdf_metadata(&pdf_path, &final_output, &metadata)?;

        // Update preview with final metadata
        fs::copy(&final_output, &preview_path)?;
        debug!(
            "Preview updated with metadata at: {}",
            preview_path.display()
        );

        // Confirm save
        trace!("Output path: {}", output_path.display());
        let confirm = inquire::Confirm::new(&format!("Save to {}?", output_path.display()))
            .with_default(true)
            .prompt()?;

        if !confirm {
            println!("Document \"{title}\" skipped");
            return Ok(());
        }

        // Check if file already exists and resolve conflicts
        let output_path = if output_path.exists() {
            match resolve_conflict(&output_path)? {
                ConflictResolution::RenameTo(new_path) => {
                    println!("Saving to {}", new_path.display());
                    new_path
                }
                ConflictResolution::Overwrite => {
                    println!("Overwriting {}", output_path.display());
                    output_path
                }
                ConflictResolution::Discard => {
                    fs::remove_dir_all(document_dir).with_context(|| {
                        format!(
                            "Failed to remove processed directory: {}",
                            document_dir.display()
                        )
                    })?;
                    println!("Document \"{title}\" discarded");
                    return Ok(());
                }
                ConflictResolution::Skip => {
                    println!("Document \"{title}\" skipped");
                    return Ok(());
                }
            }
        } else {
            output_path
        };

        // Create output directory if needed
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create output directory: {}", parent.display())
            })?;
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

        println!("Document \"{title}\" archived successfully");

        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, fs::File};

    use tempfile::TempDir;

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

    mod build_author_options {
        use super::*;

        fn make_author(name: &str, include_keywords: Vec<&str>) -> Author {
            Author {
                name: name.to_string(),
                include_keywords: include_keywords.iter().map(|s| s.to_string()).collect(),
                exclude_keywords: vec![],
                directory: name.replace(' ', "_"),
                pdf_keywords: HashSet::new(),
                document_types: vec![],
            }
        }

        fn make_config(authors: Vec<Author>) -> Config {
            Config {
                output_directory: "/tmp/output".into(),
                tools: crate::config::Tools::default(),
                scanners: vec![],
                authors,
            }
        }

        #[test]
        fn no_authors_defaults_to_skip() {
            let config = make_config(vec![]);
            let ocr_text = OcrText::new("Some text");

            let (options, default_index) = build_author_options(&config, &ocr_text).unwrap();

            assert_eq!(options, vec![AUTHOR_OPTION_SKIP, AUTHOR_OPTION_CREATE]);
            assert_eq!(default_index, 0);
        }

        #[test]
        fn no_matching_authors_defaults_to_skip() {
            let config = make_config(vec![
                make_author("Alice", vec!["alice"]),
                make_author("Bob", vec!["bob"]),
            ]);
            let ocr_text = OcrText::new("Some unrelated text");

            let (options, default_index) = build_author_options(&config, &ocr_text).unwrap();

            assert_eq!(
                options,
                vec![
                    AUTHOR_OPTION_SKIP,
                    AUTHOR_OPTION_CREATE,
                    AUTHOR_OPTION_SEPARATOR,
                    "Alice",
                    "Bob"
                ]
            );
            assert_eq!(default_index, 0);
        }

        #[test]
        fn single_matching_author_defaults_to_match() {
            let config = make_config(vec![
                make_author("Alice", vec!["alice"]),
                make_author("Bob", vec!["bob"]),
            ]);
            let ocr_text = OcrText::new("This is from alice");

            let (options, default_index) = build_author_options(&config, &ocr_text).unwrap();

            assert_eq!(
                options,
                vec![
                    AUTHOR_OPTION_SKIP,
                    AUTHOR_OPTION_CREATE,
                    AUTHOR_OPTION_SEPARATOR,
                    "Alice (matched)",
                    AUTHOR_OPTION_SEPARATOR,
                    "Bob"
                ]
            );
            assert_eq!(default_index, 3);
        }

        #[test]
        fn multiple_matching_authors_sorted_alphabetically() {
            let config = make_config(vec![
                make_author("Zara", vec!["company"]),
                make_author("Alice", vec!["company"]),
                make_author("Bob", vec!["bob"]),
            ]);
            let ocr_text = OcrText::new("Letter from company");

            let (options, default_index) = build_author_options(&config, &ocr_text).unwrap();

            assert_eq!(
                options,
                vec![
                    AUTHOR_OPTION_SKIP,
                    AUTHOR_OPTION_CREATE,
                    AUTHOR_OPTION_SEPARATOR,
                    "Alice (matched)",
                    "Zara (matched)",
                    AUTHOR_OPTION_SEPARATOR,
                    "Bob"
                ]
            );
            assert_eq!(default_index, 3);
        }

        #[test]
        fn non_matching_authors_sorted_alphabetically() {
            let config = make_config(vec![
                make_author("Zara", vec!["zara"]),
                make_author("Alice", vec!["company"]),
                make_author("Bob", vec!["bob"]),
                make_author("Charlie", vec!["charlie"]),
            ]);
            let ocr_text = OcrText::new("Letter from company");

            let (options, default_index) = build_author_options(&config, &ocr_text).unwrap();

            assert_eq!(
                options,
                vec![
                    AUTHOR_OPTION_SKIP,
                    AUTHOR_OPTION_CREATE,
                    AUTHOR_OPTION_SEPARATOR,
                    "Alice (matched)",
                    AUTHOR_OPTION_SEPARATOR,
                    "Bob",
                    "Charlie",
                    "Zara"
                ]
            );
            assert_eq!(default_index, 3);
        }

        #[test]
        fn case_insensitive_sorting() {
            let config = make_config(vec![
                make_author("zara", vec!["zara"]),
                make_author("Alice", vec!["alice"]),
                make_author("bob", vec!["bob"]),
            ]);
            let ocr_text = OcrText::new("Some text");

            let (options, default_index) = build_author_options(&config, &ocr_text).unwrap();

            assert_eq!(
                options,
                vec![
                    AUTHOR_OPTION_SKIP,
                    AUTHOR_OPTION_CREATE,
                    AUTHOR_OPTION_SEPARATOR,
                    "Alice",
                    "bob",
                    "zara"
                ]
            );
            assert_eq!(default_index, 0);
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

    mod next_available_path {
        use super::*;

        #[test]
        fn basic_rename() {
            let temp_dir = TempDir::new().unwrap();
            let existing_file = temp_dir.path().join("myfile.pdf");
            File::create(&existing_file).unwrap();

            let result = next_available_path(&existing_file);

            assert_eq!(result, temp_dir.path().join("myfile.2.pdf"));
        }

        #[test]
        fn increments_past_existing() {
            let temp_dir = TempDir::new().unwrap();
            let existing_file = temp_dir.path().join("myfile.pdf");
            File::create(&existing_file).unwrap();
            File::create(temp_dir.path().join("myfile.2.pdf")).unwrap();
            File::create(temp_dir.path().join("myfile.3.pdf")).unwrap();

            let result = next_available_path(&existing_file);

            assert_eq!(result, temp_dir.path().join("myfile.4.pdf"));
        }

        #[test]
        fn no_extension() {
            let temp_dir = TempDir::new().unwrap();
            let existing_file = temp_dir.path().join("myfile");
            File::create(&existing_file).unwrap();

            let result = next_available_path(&existing_file);

            assert_eq!(result, temp_dir.path().join("myfile.2"));
        }
    }
}
