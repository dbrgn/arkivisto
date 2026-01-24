//! PDF metadata handling.
//!
//! This module handles embedding metadata into PDF documents, including both the
//! legacy Info dictionary and the XMP metadata stream (required for PDF/A compliance).

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use chrono::NaiveDate;
use lopdf::{Document, Object, StringFormat, encode_utf16_be};
use regex::Regex;
use tracing::debug;

/// PDF metadata to embed in the document.
#[derive(Debug, Clone)]
pub struct PdfMetadata {
    pub title: String,
    pub create_date: NaiveDate,
    pub keywords: Vec<String>,
}

/// Set PDF metadata using lopdf and save to a new file.
pub fn set_pdf_metadata(
    input_path: &Path,
    output_path: &Path,
    metadata: &PdfMetadata,
) -> Result<()> {
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
        Object::String(encode_utf16_be(&metadata.title), StringFormat::Literal),
    );
    info_dict.set(
        "Author",
        Object::String("".as_bytes().to_vec(), StringFormat::Literal),
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
        Object::String(encode_utf16_be(&keywords_str), StringFormat::Literal),
    );

    // Update XMP metadata stream if present (required for PDF/A compliance)
    update_xmp_metadata(&mut doc, metadata)?;

    // Set ViewerPreferences to display the document title in the title bar
    let catalog = doc
        .catalog_mut()
        .map_err(|e| anyhow!("Failed to get catalog: {e}"))?;
    let viewer_prefs = if let Ok(Object::Dictionary(dict)) = catalog.get_mut(b"ViewerPreferences") {
        dict
    } else {
        catalog.set(
            "ViewerPreferences",
            lopdf::Dictionary::from_iter(Vec::<(Vec<u8>, Object)>::new()),
        );
        if let Ok(Object::Dictionary(dict)) = catalog.get_mut(b"ViewerPreferences") {
            dict
        } else {
            return Err(anyhow!("Failed to create ViewerPreferences dictionary"));
        }
    };
    viewer_prefs.set("DisplayDocTitle", Object::Boolean(true));

    // Save to output path
    doc.save(output_path)
        .with_context(|| format!("Failed to save PDF: {}", output_path.display()))?;

    Ok(())
}

/// Update XMP metadata stream in the PDF document catalog.
///
/// PDF/A documents contain an XMP metadata stream (XML-based) in addition to the
/// traditional Info dictionary. PDF viewers like Evince prefer the XMP metadata,
/// so both must be kept in sync.
fn update_xmp_metadata(doc: &mut Document, metadata: &PdfMetadata) -> Result<()> {
    // Get the Metadata stream reference from the catalog
    let metadata_id = {
        let catalog = doc
            .catalog()
            .map_err(|e| anyhow!("Failed to get catalog: {e}"))?;
        match catalog.get(b"Metadata") {
            Ok(obj) => match obj.as_reference() {
                Ok(id) => id,
                Err(_) => return Ok(()), // Not a reference, skip
            },
            Err(_) => return Ok(()), // No XMP metadata present, skip
        }
    };

    // Get the stream object and decode its content
    let xmp_content = match doc.get_object(metadata_id)? {
        Object::Stream(stream) => String::from_utf8(stream.content.clone())
            .with_context(|| "XMP metadata stream is not valid UTF-8")?,
        _ => return Ok(()), // Not a stream, skip
    };

    let mut xmp = xmp_content;

    // Update dc:title
    let title_re = Regex::new(
        r"(?s)(<dc:title>\s*<rdf:Alt>\s*<rdf:li[^>]*>)([^<]*)(</rdf:li>\s*</rdf:Alt>\s*</dc:title>)",
    )?;
    xmp = title_re
        .replace(&xmp, |caps: &regex::Captures| {
            format!("{}{}{}", &caps[1], xml_escape(&metadata.title), &caps[3])
        })
        .into_owned();

    // Update xmp:CreateDate
    let create_date_re = Regex::new(r"(<xmp:CreateDate>)([^<]*)(</xmp:CreateDate>)")?;
    let xmp_date = metadata.create_date.format("%Y-%m-%dT00:00:00+00:00");
    xmp = create_date_re
        .replace(&xmp, |caps: &regex::Captures| {
            format!("{}{}{}", &caps[1], xmp_date, &caps[3])
        })
        .into_owned();

    // Update xmp:ModifyDate
    let modify_date_re = Regex::new(r"(<xmp:ModifyDate>)([^<]*)(</xmp:ModifyDate>)")?;
    xmp = modify_date_re
        .replace(&xmp, |caps: &regex::Captures| {
            format!("{}{}{}", &caps[1], xmp_date, &caps[3])
        })
        .into_owned();

    // Update pdf:Keywords
    let keywords_re = Regex::new(r"(<pdf:Keywords>)([^<]*)(</pdf:Keywords>)")?;
    let keywords_str = metadata.keywords.join(", ");
    xmp = keywords_re
        .replace(&xmp, |caps: &regex::Captures| {
            format!("{}{}{}", &caps[1], xml_escape(&keywords_str), &caps[3])
        })
        .into_owned();

    // Write the updated XMP back to the stream object
    let xmp_bytes = xmp.into_bytes();
    match doc.get_object_mut(metadata_id)? {
        Object::Stream(stream) => {
            stream.set_plain_content(xmp_bytes);
        }
        _ => return Err(anyhow!("XMP metadata object changed type unexpectedly")),
    }

    Ok(())
}

/// Escape special XML characters in a string.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    mod xml_escape {
        #[test]
        fn plain_ascii() {
            assert_eq!(super::xml_escape("Hello World"), "Hello World");
        }

        #[test]
        fn special_characters() {
            assert_eq!(super::xml_escape("A & B"), "A &amp; B");
            assert_eq!(super::xml_escape("<tag>"), "&lt;tag&gt;");
            assert_eq!(super::xml_escape("a\"b'c"), "a&quot;b&apos;c");
        }

        #[test]
        fn combined() {
            assert_eq!(
                super::xml_escape("Tom & Jerry's <show>"),
                "Tom &amp; Jerry&apos;s &lt;show&gt;"
            );
        }
    }

    mod update_xmp_metadata {
        use lopdf::{Dictionary, Stream};

        use super::*;

        fn sample_xmp() -> String {
            r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
        xmlns:dc="http://purl.org/dc/elements/1.1/"
        xmlns:pdf="http://ns.adobe.com/pdf/1.3/"
        xmlns:xmp="http://ns.adobe.com/xap/1.0/">
      <dc:title>
        <rdf:Alt>
          <rdf:li xml:lang="x-default">_combined</rdf:li>
        </rdf:Alt>
      </dc:title>
      <dc:creator>
        <rdf:Seq>
          <rdf:li>OCRmyPDF</rdf:li>
        </rdf:Seq>
      </dc:creator>
      <xmp:CreateDate>2024-01-01T12:00:00+00:00</xmp:CreateDate>
      <xmp:ModifyDate>2024-01-01T12:00:00+00:00</xmp:ModifyDate>
      <pdf:Keywords></pdf:Keywords>
      <pdf:Producer>OCRmyPDF 16.13.0 / Tesseract</pdf:Producer>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
                .to_string()
        }

        fn make_pdf_with_xmp(xmp: &str) -> Document {
            let mut doc = Document::new();

            // Create XMP metadata stream
            let mut xmp_dict = Dictionary::new();
            xmp_dict.set("Type", Object::Name(b"Metadata".to_vec()));
            xmp_dict.set("Subtype", Object::Name(b"XML".to_vec()));
            let xmp_stream = Stream::new(xmp_dict, xmp.as_bytes().to_vec());
            let xmp_id = doc.add_object(Object::Stream(xmp_stream));

            // Create a page tree (required for valid catalog)
            let pages_dict = Dictionary::from_iter(vec![
                ("Type", Object::Name(b"Pages".to_vec())),
                ("Kids", Object::Array(vec![])),
                ("Count", Object::Integer(0)),
            ]);
            let pages_id = doc.add_object(pages_dict);

            // Create catalog with metadata reference
            let catalog_dict = Dictionary::from_iter(vec![
                ("Type", Object::Name(b"Catalog".to_vec())),
                ("Pages", Object::Reference(pages_id)),
                ("Metadata", Object::Reference(xmp_id)),
            ]);
            let catalog_id = doc.add_object(catalog_dict);
            doc.trailer.set("Root", Object::Reference(catalog_id));

            doc
        }

        #[test]
        fn updates_title() {
            let mut doc = make_pdf_with_xmp(&sample_xmp());
            let metadata = PdfMetadata {
                title: "My Document".to_string(),
                create_date: NaiveDate::from_ymd_opt(2025, 3, 15).unwrap(),
                keywords: vec!["test".to_string()],
            };

            super::update_xmp_metadata(&mut doc, &metadata).unwrap();

            let catalog = doc.catalog().unwrap();
            let meta_ref = catalog.get(b"Metadata").unwrap().as_reference().unwrap();
            if let Object::Stream(stream) = doc.get_object(meta_ref).unwrap() {
                let content = String::from_utf8(stream.content.clone()).unwrap();
                assert!(content.contains("<rdf:li xml:lang=\"x-default\">My Document</rdf:li>"));
                assert!(!content.contains("_combined"));
            } else {
                panic!("Expected stream object");
            }
        }

        #[test]
        fn updates_dates() {
            let mut doc = make_pdf_with_xmp(&sample_xmp());
            let metadata = PdfMetadata {
                title: "Title".to_string(),
                create_date: NaiveDate::from_ymd_opt(2025, 6, 20).unwrap(),
                keywords: vec![],
            };

            super::update_xmp_metadata(&mut doc, &metadata).unwrap();

            let catalog = doc.catalog().unwrap();
            let meta_ref = catalog.get(b"Metadata").unwrap().as_reference().unwrap();
            if let Object::Stream(stream) = doc.get_object(meta_ref).unwrap() {
                let content = String::from_utf8(stream.content.clone()).unwrap();
                assert!(
                    content.contains("<xmp:CreateDate>2025-06-20T00:00:00+00:00</xmp:CreateDate>")
                );
                assert!(
                    content.contains("<xmp:ModifyDate>2025-06-20T00:00:00+00:00</xmp:ModifyDate>")
                );
            } else {
                panic!("Expected stream object");
            }
        }

        #[test]
        fn updates_keywords() {
            let mut doc = make_pdf_with_xmp(&sample_xmp());
            let metadata = PdfMetadata {
                title: "Title".to_string(),
                create_date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
                keywords: vec!["invoice".to_string(), "tax".to_string()],
            };

            super::update_xmp_metadata(&mut doc, &metadata).unwrap();

            let catalog = doc.catalog().unwrap();
            let meta_ref = catalog.get(b"Metadata").unwrap().as_reference().unwrap();
            if let Object::Stream(stream) = doc.get_object(meta_ref).unwrap() {
                let content = String::from_utf8(stream.content.clone()).unwrap();
                assert!(content.contains("<pdf:Keywords>invoice, tax</pdf:Keywords>"));
            } else {
                panic!("Expected stream object");
            }
        }

        #[test]
        fn escapes_special_characters_in_title() {
            let mut doc = make_pdf_with_xmp(&sample_xmp());
            let metadata = PdfMetadata {
                title: "Tom & Jerry's <doc>".to_string(),
                create_date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
                keywords: vec![],
            };

            super::update_xmp_metadata(&mut doc, &metadata).unwrap();

            let catalog = doc.catalog().unwrap();
            let meta_ref = catalog.get(b"Metadata").unwrap().as_reference().unwrap();
            if let Object::Stream(stream) = doc.get_object(meta_ref).unwrap() {
                let content = String::from_utf8(stream.content.clone()).unwrap();
                assert!(content.contains("Tom &amp; Jerry&apos;s &lt;doc&gt;</rdf:li>"));
            } else {
                panic!("Expected stream object");
            }
        }

        #[test]
        fn no_xmp_stream_is_noop() {
            let mut doc = Document::new();

            // Create catalog without Metadata reference
            let pages_dict = Dictionary::from_iter(vec![
                ("Type", Object::Name(b"Pages".to_vec())),
                ("Kids", Object::Array(vec![])),
                ("Count", Object::Integer(0)),
            ]);
            let pages_id = doc.add_object(pages_dict);
            let catalog_dict = Dictionary::from_iter(vec![
                ("Type", Object::Name(b"Catalog".to_vec())),
                ("Pages", Object::Reference(pages_id)),
            ]);
            let catalog_id = doc.add_object(catalog_dict);
            doc.trailer.set("Root", Object::Reference(catalog_id));

            let metadata = PdfMetadata {
                title: "Title".to_string(),
                create_date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
                keywords: vec![],
            };

            // Should not error when no XMP metadata is present
            super::update_xmp_metadata(&mut doc, &metadata).unwrap();
        }
    }
}
