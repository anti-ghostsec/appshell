//! Plain text in.

use crate::model::*;
use crate::text::{cp1252, utf16_bom};

pub fn read_txt(data: &[u8]) -> Document {
    let text = if let Some(rest) = data.strip_prefix(b"\xef\xbb\xbf") {
        String::from_utf8_lossy(rest).into_owned()
    } else if data.starts_with(b"\xff\xfe") || data.starts_with(b"\xfe\xff") {
        utf16_bom(data)
    } else {
        match std::str::from_utf8(data) {
            Ok(t) => t.to_string(),
            Err(_) => cp1252(data),
        }
    };
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut blocks = Vec::new();
    for line in text.split('\n') {
        if line == "\u{c}" {
            blocks.push(Block::PageBreak);
        } else {
            let runs = if line.is_empty() { vec![] } else { vec![Run::Text(Text::plain(line))] };
            blocks.push(Block::Para(Para::new(runs, "p", "")));
        }
    }
    if blocks.len() > 1 && matches!(blocks.last(), Some(Block::Para(p)) if p.runs.is_empty()) {
        blocks.pop(); // the file's last newline
    }
    if blocks.is_empty() {
        blocks.push(Block::Para(Para::default()));
    }
    Document { blocks, warnings: vec![] }
}
