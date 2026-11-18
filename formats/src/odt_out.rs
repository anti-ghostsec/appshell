//! ODT out.

use crate::docx_out::heading_pt;
use crate::images::{ext_of, image_size, strip_image_metadata};
use crate::model::*;
use crate::odt_in::{ns, NS};
use crate::text::{escape as x, xml_safe};
use crate::zipio::make_zip;

fn odt_ns() -> String {
    NS.iter()
        .filter(|(k, _)| !matches!(*k, "manifest" | "meta" | "loext"))
        .map(|(k, v)| format!("xmlns:{k}=\"{v}\""))
        .collect::<Vec<_>>()
        .join(" ")
}

fn styles_xml() -> String {
    let heads: String = (1..=6)
        .map(|n| {
            format!(
                "<style:style style:name=\"Heading_20_{n}\" style:display-name=\"Heading {n}\" style:family=\"paragraph\" \
style:parent-style-name=\"Heading\" style:next-style-name=\"Text_20_body\" style:default-outline-level=\"{n}\" \
style:class=\"text\"><style:text-properties fo:font-size=\"{}pt\" fo:font-weight=\"bold\" \
style:font-weight-asian=\"bold\" style:font-weight-complex=\"bold\"/></style:style>",
                heading_pt(n)
            )
        })
        .collect();
    let ns = odt_ns();
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<office:document-styles {ns} office:version="1.3">
<office:font-face-decls><style:font-face style:name="Liberation Sans" svg:font-family="&apos;Liberation Sans&apos;" style:font-family-generic="swiss"/></office:font-face-decls>
<office:styles>
<style:default-style style:family="paragraph"><style:paragraph-properties fo:margin-top="0cm" fo:margin-bottom="0.21cm"/>
<style:text-properties style:font-name="Liberation Sans" fo:font-size="11pt" fo:language="en" fo:country="US"/></style:default-style>
<style:style style:name="Standard" style:family="paragraph" style:class="text"/>
<style:style style:name="Text_20_body" style:display-name="Text body" style:family="paragraph" style:parent-style-name="Standard" style:class="text"/>
<style:style style:name="Heading" style:family="paragraph" style:parent-style-name="Standard" style:next-style-name="Text_20_body" style:class="text">
<style:paragraph-properties fo:margin-top="0.42cm" fo:margin-bottom="0.21cm" fo:keep-with-next="always"/></style:style>
{heads}
<style:style style:name="Table_20_Contents" style:display-name="Table Contents" style:family="paragraph" style:parent-style-name="Standard" style:class="extra">
<style:paragraph-properties fo:margin-bottom="0cm"/></style:style>
<style:style style:name="Internet_20_link" style:display-name="Internet link" style:family="text">
<style:text-properties fo:color="#000080" style:text-underline-style="solid" style:text-underline-width="auto" style:text-underline-color="font-color"/></style:style>
</office:styles>
<office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="21.001cm" fo:page-height="29.7cm"
 style:print-orientation="portrait" fo:margin-top="2cm" fo:margin-bottom="2cm" fo:margin-left="2.5cm" fo:margin-right="2.5cm"/></style:page-layout></office:automatic-styles>
<office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1"/></office:master-styles>
</office:document-styles>"##
    )
}

type TextKey = (bool, bool, bool, bool, bool, bool);

#[derive(Default)]
struct Writer {
    text_styles: Vec<(TextKey, String)>,
    para_styles: Vec<((String, String, bool), String)>,
    pictures: Vec<(String, String, Vec<u8>)>,
    tables: usize,
    lists: Vec<(bool, &'static str)>,
    pending_break: bool,
}

pub fn write_odt(doc: &Document) -> Vec<u8> {
    Writer::default().write(doc)
}

impl Writer {
    fn text_style(&mut self, f: &Fmt) -> Option<String> {
        let key = (f.b, f.i, f.u, f.s, f.sup, f.sub);
        if !(f.b || f.i || f.u || f.s || f.sup || f.sub) {
            return None;
        }
        if let Some((_, n)) = self.text_styles.iter().find(|(k, _)| *k == key) {
            return Some(n.clone());
        }
        let name = format!("T{}", self.text_styles.len() + 1);
        self.text_styles.push((key, name.clone()));
        Some(name)
    }

    fn para_style(&mut self, parent: &str, align: &str, brk: bool) -> String {
        if align.is_empty() && !brk {
            return parent.to_string();
        }
        let key = (parent.to_string(), align.to_string(), brk);
        if let Some((_, n)) = self.para_styles.iter().find(|(k, _)| *k == key) {
            return n.clone();
        }
        let name = format!("P{}", self.para_styles.len() + 1);
        self.para_styles.push((key, name.clone()));
        name
    }

    fn list_style(&mut self, ordered: bool) -> &'static str {
        if let Some((_, n)) = self.lists.iter().find(|(k, _)| *k == ordered) {
            return n;
        }
        let name = if ordered { "LNum" } else { "LBul" };
        self.lists.push((ordered, name));
        name
    }

    fn write(mut self, doc: &Document) -> Vec<u8> {
        self.pending_break = false;
        let mut body: String = doc.blocks.iter().map(|b| self.block(b, false)).collect();
        if body.is_empty() {
            body = "<text:p text:style-name=\"Standard\"/>".into();
        }
        if self.pending_break {
            let s = self.para_style("Standard", "", true);
            body.push_str(&format!("<text:p text:style-name=\"{s}\"/>"));
        }
        let mut auto = Vec::new();
        for ((b, i, u, s, sup, sub), name) in &self.text_styles {
            let mut props = String::new();
            if *b {
                props.push_str(" fo:font-weight=\"bold\" style:font-weight-asian=\"bold\" style:font-weight-complex=\"bold\"");
            }
            if *i {
                props.push_str(" fo:font-style=\"italic\" style:font-style-asian=\"italic\" style:font-style-complex=\"italic\"");
            }
            if *u {
                props.push_str(" style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" style:text-underline-color=\"font-color\"");
            }
            if *s {
                props.push_str(" style:text-line-through-style=\"solid\" style:text-line-through-type=\"single\"");
            }
            if *sup || *sub {
                props.push_str(&format!(" style:text-position=\"{} 58%\"", if *sup { "super" } else { "sub" }));
            }
            auto.push(format!("<style:style style:name=\"{name}\" style:family=\"text\"><style:text-properties{props}/></style:style>"));
        }
        for ((parent, align, brk), name) in &self.para_styles {
            let mut props = String::new();
            if !align.is_empty() {
                let a = match align.as_str() {
                    "right" => "end",
                    "center" => "center",
                    _ => "justify",
                };
                props.push_str(&format!(" fo:text-align=\"{a}\""));
            }
            if *brk {
                props.push_str(" fo:break-before=\"page\"");
            }
            auto.push(format!(
                "<style:style style:name=\"{name}\" style:family=\"paragraph\" style:parent-style-name=\"{parent}\">\
<style:paragraph-properties{props}/></style:style>"
            ));
        }
        for n in 1..=self.tables {
            auto.push(format!(
                "<style:style style:name=\"Table{n}\" style:family=\"table\"><style:table-properties style:width=\"16cm\" table:align=\"margins\"/></style:style>"
            ));
        }
        if self.tables > 0 {
            auto.push(
                "<style:style style:name=\"TableCell\" style:family=\"table-cell\"><style:table-cell-properties \
fo:padding=\"0.1cm\" fo:border=\"0.5pt solid #000000\"/></style:style>"
                    .into(),
            );
        }
        auto.push(
            "<style:style style:name=\"fr1\" style:family=\"graphic\"><style:graphic-properties \
style:vertical-pos=\"top\" style:vertical-rel=\"baseline\" style:horizontal-pos=\"center\" style:horizontal-rel=\"paragraph\"/></style:style>"
                .into(),
        );
        for (ordered, name) in &self.lists {
            let mut levels = String::new();
            for n in 1..=10 {
                let pos = 0.635 * n as f64;
                let indent = format!(
                    "<style:list-level-properties text:list-level-position-and-space-mode=\"label-alignment\">\
<style:list-level-label-alignment text:label-followed-by=\"listtab\" text:list-tab-stop-position=\"{pos:.3}cm\" \
fo:text-indent=\"-0.635cm\" fo:margin-left=\"{pos:.3}cm\"/></style:list-level-properties>"
                );
                if *ordered {
                    let fmt = ["1", "a", "i"][(n - 1) % 3];
                    levels.push_str(&format!(
                        "<text:list-level-style-number text:level=\"{n}\" style:num-suffix=\".\" style:num-format=\"{fmt}\">{indent}</text:list-level-style-number>"
                    ));
                } else {
                    let ch = ["\u{2022}", "\u{25e6}", "\u{25aa}"][(n - 1) % 3];
                    levels.push_str(&format!(
                        "<text:list-level-style-bullet text:level=\"{n}\" text:bullet-char=\"{ch}\">{indent}</text:list-level-style-bullet>"
                    ));
                }
            }
            auto.push(format!("<text:list-style style:name=\"{name}\">{levels}</text:list-style>"));
        }
        let ons = odt_ns();
        let content = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-content {ons} office:version=\"1.3\">\
<office:automatic-styles>{}</office:automatic-styles>\
<office:body><office:text>{body}</office:text></office:body></office:document-content>",
            auto.join("")
        );
        let mut manifest = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<manifest:manifest xmlns:manifest=\"{}\" manifest:version=\"1.3\">\
<manifest:file-entry manifest:full-path=\"/\" manifest:version=\"1.3\" manifest:media-type=\"application/vnd.oasis.opendocument.text\"/>\
<manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/>\
<manifest:file-entry manifest:full-path=\"styles.xml\" manifest:media-type=\"text/xml\"/>",
            ns("manifest")
        );
        for (path, mime, _) in &self.pictures {
            manifest.push_str(&format!("<manifest:file-entry manifest:full-path=\"{path}\" manifest:media-type=\"{mime}\"/>"));
        }
        manifest.push_str("</manifest:manifest>");
        let mut files: Vec<(String, Vec<u8>)> = vec![
            ("mimetype".into(), b"application/vnd.oasis.opendocument.text".to_vec()),
            ("content.xml".into(), content.into_bytes()),
            ("styles.xml".into(), styles_xml().into_bytes()),
            ("META-INF/manifest.xml".into(), manifest.into_bytes()),
        ];
        for (path, _, data) in self.pictures.drain(..) {
            files.push((path, data));
        }
        make_zip(&files, Some("mimetype"))
    }

    fn take_break(&mut self) -> bool {
        std::mem::replace(&mut self.pending_break, false)
    }

    fn block(&mut self, b: &Block, in_cell: bool) -> String {
        match b {
            Block::PageBreak => {
                self.pending_break = true;
                String::new()
            }
            Block::Para(p) => self.para(p, in_cell),
            Block::List(l) => self.list(l),
            Block::Table(t) => self.table(t),
        }
    }

    fn para(&mut self, p: &Para, in_cell: bool) -> String {
        let brk = self.take_break();
        if let Some(n) = p.style.strip_prefix('h').filter(|n| crate::text::is_digits(n)) {
            let style = self.para_style(&format!("Heading_20_{n}"), &p.align, brk);
            let runs = self.runs(&p.runs);
            return format!("<text:h text:style-name=\"{style}\" text:outline-level=\"{n}\">{runs}</text:h>");
        }
        let style = self.para_style(if in_cell { "Table_20_Contents" } else { "Standard" }, &p.align, brk);
        let runs = self.runs(&p.runs);
        format!("<text:p text:style-name=\"{style}\">{runs}</text:p>")
    }

    fn list(&mut self, lst: &ListBlock) -> String {
        let name = self.list_style(lst.ordered);
        let mut out = format!("<text:list text:style-name=\"{name}\">");
        for item in &lst.items {
            out.push_str("<text:list-item>");
            let inner: String = item
                .iter()
                .map(|b| match b {
                    Block::List(l) => self.list(l),
                    other => self.block(other, false),
                })
                .collect();
            out.push_str(if inner.is_empty() { "<text:p text:style-name=\"Standard\"/>" } else { &inner });
            out.push_str("</text:list-item>");
        }
        out.push_str("</text:list>");
        out
    }

    fn table(&mut self, t: &Table) -> String {
        self.tables += 1;
        let n = self.tables;
        let cols = t.rows.iter().map(|row| row.iter().map(|c| c.colspan).sum::<i64>()).max().unwrap_or(1);
        let mut out = format!(
            "<table:table table:name=\"Table{n}\" table:style-name=\"Table{n}\"><table:table-column table:number-columns-repeated=\"{cols}\"/>"
        );
        for row in &t.rows {
            out.push_str("<table:table-row>");
            let mut used = 0;
            for c in row {
                let span = if c.colspan > 1 { format!(" table:number-columns-spanned=\"{}\"", c.colspan) } else { String::new() };
                let mut inner: String = c.blocks.iter().map(|b| self.block(b, true)).collect();
                if inner.is_empty() {
                    inner = "<text:p text:style-name=\"Table_20_Contents\"/>".into();
                }
                out.push_str(&format!(
                    "<table:table-cell table:style-name=\"TableCell\" office:value-type=\"string\"{span}>{inner}</table:table-cell>"
                ));
                for _ in 0..(c.colspan - 1).max(0) {
                    out.push_str("<table:covered-table-cell/>");
                }
                used += c.colspan;
            }
            for _ in 0..(cols - used).max(0) {
                out.push_str("<table:table-cell table:style-name=\"TableCell\" office:value-type=\"string\"><text:p/></table:table-cell>");
            }
            out.push_str("</table:table-row>");
        }
        out.push_str("</table:table>");
        out
    }

    fn runs(&mut self, runs: &[Run]) -> String {
        let mut out = String::new();
        for r in runs {
            match r {
                Run::Break => out.push_str("<text:line-break/>"),
                Run::Image(im) => {
                    let s = self.image(im);
                    out.push_str(&s);
                }
                Run::Text(t) => {
                    let mut s = text(&t.text);
                    if let Some(style) = self.text_style(&t.fmt) {
                        s = format!("<text:span text:style-name=\"{style}\">{s}</text:span>");
                    }
                    if let Some(href) = t.fmt.href.as_deref().filter(|h| !h.is_empty()) {
                        s = format!("<text:a xlink:type=\"simple\" xlink:href=\"{}\" text:style-name=\"Internet_20_link\">{s}</text:a>", x(href));
                    }
                    out.push_str(&s);
                }
                Run::PageBreak => {}
            }
        }
        out
    }

    fn image(&mut self, im: &Image) -> String {
        let Some(ext) = ext_of(&im.mime) else { return String::new() };
        let n = self.pictures.len() + 1;
        let path = format!("Pictures/image{n}.{ext}");
        self.pictures.push((path.clone(), im.mime.clone(), strip_image_metadata(&im.data)));
        let size = image_size(&im.data);
        let mut wpx = if im.width != 0 { im.width } else if size.0 != 0 { size.0 } else { 200 };
        let mut hpx = if im.height != 0 { im.height } else if size.1 != 0 { size.1 } else { 150 };
        if wpx > 604 {
            hpx = ((hpx * 604) as f64 / wpx as f64) as i64;
            wpx = 604;
        }
        let alt = if im.alt.is_empty() { String::new() } else { format!("<svg:desc>{}</svg:desc>", x(&im.alt)) };
        format!(
            "<draw:frame draw:style-name=\"fr1\" draw:name=\"Image{n}\" text:anchor-type=\"as-char\" \
svg:width=\"{:.3}cm\" svg:height=\"{:.3}cm\" draw:z-index=\"0\">\
<draw:image xlink:href=\"{path}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/>{alt}</draw:frame>",
            wpx as f64 / 96.0 * 2.54,
            hpx as f64 / 96.0 * 2.54
        )
    }
}

/// Text with tabs and runs of spaces the way ODF writes them.
fn text(t: &str) -> String {
    let t = xml_safe(t);
    let mut out = String::new();
    // re.split(r'(\t| {2,})', t)
    let mut plain = String::new();
    let chars: Vec<char> = t.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\t' {
            out.push_str(&x(&std::mem::take(&mut plain)));
            out.push_str("<text:tab/>");
            i += 1;
        } else if chars[i] == ' ' && chars.get(i + 1) == Some(&' ') {
            let mut j = i;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            out.push_str(&x(&std::mem::take(&mut plain)));
            out.push_str(&format!(" <text:s text:c=\"{}\"/>", j - i - 1));
            i = j;
        } else {
            plain.push(chars[i]);
            i += 1;
        }
    }
    out.push_str(&x(&plain));
    if let Some(rest) = out.strip_prefix(' ') {
        return format!("<text:s/>{rest}");
    }
    out
}
