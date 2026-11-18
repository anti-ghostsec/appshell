//! RTF in (Leaflet reads RTF but saves it as DOCX).

use crate::model::*;
use crate::text::decode_codepage;
use std::collections::BTreeSet;

const SKIP: &[&str] = &[
    "fonttbl", "colortbl", "stylesheet", "info", "pict", "header", "footer", "headerl", "headerr", "headerf", "footerl",
    "footerr", "footerf", "footnote", "annotation", "object", "listtable", "listoverridetable", "rsidtbl", "generator",
    "xmlnstbl", "themedata", "colorschememapping", "latentstyles", "datastore", "filetbl", "revtbl", "pgdsctbl", "mmathPr",
    "fldinst", "bkmkstart", "bkmkend", "shp", "shpinst", "nonshppict", "blipuid", "userprops", "docvar", "ftnsep", "ftnsepc",
    "aftnsep", "aftnsepc", "pn", "pntext", "listtext", "wgrffmtfilter", "xe", "tc", "field",
];

/// A number after a control word: kept exactly enough for what it's used for.
#[derive(Clone, Copy)]
struct Num {
    value: i64, // saturated
    mod65536: i64,
    nonzero: bool,
}

enum Tok<'a> {
    Word(&'a [u8], Option<Num>),
    Hex(u8),
    Sym(u8),
    Brace(u8),
    Newlines,
    Text(&'a [u8]),
}

/// The tokens Python's _RTF_TOKEN regex finds, in order.
fn tokens(data: &[u8]) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    let n = data.len();
    let mut i = 0;
    while i < n {
        let c = data[i];
        if c == b'\\' {
            if i + 1 >= n {
                i += 1; // a lone backslash at the end matches nothing
                continue;
            }
            let d = data[i + 1];
            if d.is_ascii_alphabetic() {
                let mut j = i + 1;
                while j < n && data[j].is_ascii_alphabetic() {
                    j += 1;
                }
                let word = &data[i + 1..j];
                let mut num = None;
                let neg = j < n && data[j] == b'-' && j + 1 < n && data[j + 1].is_ascii_digit();
                let start = if neg { j + 1 } else { j };
                if start < n && data[start].is_ascii_digit() {
                    let mut k = start;
                    let mut value: i64 = 0;
                    let mut m: i64 = 0;
                    let mut nonzero = false;
                    while k < n && data[k].is_ascii_digit() {
                        let dd = (data[k] - b'0') as i64;
                        value = value.saturating_mul(10).saturating_add(dd);
                        m = (m * 10 + dd) % 65536;
                        nonzero |= dd != 0;
                        k += 1;
                    }
                    if neg {
                        value = -value;
                        m = (65536 - m) % 65536;
                    }
                    num = Some(Num { value, mod65536: m, nonzero });
                    j = k;
                }
                if j < n && data[j] == b' ' {
                    j += 1;
                }
                out.push(Tok::Word(word, num));
                i = j;
                continue;
            }
            if d == b'\'' && i + 3 < n && data[i + 2].is_ascii_hexdigit() && data[i + 3].is_ascii_hexdigit() {
                let h = std::str::from_utf8(&data[i + 2..i + 4]).unwrap();
                out.push(Tok::Hex(u8::from_str_radix(h, 16).unwrap()));
                i += 4;
                continue;
            }
            out.push(Tok::Sym(d));
            i += 2;
            continue;
        }
        if c == b'{' || c == b'}' {
            out.push(Tok::Brace(c));
            i += 1;
            continue;
        }
        if c == b'\r' || c == b'\n' {
            while i < n && (data[i] == b'\r' || data[i] == b'\n') {
                i += 1;
            }
            out.push(Tok::Newlines);
            continue;
        }
        let start = i;
        while i < n && !matches!(data[i], b'\\' | b'{' | b'}' | b'\r' | b'\n') {
            i += 1;
        }
        out.push(Tok::Text(&data[start..i]));
    }
    out
}

#[derive(Clone, Copy, Default)]
struct State {
    b: bool,
    i: bool,
    u: bool,
    s: bool,
    sup: bool,
    sub: bool,
    skip: bool,
    uc: i64,
}

struct Rtf {
    warnings: BTreeSet<String>,
    blocks: Vec<Block>,
    runs: Vec<Run>,
    para: Para,
    state: State,
    codepage: u32,
    pending: Vec<u8>,
    in_table: bool,
    row_cells: Vec<Option<Para>>,
    rows: Vec<Vec<Cell>>,
    high_surrogate: Option<u16>,
}

impl Rtf {
    fn add(&mut self, text: &str) {
        if self.state.skip || text.is_empty() {
            return;
        }
        let s = self.state;
        self.runs.push(Run::Text(Text::new(text, Fmt { b: s.b, i: s.i, u: s.u, s: s.s, sup: s.sup, sub: s.sub, href: None })));
    }

    /// \uN: Python keeps a lone surrogate in its string, and a pair of them
    /// becomes one character on its way to the page; here the pair is joined.
    fn add_unit(&mut self, unit: u16) {
        if (0xD800..0xDC00).contains(&unit) {
            if self.high_surrogate.is_some() {
                self.add("\u{FFFD}");
            }
            self.high_surrogate = Some(unit);
            return;
        }
        if (0xDC00..0xE000).contains(&unit) {
            if let Some(hi) = self.high_surrogate.take() {
                let c = char::decode_utf16([hi, unit]).next().and_then(|r| r.ok()).unwrap_or('\u{FFFD}');
                self.add(&c.to_string());
            } else {
                self.add("\u{FFFD}");
            }
            return;
        }
        self.flush_surrogate();
        let c = char::from_u32(unit as u32).unwrap_or('\u{FFFD}');
        self.add(&c.to_string());
    }

    fn flush_surrogate(&mut self) {
        if self.high_surrogate.take().is_some() {
            self.add("\u{FFFD}");
        }
    }

    fn flush_bytes(&mut self) {
        if !self.pending.is_empty() {
            let bytes = std::mem::take(&mut self.pending);
            let t = decode_codepage(self.codepage, &bytes);
            self.add(&t);
        }
    }

    fn end_para(&mut self) {
        self.flush_bytes();
        let runs = std::mem::take(&mut self.runs);
        let align = self.para.align.clone();
        let mut para = std::mem::take(&mut self.para);
        para.runs = merge_runs(runs);
        if self.in_table {
            self.row_cells.push(Some(para));
        } else {
            self.blocks.push(Block::Para(para));
        }
        self.para = Para { align, ..Default::default() };
    }
}

pub fn read_rtf(data: &[u8]) -> Document {
    let mut r = Rtf {
        warnings: BTreeSet::new(),
        blocks: Vec::new(),
        runs: Vec::new(),
        para: Para::default(),
        state: State { uc: 1, ..Default::default() },
        codepage: 1252,
        pending: Vec::new(),
        in_table: false,
        row_cells: Vec::new(),
        rows: Vec::new(),
        high_surrogate: None,
    };
    let mut stack: Vec<State> = Vec::new();
    let mut skip_chars: i64 = 0;
    let mut group_start = true;
    for tok in tokens(data) {
        if let Tok::Hex(b) = tok {
            if skip_chars != 0 {
                skip_chars -= 1;
                continue;
            }
            r.pending.push(b);
            continue;
        }
        r.flush_bytes();
        match tok {
            Tok::Hex(_) => unreachable!(),
            Tok::Brace(b'{') => {
                stack.push(r.state);
                group_start = true;
            }
            Tok::Brace(_) => {
                if let Some(s) = stack.pop() {
                    r.state = s;
                }
                group_start = false;
            }
            Tok::Sym(sym) => {
                match sym {
                    b'*' => r.state.skip = true,
                    b'\\' | b'{' | b'}' => {
                        r.flush_surrogate();
                        r.add(&(sym as char).to_string())
                    }
                    b'~' => {
                        r.flush_surrogate();
                        r.add("\u{a0}")
                    }
                    b'_' => {
                        r.flush_surrogate();
                        r.add("\u{2011}")
                    }
                    b'\n' | b'\r' => r.end_para(),
                    _ => {}
                }
                group_start = false;
            }
            Tok::Word(word, num) => {
                let w = std::str::from_utf8(word).unwrap_or("");
                if group_start && SKIP.contains(&w) {
                    r.state.skip = true;
                    match w {
                        "pict" => {
                            r.warnings.insert("pictures in RTF files (not loaded)".into());
                        }
                        "header" | "footer" | "headerl" | "headerr" | "footerl" | "footerr" => {
                            r.warnings.insert("headers and footers".into());
                        }
                        "footnote" => {
                            r.warnings.insert("footnotes and endnotes".into());
                        }
                        _ => {}
                    }
                }
                group_start = false;
                let on = num.is_none_or(|n| n.nonzero);
                if w != "u" {
                    r.flush_surrogate();
                }
                match w {
                    "ansicpg" => {
                        if let Some(n) = num.filter(|n| n.nonzero) {
                            if n.value > 0 && n.value < u32::MAX as i64 {
                                r.codepage = n.value as u32;
                            } else {
                                r.codepage = 0;
                            }
                        }
                    }
                    "b" => r.state.b = on,
                    "i" => r.state.i = on,
                    "strike" => r.state.s = on,
                    "ul" => r.state.u = on,
                    "ulnone" => r.state.u = false,
                    "super" => {
                        r.state.sup = true;
                        r.state.sub = false;
                    }
                    "sub" => {
                        r.state.sup = false;
                        r.state.sub = true;
                    }
                    "nosupersub" => {
                        r.state.sup = false;
                        r.state.sub = false;
                    }
                    "plain" => {
                        let st = &mut r.state;
                        st.b = false;
                        st.i = false;
                        st.u = false;
                        st.s = false;
                        st.sup = false;
                        st.sub = false;
                    }
                    "par" | "sect" => r.end_para(),
                    "page" => {
                        r.end_para();
                        r.blocks.push(Block::PageBreak);
                    }
                    "line" => {
                        if !r.state.skip {
                            r.runs.push(Run::Break);
                        }
                    }
                    "tab" => r.add("\t"),
                    "pard" => r.para.align.clear(),
                    "qc" => r.para.align = "center".into(),
                    "qr" => r.para.align = "right".into(),
                    "qj" => r.para.align = "justify".into(),
                    "ql" => r.para.align.clear(),
                    "uc" => {
                        if let Some(n) = num {
                            r.state.uc = n.value;
                        }
                    }
                    "u" => {
                        if let Some(n) = num {
                            r.add_unit(n.mod65536 as u16);
                            skip_chars = r.state.uc;
                        }
                    }
                    "emdash" => r.add("\u{2014}"),
                    "endash" => r.add("\u{2013}"),
                    "bullet" => r.add("\u{2022}"),
                    "lquote" => r.add("\u{2018}"),
                    "rquote" => r.add("\u{2019}"),
                    "ldblquote" => r.add("\u{201c}"),
                    "rdblquote" => r.add("\u{201d}"),
                    "trowd" => r.in_table = true,
                    "cell" => {
                        r.flush_bytes();
                        let runs = std::mem::take(&mut r.runs);
                        let mut para = std::mem::take(&mut r.para);
                        para.runs = merge_runs(runs);
                        r.row_cells.push(Some(para));
                        r.row_cells.push(None);
                    }
                    "row" => {
                        let mut cells = Vec::new();
                        let mut cur = Vec::new();
                        for x in r.row_cells.drain(..) {
                            match x {
                                None => {
                                    let blocks = if cur.is_empty() { vec![Block::Para(Para::default())] } else { std::mem::take(&mut cur) };
                                    cells.push(Cell { blocks, colspan: 1 });
                                }
                                Some(p) => cur.push(Block::Para(p)),
                            }
                        }
                        if !cells.is_empty() {
                            r.rows.push(cells);
                        }
                        r.in_table = false;
                    }
                    "intbl" => r.in_table = true,
                    _ => {}
                }
                if !matches!(w, "intbl" | "trowd" | "cell" | "row") && !r.in_table && !r.rows.is_empty() {
                    let rows = std::mem::take(&mut r.rows);
                    r.blocks.push(Block::Table(Table { rows }));
                }
            }
            Tok::Newlines => {}
            Tok::Text(text) => {
                group_start = false;
                r.flush_surrogate();
                let mut t: Vec<char> = decode_codepage(r.codepage, text).chars().collect();
                if skip_chars != 0 {
                    // cut = min(skip_chars, len(t)); t = t[cut:]; skip_chars -= cut
                    let len = t.len() as i64;
                    let cut = skip_chars.min(len);
                    let start = if cut >= 0 { cut } else { (len + cut).max(0) };
                    t = t[start as usize..].to_vec();
                    skip_chars -= cut;
                }
                let s: String = t.into_iter().collect();
                r.add(&s);
            }
        }
    }
    r.flush_bytes();
    r.flush_surrogate();
    if !r.runs.is_empty() {
        r.end_para();
    }
    if !r.rows.is_empty() {
        let rows = std::mem::take(&mut r.rows);
        r.blocks.push(Block::Table(Table { rows }));
    }
    if r.blocks.is_empty() {
        r.blocks.push(Block::Para(Para::default()));
    }
    Document { blocks: r.blocks, warnings: r.warnings.into_iter().collect() }
}
