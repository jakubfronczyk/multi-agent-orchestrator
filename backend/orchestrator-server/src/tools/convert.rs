//! An uploaded file as text the agents can read: a spreadsheet becomes one CSV per sheet, a Word
//! document Markdown, a PDF its text layer. Any other text stays as it is; any other binary file
//! is stored as sent, and the file tools say it cannot be read.

use std::io::{Cursor, Read};

use calamine::{Data, Reader, open_workbook_auto_from_rs};
use quick_xml::events::Event;

/// A zip whose entries unpack to more than this is refused before it is read.
const UNZIPPED_BYTES: u64 = 50 * 1024 * 1024;

/// One file to store: its name and its bytes.
pub struct Output {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("{0} could not be read: {1}")]
    Unreadable(String, String),
    #[error("{0} unpacks to more than 50 MB")]
    TooBig(String),
    #[error("{0} has no text to read; a scanned PDF needs OCR first")]
    NoText(String),
}

/// `name` is the cleaned upload name. A converted file keeps the original stem.
pub fn convert(name: &str, bytes: Vec<u8>) -> Result<Vec<Output>, ConvertError> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) => (stem, extension.to_ascii_lowercase()),
        None => (name, String::new()),
    };
    match extension.as_str() {
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => sheets(name, stem, bytes),
        "docx" => Ok(vec![Output {
            name: format!("{stem}.md"),
            bytes: docx_markdown(name, &bytes)?.into_bytes(),
        }]),
        "pdf" => Ok(vec![Output {
            name: format!("{stem}.txt"),
            bytes: pdf_text(name, &bytes)?.into_bytes(),
        }]),
        _ => Ok(vec![Output {
            name: name.to_string(),
            bytes,
        }]),
    }
}

/// Whether bytes are text the file tools can read.
pub fn is_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| !text.contains('\0'))
}

fn unreadable(name: &str, error: impl std::fmt::Display) -> ConvertError {
    ConvertError::Unreadable(name.to_string(), error.to_string())
}

/// A zip bomb is refused by its declared sizes before anything is unpacked.
fn check_zip(name: &str, bytes: &[u8]) -> Result<(), ConvertError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| unreadable(name, e))?;
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| unreadable(name, e))?;
        total = total.saturating_add(entry.size());
        if total > UNZIPPED_BYTES {
            return Err(ConvertError::TooBig(name.to_string()));
        }
    }
    Ok(())
}

/// One CSV per sheet that has cells; a single sheet keeps the workbook's name.
fn sheets(name: &str, stem: &str, bytes: Vec<u8>) -> Result<Vec<Output>, ConvertError> {
    if !name.to_ascii_lowercase().ends_with(".xls") {
        check_zip(name, &bytes)?;
    }
    let mut workbook =
        open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|e| unreadable(name, e))?;
    let names = workbook.sheet_names();
    let mut outputs = Vec::new();
    for sheet in &names {
        let range = workbook
            .worksheet_range(sheet)
            .map_err(|e| unreadable(name, e))?;
        if range.is_empty() {
            continue;
        }
        let mut writer = csv::Writer::from_writer(Vec::new());
        for row in range.rows() {
            writer
                .write_record(row.iter().map(cell))
                .map_err(|e| unreadable(name, e))?;
        }
        let csv = writer.into_inner().map_err(|e| unreadable(name, e))?;
        let file = if names.len() == 1 {
            format!("{stem}.csv")
        } else {
            format!("{stem}-{}.csv", super::workspace::upload_name(sheet))
        };
        outputs.push(Output {
            name: file,
            bytes: csv,
        });
    }
    if outputs.is_empty() {
        return Err(ConvertError::NoText(name.to_string()));
    }
    Ok(outputs)
}

/// Whole numbers without a trailing `.0`, dates as ISO text, errors as written in Excel.
fn cell(data: &Data) -> String {
    match data {
        Data::Empty => String::new(),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        Data::DateTime(dt) => dt
            .as_datetime()
            .map(|d| {
                if d.time() == chrono::NaiveTime::MIN {
                    d.date().to_string()
                } else {
                    d.to_string()
                }
            })
            .unwrap_or_else(|| dt.to_string()),
        other => other.to_string(),
    }
}

/// Paragraphs of `word/document.xml`, headings as `#`, table cells split by ` | `.
fn docx_markdown(name: &str, bytes: &[u8]) -> Result<String, ConvertError> {
    check_zip(name, bytes)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| unreadable(name, e))?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|e| unreadable(name, e))?
        .read_to_string(&mut xml)
        .map_err(|e| unreadable(name, e))?;

    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut out = String::new();
    let mut paragraph = String::new();
    let mut heading: usize = 0;
    let mut in_text = false;
    loop {
        match reader.read_event().map_err(|e| unreadable(name, e))? {
            Event::Start(tag) | Event::Empty(tag) => match tag.local_name().as_ref() {
                "t" => in_text = true,
                "tab" => paragraph.push('\t'),
                "br" => paragraph.push('\n'),
                "pStyle" => {
                    let style = tag
                        .attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == "val")
                        .map(|a| a.value.to_ascii_lowercase());
                    heading = style
                        .as_deref()
                        .and_then(|s| s.strip_prefix("heading"))
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                }
                _ => {}
            },
            Event::Text(text) if in_text => {
                paragraph.push_str(text.as_ref());
            }
            Event::GeneralRef(entity) if in_text => {
                let resolved = match entity.as_ref() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    _ => "",
                };
                paragraph.push_str(resolved);
            }
            Event::End(tag) => match tag.local_name().as_ref() {
                "t" => in_text = false,
                "tc" => paragraph.push_str(" | "),
                "p" => {
                    let line = paragraph.trim();
                    if !line.is_empty() {
                        if (1..=6).contains(&heading) {
                            out.push_str(&"#".repeat(heading));
                            out.push(' ');
                        }
                        out.push_str(line);
                        out.push_str("\n\n");
                    }
                    paragraph.clear();
                    heading = 0;
                }
                "tr" => {
                    out.push('\n');
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    let text = out.trim().to_string();
    if text.is_empty() {
        return Err(ConvertError::NoText(name.to_string()));
    }
    Ok(text)
}

/// The PDF's text layer. The extractor can panic on a malformed file, so a panic is a refusal.
fn pdf_text(name: &str, bytes: &[u8]) -> Result<String, ConvertError> {
    let text = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes))
        .map_err(|_| unreadable(name, "the PDF is malformed"))?
        .map_err(|e| unreadable(name, e))?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(ConvertError::NoText(name.to_string()));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn docx(document: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .expect("entry");
        zip.write_all(document.as_bytes()).expect("write");
        zip.finish().expect("zip").into_inner()
    }

    #[test]
    fn a_word_document_becomes_markdown() {
        let xml = r#"<w:document xmlns:w="w"><w:body>
            <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Payment policy</w:t></w:r></w:p>
            <w:p><w:r><w:t xml:space="preserve">Over 5k goes to </w:t></w:r><w:r><w:t>Marta &amp; the CFO.</w:t></w:r></w:p>
            <w:tbl><w:tr><w:tc><w:p><w:r><w:t>INV-301</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>1200</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
            </w:body></w:document>"#;
        let out = convert("Policy.docx", docx(xml)).expect("converted");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Policy.md");
        let text = String::from_utf8(out[0].bytes.clone()).expect("utf8");
        assert!(
            text.starts_with("# Payment policy\n\nOver 5k goes to Marta & the CFO."),
            "{text}"
        );
        assert!(text.contains("INV-301") && text.contains("1200"), "{text}");
    }

    #[test]
    fn text_stays_as_it_is_and_binary_is_kept_as_sent() {
        let out = convert("notes.log", b"hello".to_vec()).expect("as is");
        assert_eq!(
            (out[0].name.as_str(), out[0].bytes.as_slice()),
            ("notes.log", &b"hello"[..])
        );
        assert!(is_text(b"a,b\n1,2") && !is_text(&[0x89, b'P', b'N', b'G', 0]));
        let image = convert("photo.png", vec![0x89, b'P', b'N', b'G', 0]).expect("kept");
        assert_eq!(image[0].name, "photo.png");
    }

    /// The smallest PDF with a text layer, offsets computed so the cross-reference table is valid.
    fn pdf(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 24 Tf 72 720 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
        }
        let xref = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    #[test]
    fn a_pdf_becomes_its_text() {
        let out = convert("Invoice.pdf", pdf("Total 17899 EUR")).expect("converted");
        assert_eq!(out[0].name, "Invoice.txt");
        let text = String::from_utf8(out[0].bytes.clone()).expect("utf8");
        assert!(text.contains("Total 17899 EUR"), "{text:?}");
    }

    #[test]
    fn broken_office_files_are_refused_not_panicked_on() {
        for name in ["a.xlsx", "a.docx", "a.pdf", "a.ods"] {
            assert!(convert(name, b"not really".to_vec()).is_err(), "{name}");
        }
    }
}
