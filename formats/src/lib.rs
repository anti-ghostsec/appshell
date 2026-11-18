//! Reading and writing word-processor documents for Leaflet: a Rust port of
//! leaflet_formats.py, made to give the same results (the differential
//! tests in tests/oracle.rs compare the two on the same files).
//!
//! DOCX, ODT, RTF and plain text become one small document model, and that
//! model the HTML the editor shows (and back). Files are untrusted: zip
//! members are size-checked, XML with a DOCTYPE is refused, and the HTML that
//! goes to the editor is built here from the model, never copied.

pub mod docx_in;
pub mod docx_out;
pub mod html_in;
pub mod html_out;
pub mod images;
pub mod model;
pub mod odt_in;
pub mod odt_out;
pub mod rtf;
pub mod text;
pub mod txt;
pub mod xml;
pub mod zipio;

pub use html_in::from_html;
pub use html_out::to_html;
pub use images::{sniff_image, strip_image_metadata};
pub use model::Document;

pub const MAX_MEMBER: usize = 64 * 1024 * 1024;
pub const MAX_TOTAL: usize = 256 * 1024 * 1024;
pub const MAX_IMAGE: usize = 32 * 1024 * 1024;
pub const EMU_PER_PX: i64 = 9525;

/// The file can't be read as the format it claims to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(pub String);

impl FormatError {
    pub fn new(msg: impl Into<String>) -> FormatError {
        FormatError(msg.into())
    }
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FormatError {}

/// What kind of file this is, by its contents: pdf, docx, odt, rtf, ole (old
/// binary .doc), txt, or None.
pub fn detect(data: &[u8]) -> Option<&'static str> {
    let head = &data[..data.len().min(1024)];
    let trimmed = trim_ascii_start(head);
    if trimmed.starts_with(b"%PDF") || head.windows(5).any(|w| w == b"%PDF-") {
        return Some("pdf");
    }
    if data.starts_with(b"PK") {
        let Ok(mut z) = zipio::Zip::parse(data) else { return None };
        if z.has("word/document.xml") {
            return Some("docx");
        }
        if z.has("mimetype") && z.has("content.xml") {
            let mt = z.read("mimetype", false).ok().flatten().unwrap_or_default();
            if trim_ascii(&mt) == b"application/vnd.oasis.opendocument.text" {
                return Some("odt");
            }
        }
        return None;
    }
    if trim_ascii_start(data).starts_with(b"{\\rtf") {
        return Some("rtf");
    }
    if data.starts_with(b"\xd0\xcf\x11\xe0") {
        return Some("ole");
    }
    if data[..data.len().min(8192)].contains(&0) && !data.starts_with(b"\xff\xfe") && !data.starts_with(b"\xfe\xff") {
        return None;
    }
    Some("txt")
}

/// bytes.lstrip()/strip(): ASCII whitespace (space, \t, \n, \r, \x0b, \x0c).
fn is_py_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}
fn trim_ascii_start(d: &[u8]) -> &[u8] {
    let n = d.iter().take_while(|&&b| is_py_space(b)).count();
    &d[n..]
}
fn trim_ascii(d: &[u8]) -> &[u8] {
    let d = trim_ascii_start(d);
    let n = d.iter().rev().take_while(|&&b| is_py_space(b)).count();
    &d[..d.len() - n]
}

pub fn load(data: &[u8], kind: &str) -> Result<Document, FormatError> {
    match kind {
        "docx" => docx_in::read_docx(data),
        "odt" => odt_in::read_odt(data),
        "rtf" => Ok(rtf::read_rtf(data)),
        "txt" => Ok(txt::read_txt(data)),
        _ => Err(FormatError::new("Leaflet can't open this kind of file")),
    }
}

pub fn save(doc: &Document, kind: &str) -> Result<Vec<u8>, FormatError> {
    match kind {
        "docx" => Ok(docx_out::write_docx(doc)),
        "odt" => Ok(odt_out::write_odt(doc)),
        "txt" => Ok(model::plain_text(&doc.blocks).into_bytes()),
        _ => Err(FormatError::new(format!("Leaflet can't save as {kind}"))),
    }
}
