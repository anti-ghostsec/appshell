//! The small document model every format is read into and written from.

#[derive(Clone, Debug, PartialEq, Eq, Default, Hash)]
pub struct Fmt {
    pub b: bool,
    pub i: bool,
    pub u: bool,
    pub s: bool,
    pub sup: bool,
    pub sub: bool,
    pub href: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub text: String,
    pub fmt: Fmt,
}

impl Text {
    pub fn new(text: impl Into<String>, fmt: Fmt) -> Text {
        Text { text: text.into(), fmt }
    }
    pub fn plain(text: impl Into<String>) -> Text {
        Text { text: text.into(), fmt: Fmt::default() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub data: Vec<u8>,
    pub mime: String,
    pub width: i64,
    pub height: i64,
    pub alt: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Run {
    Text(Text),
    Image(Image),
    Break,
    /// Only while a DOCX paragraph is being read: it splits the paragraph.
    PageBreak,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Para {
    pub runs: Vec<Run>,
    pub style: String,
    pub align: String,
}

impl Default for Para {
    fn default() -> Para {
        Para { runs: Vec::new(), style: "p".into(), align: String::new() }
    }
}

impl Para {
    pub fn new(runs: Vec<Run>, style: &str, align: &str) -> Para {
        Para { runs, style: style.into(), align: align.into() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListBlock {
    pub ordered: bool,
    pub items: Vec<Vec<Block>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub blocks: Vec<Block>,
    pub colspan: i64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Table {
    pub rows: Vec<Vec<Cell>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Para(Para),
    List(ListBlock),
    Table(Table),
    PageBreak,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub warnings: Vec<String>,
}

/// Join neighbouring text runs that look the same; drop empty ones.
pub fn merge_runs(runs: Vec<Run>) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::with_capacity(runs.len());
    for r in runs {
        if let Run::Text(t) = &r {
            if t.text.is_empty() {
                continue;
            }
            if let Some(Run::Text(last)) = out.last_mut() {
                if last.fmt == t.fmt {
                    last.text.push_str(&t.text);
                    continue;
                }
            }
        }
        out.push(r);
    }
    out
}

pub fn plain_text(blocks: &[Block]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for b in blocks {
        match b {
            Block::Para(p) => lines.push(
                p.runs
                    .iter()
                    .map(|r| match r {
                        Run::Text(t) => t.text.as_str(),
                        Run::Break => "\n",
                        _ => "",
                    })
                    .collect(),
            ),
            Block::List(l) => {
                for (n, item) in l.items.iter().enumerate() {
                    let text = plain_text(item);
                    let sub: Vec<&str> = text.split('\n').collect();
                    let mark = if l.ordered { format!("{}. ", n + 1) } else { "• ".to_string() };
                    lines.push(format!("{mark}{}", sub[0]));
                    for s in &sub[1..] {
                        lines.push(format!("   {s}"));
                    }
                }
            }
            Block::Table(t) => {
                for row in &t.rows {
                    lines.push(row.iter().map(|c| plain_text(&c.blocks).replace('\n', " ")).collect::<Vec<_>>().join("\t"));
                }
            }
            Block::PageBreak => lines.push("\u{c}".into()),
        }
    }
    lines.join("\n")
}
