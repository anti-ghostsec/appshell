//! The editor's HTML back into the model. Unknown tags keep their text.
//!
//! The HTML is what the page's editor serialises (innerHTML), so the parser
//! follows Python's HTMLParser for that kind of input: lower-cased names,
//! entities decoded, comments and declarations skipped, script and style
//! read as raw text.

use crate::images::sniff_image;
use crate::model::*;
use crate::text::{b64decode, is_web_link, py_int};
use crate::MAX_IMAGE;

#[derive(Debug)]
enum Child {
    Text(String),
    Node(usize),
}

#[derive(Debug)]
struct Node {
    tag: String,
    attrs: Vec<(String, Option<String>)>,
    children: Vec<Child>,
    parent: Option<usize>,
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        // dict(attrs): the last one with the name wins
        self.attrs.iter().rev().find(|(k, _)| k == name).and_then(|(_, v)| v.as_deref())
    }
}

struct Tree {
    nodes: Vec<Node>,
}

const VOID: &[&str] = &["br", "img", "hr", "input", "meta", "link", "col", "wbr", "area", "base", "source"];
const DROP: &[&str] = &[
    "script", "style", "head", "title", "template", "noscript", "iframe", "object", "embed", "svg", "math", "button", "select",
    "textarea", "canvas", "video", "audio",
];
const BLOCK_TAGS: &[&str] = &[
    "p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "table", "hr", "blockquote", "pre", "section", "article",
    "header", "footer", "li", "dl", "dt", "dd", "figure", "figcaption", "address", "main", "nav", "aside",
];

fn is_heading(tag: &str) -> bool {
    tag.len() == 2 && tag.starts_with('h') && (b'1'..=b'6').contains(&tag.as_bytes()[1])
}

// ------------------------------------------------------------ entities

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'&' {
            let next = s[i..].find('&').map_or(s.len(), |p| i + p);
            out.push_str(&s[i..next]);
            i = next;
            continue;
        }
        if let Some((c, len)) = entity_at(&s[i..]) {
            out.push_str(&c);
            i += len;
        } else {
            out.push('&');
            i += 1;
        }
    }
    out
}

fn entity_at(s: &str) -> Option<(String, usize)> {
    let rest = &s[1..];
    if let Some(num) = rest.strip_prefix('#') {
        let (hex, digits) = match num.strip_prefix(['x', 'X']) {
            Some(h) => (true, h),
            None => (false, num),
        };
        let len = digits.bytes().take_while(|c| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() }).count();
        if len == 0 {
            return None;
        }
        let v = u32::from_str_radix(&digits[..len], if hex { 16 } else { 10 }).unwrap_or(0x110000);
        let c = match v {
            0 => '\u{FFFD}',
            0x80..=0x9F => crate::text::cp1252(&[v as u8]).chars().next().unwrap(),
            _ => char::from_u32(v).unwrap_or('\u{FFFD}'),
        };
        let mut used = 1 + 1 + (hex as usize) + len;
        if s.as_bytes().get(used) == Some(&b';') {
            used += 1;
        }
        return Some((c.to_string(), used));
    }
    const NAMED: &[(&str, &str)] = &[
        ("amp", "&"),
        ("lt", "<"),
        ("gt", ">"),
        ("quot", "\""),
        ("apos", "'"),
        ("nbsp", "\u{a0}"),
        ("shy", "\u{ad}"),
        ("copy", "©"),
        ("reg", "®"),
        ("trade", "™"),
        ("hellip", "…"),
        ("mdash", "—"),
        ("ndash", "–"),
        ("lsquo", "‘"),
        ("rsquo", "’"),
        ("ldquo", "“"),
        ("rdquo", "”"),
        ("bull", "•"),
        ("middot", "·"),
        ("euro", "€"),
        ("zwj", "\u{200d}"),
        ("zwnj", "\u{200c}"),
        ("lrm", "\u{200e}"),
        ("rlm", "\u{200f}"),
    ];
    for (name, val) in NAMED {
        if rest.starts_with(name) && rest.as_bytes().get(name.len()) == Some(&b';') {
            return Some((val.to_string(), 1 + name.len() + 1));
        }
    }
    // the few that html.unescape also takes without a semicolon
    for (name, val) in &NAMED[..6] {
        if rest.starts_with(name) {
            return Some((val.to_string(), 1 + name.len()));
        }
    }
    None
}

// ------------------------------------------------------------ parsing

fn parse(source: &str) -> Tree {
    let mut tree = Tree { nodes: vec![Node { tag: "root".into(), attrs: vec![], children: vec![], parent: None }] };
    let mut cur = 0usize;
    let s = source;
    let b = s.as_bytes();
    let mut i = 0;
    let mut text = String::new();
    let flush = |tree: &mut Tree, cur: usize, text: &mut String| {
        if !text.is_empty() {
            let t = decode_entities(text);
            tree.nodes[cur].children.push(Child::Text(t));
            text.clear();
        }
    };
    while i < b.len() {
        if b[i] != b'<' {
            let next = s[i..].find('<').map_or(s.len(), |p| i + p);
            text.push_str(&s[i..next]);
            i = next;
            continue;
        }
        let rest = &s[i..];
        if rest.starts_with("<!--") {
            flush(&mut tree, cur, &mut text);
            i += rest.find("-->").map_or(rest.len(), |p| p + 3);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            flush(&mut tree, cur, &mut text);
            i += rest.find('>').map_or(rest.len(), |p| p + 1);
            continue;
        }
        if let Some(after) = rest.strip_prefix("</") {
            if after.as_bytes().first().is_some_and(|c| c.is_ascii_alphabetic()) {
                flush(&mut tree, cur, &mut text);
                let end = after.find('>').map_or(after.len(), |p| p + 1);
                let name: String = after[..end].chars().take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/').collect();
                let name = name.to_lowercase();
                // handle_endtag: back up to the matching open element, if any
                let mut n = Some(cur);
                while let Some(k) = n {
                    if tree.nodes[k].tag == name {
                        break;
                    }
                    n = tree.nodes[k].parent;
                }
                if let Some(k) = n {
                    if let Some(p) = tree.nodes[k].parent {
                        cur = p;
                    }
                }
                i += 2 + end;
                continue;
            }
            text.push_str("</");
            i += 2;
            continue;
        }
        if !rest.as_bytes().get(1).is_some_and(|c| c.is_ascii_alphabetic()) {
            text.push('<');
            i += 1;
            continue;
        }
        flush(&mut tree, cur, &mut text);
        let (tag, attrs, self_closing, used) = start_tag(rest);
        i += used;
        let idx = tree.nodes.len();
        tree.nodes.push(Node { tag: tag.clone(), attrs, children: vec![], parent: Some(cur) });
        tree.nodes[cur].children.push(Child::Node(idx));
        if self_closing {
            continue;
        }
        if tag == "script" || tag == "style" {
            // raw text up to the closing tag
            let lower = s[i..].to_ascii_lowercase();
            let close = format!("</{tag}");
            let end = lower.find(&close).map_or(s.len(), |p| i + p);
            if end > i {
                tree.nodes[idx].children.push(Child::Text(s[i..end].to_string()));
            }
            i = end;
            if i < s.len() {
                i += s[i..].find('>').map_or(s.len() - i, |p| p + 1);
            }
            continue;
        }
        if !VOID.contains(&tag.as_str()) {
            cur = idx;
        }
    }
    flush(&mut tree, cur, &mut text);
    tree
}

/// "<name a="v" b='w' c=x d>" -> (name, attrs, ends with "/>", bytes used)
fn start_tag(s: &str) -> (String, Vec<(String, Option<String>)>, bool, usize) {
    let b = s.as_bytes();
    let mut i = 1;
    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' && b[i] != b'/' {
        i += 1;
    }
    let tag = s[1..i].to_lowercase();
    let mut attrs = Vec::new();
    let mut self_closing = false;
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || (b[i] == b'/' && b.get(i + 1) != Some(&b'>'))) {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if b[i] == b'>' {
            i += 1;
            break;
        }
        if b[i] == b'/' {
            self_closing = true;
            i += 2;
            break;
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>') && !(b[i] == b'/' && b.get(i + 1) == Some(&b'>')) {
            i += 1;
        }
        let name = s[start..i].to_lowercase();
        let mut j = i;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if j < b.len() && b[j] == b'=' {
            j += 1;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            let value;
            if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                let q = b[j];
                let close = s[j + 1..].find(q as char).map_or(s.len(), |p| j + 1 + p);
                value = &s[j + 1..close];
                i = (close + 1).min(s.len());
            } else {
                let vs = j;
                while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                    j += 1;
                }
                value = &s[vs..j];
                i = j;
            }
            attrs.push((name, Some(decode_entities(value))));
        } else {
            attrs.push((name, None));
        }
    }
    (tag, attrs, self_closing, i)
}

// ------------------------------------------------------------ to the model

pub fn from_html(source: &str) -> Document {
    let tree = parse(source);
    let root = &tree.nodes[0];
    Document { blocks: tree.blocks(&root.children, &Fmt::default(), ""), warnings: vec![] }
}

fn style_of(node: &Node) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for decl in node.attr("style").unwrap_or("").split(';') {
        if let Some((k, v)) = decl.split_once(':') {
            let k = k.trim().to_lowercase();
            let v = v.trim().to_lowercase();
            if let Some(slot) = out.iter_mut().find(|(kk, _)| *kk == k) {
                slot.1 = v;
            } else {
                out.push((k, v));
            }
        }
    }
    out
}

fn get<'a>(st: &'a [(String, String)], k: &str) -> Option<&'a str> {
    st.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str())
}

fn fmt_of(node: &Node, fmt: &Fmt) -> Fmt {
    let mut f = fmt.clone();
    match node.tag.as_str() {
        "b" | "strong" => f.b = true,
        "i" | "em" | "cite" | "var" | "dfn" => f.i = true,
        "u" | "ins" => f.u = true,
        "s" | "strike" | "del" => f.s = true,
        "sup" => {
            f.sup = true;
            f.sub = false;
        }
        "sub" => {
            f.sub = true;
            f.sup = false;
        }
        "a" => {
            let href = node.attr("href").unwrap_or("");
            if is_web_link(href) {
                f.href = Some(href.to_string());
            }
        }
        _ => {}
    }
    let st = style_of(node);
    if let Some(w) = get(&st, "font-weight") {
        f.b = matches!(w, "bold" | "bolder" | "600" | "700" | "800" | "900");
    }
    if let Some(s) = get(&st, "font-style") {
        f.i = matches!(s, "italic" | "oblique");
    }
    let deco = get(&st, "text-decoration").filter(|d| !d.is_empty()).or_else(|| get(&st, "text-decoration-line")).unwrap_or("");
    if deco.contains("underline") {
        f.u = true;
    }
    if deco.contains("line-through") {
        f.s = true;
    }
    match get(&st, "vertical-align") {
        Some("super") => f.sup = true,
        Some("sub") => f.sub = true,
        _ => {}
    }
    f
}

fn image_of(node: &Node) -> Option<Image> {
    let src = node.attr("src").unwrap_or("");
    // re.match(r'data:(image/(?:png|jpeg|jpg|gif|bmp));base64,([A-Za-z0-9+/=\s]+)$', src)
    let rest = src.strip_prefix("data:image/")?;
    let (sub, payload) = rest.split_once(";base64,")?;
    if !matches!(sub, "png" | "jpeg" | "jpg" | "gif" | "bmp") {
        return None;
    }
    let payload = payload.strip_suffix('\n').unwrap_or(payload);
    if payload.is_empty() || !payload.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=' || c.is_whitespace()) {
        return None;
    }
    let data = b64decode(payload)?;
    let mime = sniff_image(&data)?;
    if data.len() > MAX_IMAGE {
        return None;
    }
    let num = |v: Option<&str>| -> i64 {
        let v = v.unwrap_or("").trim();
        let v = v.strip_suffix("px").unwrap_or(v);
        // re.fullmatch(r'\d+(\.\d+)?', v)
        let (int, frac) = match v.split_once('.') {
            Some((a, b)) => (a, Some(b)),
            None => (v, None),
        };
        let ok = !int.is_empty() && int.bytes().all(|c| c.is_ascii_digit()) && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|c| c.is_ascii_digit()));
        if ok {
            v.parse::<f64>().map(|f| f as i64).unwrap_or(0)
        } else {
            0
        }
    };
    let st = style_of(node);
    let width = match num(node.attr("width")) {
        0 => num(get(&st, "width")),
        n => n,
    };
    let height = match num(node.attr("height")) {
        0 => num(get(&st, "height")),
        n => n,
    };
    Some(Image { data, mime: mime.into(), width, height, alt: node.attr("alt").unwrap_or("").to_string() })
}

fn text_run(child: &str, fmt: &Fmt, out: &mut Vec<Run>) {
    // re.sub(r'[ \r\n]+', ' ', child).replace(' ', ' ')
    let mut t = String::with_capacity(child.len());
    let mut ws = false;
    for c in child.chars() {
        if matches!(c, ' ' | '\r' | '\n') {
            if !ws {
                t.push(' ');
            }
            ws = true;
        } else {
            t.push(if c == '\u{a0}' { ' ' } else { c });
            ws = false;
        }
    }
    if !t.is_empty() {
        out.push(Run::Text(Text::new(t, fmt.clone())));
    }
}

fn clean_runs(runs: Vec<Run>) -> Vec<Run> {
    let mut runs = merge_runs(runs);
    if matches!(runs.last(), Some(Run::Break)) {
        runs.pop();
    }
    if let Some(Run::Text(t)) = runs.first_mut() {
        t.text = t.text.trim_start_matches(' ').to_string();
    }
    if let Some(Run::Text(t)) = runs.last_mut() {
        t.text = t.text.trim_end_matches(' ').to_string();
    }
    runs.into_iter().filter(|r| !matches!(r, Run::Text(t) if t.text.is_empty())).collect()
}

fn align_of(node: &Node) -> &'static str {
    let st = style_of(node);
    let a = get(&st, "text-align").filter(|a| !a.is_empty()).map(str::to_string).unwrap_or_else(|| node.attr("align").unwrap_or("").to_lowercase());
    match a.as_str() {
        "center" => "center",
        "right" | "end" => "right",
        "justify" => "justify",
        _ => "",
    }
}

impl Tree {
    fn node(&self, c: &Child) -> Option<&Node> {
        match c {
            Child::Node(i) => Some(&self.nodes[*i]),
            Child::Text(_) => None,
        }
    }

    fn inline(&self, node: &Node, fmt: &Fmt, out: &mut Vec<Run>) {
        for child in &node.children {
            match child {
                Child::Text(t) => text_run(t, fmt, out),
                Child::Node(i) => {
                    let c = &self.nodes[*i];
                    if DROP.contains(&c.tag.as_str()) {
                        continue;
                    }
                    if c.tag == "br" {
                        out.push(Run::Break);
                    } else if c.tag == "img" {
                        if let Some(im) = image_of(c) {
                            out.push(Run::Image(im));
                        }
                    } else {
                        self.inline(c, &fmt_of(c, fmt), out);
                    }
                }
            }
        }
    }

    fn has_block_inside(&self, node: &Node) -> bool {
        node.children.iter().any(|c| match self.node(c) {
            Some(n) => BLOCK_TAGS.contains(&n.tag.as_str()) || self.has_block_inside(n),
            None => false,
        })
    }

    fn blocks(&self, children: &[Child], fmt: &Fmt, align: &str) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut pending: Vec<Run> = Vec::new();
        let flush = |pending: &mut Vec<Run>, blocks: &mut Vec<Block>| {
            if !pending.is_empty() {
                let runs = clean_runs(std::mem::take(pending));
                if !runs.is_empty() {
                    blocks.push(Block::Para(Para::new(runs, "p", align)));
                }
            }
        };
        for child in children {
            let node = match child {
                Child::Text(t) => {
                    if !t.trim().is_empty() || !pending.is_empty() {
                        text_run(t, fmt, &mut pending);
                    }
                    continue;
                }
                Child::Node(i) => &self.nodes[*i],
            };
            let tag = node.tag.as_str();
            if !BLOCK_TAGS.contains(&tag) {
                if DROP.contains(&tag) {
                    continue;
                } else if tag == "br" {
                    if !pending.is_empty() {
                        pending.push(Run::Break);
                    } else {
                        blocks.push(Block::Para(Para::new(vec![], "p", align)));
                    }
                } else if tag == "img" {
                    if let Some(im) = image_of(node) {
                        pending.push(Run::Image(im));
                    }
                } else if self.has_block_inside(node) {
                    flush(&mut pending, &mut blocks);
                    blocks.extend(self.blocks(&node.children, &fmt_of(node, fmt), align));
                } else {
                    self.inline(node, &fmt_of(node, fmt), &mut pending);
                }
                continue;
            }
            flush(&mut pending, &mut blocks);
            if matches!(tag, "p" | "div" | "pre" | "address" | "dt" | "dd" | "figcaption") || is_heading(tag) {
                let inner_blocks = node.children.iter().any(|c| self.node(c).is_some_and(|n| BLOCK_TAGS.contains(&n.tag.as_str())));
                let a = match align_of(node) {
                    "" => align,
                    a => a,
                };
                if inner_blocks {
                    blocks.extend(self.blocks(&node.children, &fmt_of(node, fmt), a));
                    continue;
                }
                let mut runs = Vec::new();
                let f = if tag.starts_with('h') { fmt.clone() } else { fmt_of(node, fmt) };
                self.inline(node, &f, &mut runs);
                let runs = clean_runs(runs);
                let style = if is_heading(tag) { tag } else { "p" };
                if tag == "div" && runs.is_empty() && node.children.is_empty() {
                    continue;
                }
                blocks.push(Block::Para(Para::new(runs, style, a)));
            } else if tag == "ul" || tag == "ol" {
                let mut lst = ListBlock { ordered: tag == "ol", items: Vec::new() };
                for li in &node.children {
                    let Some(n) = self.node(li) else { continue };
                    if n.tag == "li" {
                        let item = self.blocks(&n.children, fmt, "");
                        lst.items.push(if item.is_empty() { vec![Block::Para(Para::default())] } else { item });
                    } else if n.tag == "ul" || n.tag == "ol" {
                        // a list directly inside a list belongs to the previous item
                        let sub = self.blocks(std::slice::from_ref(li), fmt, "");
                        if let Some(last) = lst.items.last_mut() {
                            last.extend(sub);
                        } else {
                            let mut item = vec![Block::Para(Para::default())];
                            item.extend(sub);
                            lst.items.push(item);
                        }
                    }
                }
                if !lst.items.is_empty() {
                    blocks.push(Block::List(lst));
                }
            } else if tag == "li" {
                blocks.extend(self.blocks(&node.children, fmt, align));
            } else if tag == "table" {
                let mut t = Table::default();
                for tr in self.rows(node) {
                    let mut row = Vec::new();
                    for td in &tr.children {
                        let Some(td) = self.node(td) else { continue };
                        if td.tag == "td" || td.tag == "th" {
                            let span = py_int(td.attr("colspan").filter(|s| !s.is_empty()).unwrap_or("1")).unwrap_or(1).clamp(1, 64);
                            let cfmt = if td.tag == "th" { Fmt { b: true, ..fmt.clone() } } else { fmt.clone() };
                            let b = self.blocks(&td.children, &cfmt, "");
                            row.push(Cell { blocks: if b.is_empty() { vec![Block::Para(Para::default())] } else { b }, colspan: span });
                        }
                    }
                    if !row.is_empty() {
                        t.rows.push(row);
                    }
                }
                if !t.rows.is_empty() {
                    blocks.push(Block::Table(t));
                }
            } else if tag == "hr" {
                if node.attr("class").unwrap_or("").contains("page-break") {
                    blocks.push(Block::PageBreak);
                }
            } else {
                // blockquote, section, figure...
                let a = match align_of(node) {
                    "" => align,
                    a => a,
                };
                blocks.extend(self.blocks(&node.children, &fmt_of(node, fmt), a));
            }
        }
        flush(&mut pending, &mut blocks);
        blocks
    }

    fn rows<'a>(&'a self, table: &'a Node) -> Vec<&'a Node> {
        let mut out = Vec::new();
        for c in &table.children {
            if let Some(n) = self.node(c) {
                if n.tag == "tr" {
                    out.push(n);
                } else if matches!(n.tag.as_str(), "tbody" | "thead" | "tfoot") {
                    out.extend(self.rows(n));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_html() {
        let d = from_html("<p>Hello <b>bold</b>&nbsp;&amp; <a href=\"https://x.test/\">link</a></p><ul><li>one</li><li>two<ul><li>sub</li></ul></li></ul><hr class=\"page-break\"><p><br></p>");
        assert_eq!(d.blocks.len(), 4);
        let Block::Para(p) = &d.blocks[0] else { panic!() };
        assert_eq!(p.runs.len(), 4);
        let Block::List(l) = &d.blocks[1] else { panic!() };
        assert_eq!(l.items.len(), 2);
        assert!(matches!(d.blocks[2], Block::PageBreak));
    }
}
