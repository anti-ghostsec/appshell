//! The model as the editor's HTML. Built from the model, never copied from a file.

use crate::model::*;
use crate::text::{b64encode, escape as x};

pub fn to_html(doc: &Document) -> String {
    let s: String = doc.blocks.iter().map(block_html).collect();
    if s.is_empty() {
        "<p><br></p>".into()
    } else {
        s
    }
}

fn is_heading(style: &str) -> bool {
    style.len() == 2 && style.starts_with('h') && (b'1'..=b'6').contains(&style.as_bytes()[1])
}

fn block_html(b: &Block) -> String {
    match b {
        Block::Para(p) => {
            let tag = if is_heading(&p.style) { p.style.as_str() } else { "p" };
            let style = if p.align.is_empty() { String::new() } else { format!(" style=\"text-align: {}\"", p.align) };
            let inner = runs_html(&p.runs);
            format!("<{tag}{style}>{}</{tag}>", if inner.is_empty() { "<br>" } else { &inner })
        }
        Block::List(l) => {
            let tag = if l.ordered { "ol" } else { "ul" };
            let mut items = String::new();
            for item in &l.items {
                let mut parts = String::new();
                for (k, b) in item.iter().enumerate() {
                    match b {
                        Block::Para(p) if k == 0 && p.style == "p" && p.align.is_empty() => {
                            let r = runs_html(&p.runs);
                            parts.push_str(if r.is_empty() { "<br>" } else { &r });
                        }
                        other => parts.push_str(&block_html(other)),
                    }
                }
                items.push_str(&format!("<li>{}</li>", if parts.is_empty() { "<br>" } else { &parts }));
            }
            format!("<{tag}>{items}</{tag}>")
        }
        Block::Table(t) => {
            let mut rows = String::new();
            for row in &t.rows {
                let mut cells = String::new();
                for c in row {
                    let span = if c.colspan > 1 { format!(" colspan=\"{}\"", c.colspan) } else { String::new() };
                    let inner: String = c.blocks.iter().map(block_html).collect();
                    cells.push_str(&format!("<td{span}>{}</td>", if inner.is_empty() { "<p><br></p>" } else { &inner }));
                }
                rows.push_str(&format!("<tr>{cells}</tr>"));
            }
            format!("<table><tbody>{rows}</tbody></table>")
        }
        Block::PageBreak => "<hr class=\"page-break\">".into(),
    }
}

fn runs_html(runs: &[Run]) -> String {
    let mut out = String::new();
    for r in runs {
        match r {
            Run::Break => out.push_str("<br>"),
            Run::Image(im) => {
                let src = format!("data:{};base64,{}", im.mime, b64encode(&im.data));
                let mut size = String::new();
                if im.width != 0 {
                    size.push_str(&format!(" width=\"{}\"", im.width));
                }
                if im.height != 0 {
                    size.push_str(&format!(" height=\"{}\"", im.height));
                }
                out.push_str(&format!("<img src=\"{src}\"{size} alt=\"{}\">", x(&im.alt)));
            }
            Run::Text(t) => {
                let escaped = x(&t.text).replace('\t', "<span class=\"tab\">\t</span>");
                let mut s = keep_spaces(&escaped);
                let f = &t.fmt;
                for (flag, tag) in [(f.sub, "sub"), (f.sup, "sup"), (f.s, "s"), (f.u, "u"), (f.i, "i"), (f.b, "b")] {
                    if flag {
                        s = format!("<{tag}>{s}</{tag}>");
                    }
                }
                if let Some(href) = f.href.as_deref().filter(|h| !h.is_empty()) {
                    s = format!("<a href=\"{}\">{s}</a>", x(href));
                }
                out.push_str(&s);
            }
            Run::PageBreak => {}
        }
    }
    out
}

/// re.sub(r'  +', lambda m: ' ' + ' ' * (len(m.group()) - 1), t)
fn keep_spaces(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut run = 0usize;
    let flush = |out: &mut String, run: usize| {
        if run == 1 {
            out.push(' ');
        } else if run > 1 {
            out.push(' ');
            for _ in 1..run {
                out.push('\u{a0}');
            }
        }
    };
    for c in t.chars() {
        if c == ' ' {
            run += 1;
        } else {
            flush(&mut out, run);
            run = 0;
            out.push(c);
        }
    }
    flush(&mut out, run);
    out
}
