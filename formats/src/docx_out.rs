//! DOCX out: no author, dates or program name; pictures without metadata.

use crate::docx_in::{A, PIC, PR, R, W, WP};
use crate::images::{ext_of, image_size, strip_image_metadata};
use crate::model::*;
use crate::text::{escape as x, xml_safe};
use crate::zipio::make_zip;
use crate::EMU_PER_PX;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Default Extension="png" ContentType="image/png"/>
<Default Extension="jpeg" ContentType="image/jpeg"/>
<Default Extension="gif" ContentType="image/gif"/>
<Default Extension="bmp" ContentType="image/bmp"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
<Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>
</Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

pub fn heading_pt(n: u32) -> u32 {
    match n {
        1 => 20,
        2 => 16,
        3 => 14,
        4 => 12,
        _ => 11,
    }
}

fn styles() -> String {
    let heads: String = (1..=6)
        .map(|n| {
            format!(
                "<w:style w:type=\"paragraph\" w:styleId=\"Heading{n}\"><w:name w:val=\"heading {n}\"/>\
<w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:uiPriority w:val=\"9\"/><w:qFormat/>\
<w:pPr><w:keepNext/><w:spacing w:before=\"{}\" w:after=\"80\"/>\
<w:outlineLvl w:val=\"{}\"/></w:pPr>\
<w:rPr><w:b/><w:sz w:val=\"{}\"/></w:rPr></w:style>",
                if n < 3 { 240 } else { 160 },
                n - 1,
                heading_pt(n) * 2
            )
        })
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="{W}">
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Calibri"/>
<w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault>
<w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="264" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>
{heads}
<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/>
<w:pPr><w:spacing w:after="40"/><w:contextualSpacing/></w:pPr></w:style>
<w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph Font"/></w:style>
<w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style>
<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/>
<w:tblPr><w:tblCellMar><w:left w:w="108" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>
<w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/>
<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr>
<w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/>
<w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/>
<w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style>
</w:styles>"#
    )
}

fn numbering(lists: &[(usize, bool)]) -> String {
    let levels = |ordered: bool| -> String {
        (0..9)
            .map(|n| {
                let ind = 720 * (n + 1);
                if ordered {
                    let fmt = ["decimal", "lowerLetter", "lowerRoman"][n % 3];
                    format!(
                        "<w:lvl w:ilvl=\"{n}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{fmt}\"/>\
<w:lvlText w:val=\"%{}.\"/><w:lvlJc w:val=\"left\"/>\
<w:pPr><w:ind w:left=\"{ind}\" w:hanging=\"360\"/></w:pPr></w:lvl>",
                        n + 1
                    )
                } else {
                    let ch = ["\u{2022}", "\u{25e6}", "\u{25aa}"][n % 3];
                    format!(
                        "<w:lvl w:ilvl=\"{n}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/>\
<w:lvlText w:val=\"{ch}\"/><w:lvlJc w:val=\"left\"/>\
<w:pPr><w:ind w:left=\"{ind}\" w:hanging=\"360\"/></w:pPr></w:lvl>"
                    )
                }
            })
            .collect()
    };
    let abstracts = format!(
        "<w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"hybridMultilevel\"/>{}</w:abstractNum>\
<w:abstractNum w:abstractNumId=\"1\"><w:multiLevelType w:val=\"hybridMultilevel\"/>{}</w:abstractNum>",
        levels(false),
        levels(true)
    );
    let nums: String = lists
        .iter()
        .map(|&(nid, ordered)| {
            let over: String = if ordered {
                (0..9).map(|n| format!("<w:lvlOverride w:ilvl=\"{n}\"><w:startOverride w:val=\"1\"/></w:lvlOverride>")).collect()
            } else {
                String::new()
            };
            format!("<w:num w:numId=\"{nid}\"><w:abstractNumId w:val=\"{}\"/>{over}</w:num>", if ordered { 1 } else { 0 })
        })
        .collect();
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:numbering xmlns:w=\"{W}\">{abstracts}{nums}</w:numbering>")
}

#[derive(Default)]
struct Writer {
    rels: Vec<(String, &'static str, String, bool)>,
    media: Vec<(String, Vec<u8>)>,
    lists: Vec<(usize, bool)>,
    pic_id: usize,
}

pub fn write_docx(doc: &Document) -> Vec<u8> {
    Writer::default().write(doc)
}

impl Writer {
    fn rel(&mut self, typ: &'static str, target: &str, external: bool) -> String {
        let rid = format!("rId{}", self.rels.len() + 10);
        self.rels.push((rid.clone(), typ, target.to_string(), external));
        rid
    }

    fn write(mut self, doc: &Document) -> Vec<u8> {
        let mut body: String = doc.blocks.iter().map(|b| self.block(b, None)).collect();
        if body.is_empty() {
            body = "<w:p/>".into();
        }
        body.push_str(
            "<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/>\
<w:pgMar w:top=\"1417\" w:right=\"1417\" w:bottom=\"1134\" w:left=\"1417\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/>\
</w:sectPr>",
        );
        let document = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<w:document xmlns:w=\"{W}\" xmlns:r=\"{R}\" xmlns:wp=\"{WP}\" xmlns:a=\"{A}\" xmlns:pic=\"{PIC}\">\
<w:body>{body}</w:body></w:document>"
        );
        let mut all: Vec<(String, &str, String, bool)> =
            vec![("rId1".into(), "styles", "styles.xml".into(), false), ("rId2".into(), "numbering", "numbering.xml".into(), false)];
        all.extend(self.rels.iter().cloned());
        let rels: String = all
            .iter()
            .map(|(rid, typ, target, ext)| {
                format!(
                    "<Relationship Id=\"{rid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/{typ}\" Target=\"{}\"{}/>",
                    x(target),
                    if *ext { " TargetMode=\"External\"" } else { "" }
                )
            })
            .collect();
        let mut files: Vec<(String, Vec<u8>)> = vec![
            ("[Content_Types].xml".into(), CONTENT_TYPES.as_bytes().to_vec()),
            ("_rels/.rels".into(), ROOT_RELS.as_bytes().to_vec()),
            ("word/document.xml".into(), document.into_bytes()),
            ("word/styles.xml".into(), styles().into_bytes()),
            ("word/numbering.xml".into(), numbering(&self.lists).into_bytes()),
            (
                "word/_rels/document.xml.rels".into(),
                format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"{PR}\">{rels}</Relationships>").into_bytes(),
            ),
        ];
        for (name, data) in self.media.drain(..) {
            files.push((format!("word/media/{name}"), data));
        }
        make_zip(&files, None)
    }

    fn block(&mut self, b: &Block, numbering: Option<(usize, i64)>) -> String {
        match b {
            Block::Para(p) => self.para(p, numbering, 0),
            Block::List(l) => self.list(l, 0),
            Block::Table(t) => self.table(t),
            Block::PageBreak => "<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>".into(),
        }
    }

    fn list(&mut self, lst: &ListBlock, level: i64) -> String {
        let nid = self.lists.len() + 1;
        self.lists.push((nid, lst.ordered));
        let mut out = String::new();
        for item in &lst.items {
            let mut first = true;
            for b in item {
                match b {
                    Block::List(sub) => out.push_str(&self.list(sub, (level + 1).min(8))),
                    Block::Para(p) if first => {
                        out.push_str(&self.para(p, Some((nid, level)), 0));
                        first = false;
                    }
                    Block::Para(p) => out.push_str(&self.para(p, None, 720 * (level + 1))),
                    other => out.push_str(&self.block(other, None)),
                }
            }
            if first {
                out.push_str(&self.para(&Para::default(), Some((nid, level)), 0));
            }
        }
        out
    }

    fn para(&mut self, p: &Para, numbering: Option<(usize, i64)>, indent: i64) -> String {
        let mut ppr = String::new();
        let heading = p.style.strip_prefix('h').filter(|n| crate::text::is_digits(n));
        if let Some(n) = heading {
            ppr.push_str(&format!("<w:pStyle w:val=\"Heading{n}\"/>"));
        } else if numbering.is_some() {
            ppr.push_str("<w:pStyle w:val=\"ListParagraph\"/>");
        }
        if let Some((nid, lvl)) = numbering {
            ppr.push_str(&format!("<w:numPr><w:ilvl w:val=\"{lvl}\"/><w:numId w:val=\"{nid}\"/></w:numPr>"));
        }
        if indent != 0 {
            ppr.push_str(&format!("<w:ind w:left=\"{indent}\"/>"));
        }
        if !p.align.is_empty() {
            let a = if p.align == "justify" { "both" } else { p.align.as_str() };
            ppr.push_str(&format!("<w:jc w:val=\"{a}\"/>"));
        }
        let mut out = String::from("<w:p>");
        if !ppr.is_empty() {
            out.push_str(&format!("<w:pPr>{ppr}</w:pPr>"));
        }
        let runs = &p.runs;
        let mut i = 0;
        while i < runs.len() {
            if let Run::Text(t) = &runs[i] {
                if let Some(href) = t.fmt.href.as_deref().filter(|h| !h.is_empty()) {
                    let mut j = i;
                    while j < runs.len() && matches!(&runs[j], Run::Text(r) if r.fmt.href.as_deref() == Some(href)) {
                        j += 1;
                    }
                    let rid = self.rel("hyperlink", href, true);
                    let inner: String = runs[i..j].iter().map(|r| self.run(r)).collect();
                    out.push_str(&format!("<w:hyperlink r:id=\"{rid}\">{inner}</w:hyperlink>"));
                    i = j;
                    continue;
                }
            }
            let r = self.run(&runs[i]);
            out.push_str(&r);
            i += 1;
        }
        out.push_str("</w:p>");
        out
    }

    fn run(&mut self, r: &Run) -> String {
        let t = match r {
            Run::Break => return "<w:r><w:br/></w:r>".into(),
            Run::Image(im) => return self.image(im),
            Run::Text(t) => t,
            Run::PageBreak => return String::new(),
        };
        let f = &t.fmt;
        let mut rpr = String::new();
        if f.href.as_deref().is_some_and(|h| !h.is_empty()) {
            rpr.push_str("<w:rStyle w:val=\"Hyperlink\"/>");
        }
        if f.b {
            rpr.push_str("<w:b/><w:bCs/>");
        }
        if f.i {
            rpr.push_str("<w:i/><w:iCs/>");
        }
        if f.s {
            rpr.push_str("<w:strike/>");
        }
        if f.u {
            rpr.push_str("<w:u w:val=\"single\"/>");
        }
        if f.sup || f.sub {
            rpr.push_str(&format!("<w:vertAlign w:val=\"{}\"/>", if f.sup { "superscript" } else { "subscript" }));
        }
        let mut parts = String::new();
        for (k, chunk) in t.text.split('\t').enumerate() {
            if k > 0 {
                parts.push_str("<w:tab/>");
            }
            if !chunk.is_empty() {
                parts.push_str(&format!("<w:t xml:space=\"preserve\">{}</w:t>", x(&xml_safe(chunk))));
            }
        }
        let rpr = if rpr.is_empty() { String::new() } else { format!("<w:rPr>{rpr}</w:rPr>") };
        format!("<w:r>{rpr}{parts}</w:r>")
    }

    fn image(&mut self, im: &Image) -> String {
        let Some(ext) = ext_of(&im.mime) else { return String::new() };
        self.pic_id += 1;
        let name = format!("image{}.{ext}", self.pic_id);
        self.media.push((name.clone(), strip_image_metadata(&im.data)));
        let rid = self.rel("image", &format!("media/{name}"), false);
        let size = image_size(&im.data);
        let mut wpx = if im.width != 0 { im.width } else if size.0 != 0 { size.0 } else { 200 };
        let mut hpx = if im.height != 0 { im.height } else if size.1 != 0 { size.1 } else { 150 };
        if wpx > 624 {
            hpx = ((hpx * 624) as f64 / wpx as f64) as i64;
            wpx = 624;
        }
        let (cx, cy) = (wpx * EMU_PER_PX, hpx * EMU_PER_PX);
        let n = self.pic_id;
        format!(
            "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
<wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{n}\" name=\"Picture {n}\" descr=\"{}\"/>\
<a:graphic><a:graphicData uri=\"{PIC}\"><pic:pic><pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"{name}\"/>\
<pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch>\
</pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>\
</wp:inline></w:drawing></w:r>",
            x(&im.alt)
        )
    }

    fn table(&mut self, t: &Table) -> String {
        let cols = t.rows.iter().map(|row| row.iter().map(|c| c.colspan).sum::<i64>()).max().unwrap_or(1);
        let width = 9000i64.div_euclid(cols.max(1));
        let grid: String = (0..cols.max(0)).map(|_| format!("<w:gridCol w:w=\"{width}\"/>")).collect();
        let mut out = format!(
            "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/>\
<w:tblLook w:val=\"04A0\"/></w:tblPr><w:tblGrid>{grid}</w:tblGrid>"
        );
        for row in &t.rows {
            out.push_str("<w:tr>");
            for c in row {
                let mut tcpr = format!("<w:tcW w:w=\"{}\" w:type=\"dxa\"/>", width * c.colspan);
                if c.colspan > 1 {
                    tcpr.push_str(&format!("<w:gridSpan w:val=\"{}\"/>", c.colspan));
                }
                let mut inner: String = c.blocks.iter().map(|b| self.block(b, None)).collect();
                if !inner.trim_end().ends_with("</w:p>") {
                    inner.push_str("<w:p/>");
                }
                out.push_str(&format!("<w:tc><w:tcPr>{tcpr}</w:tcPr>{inner}</w:tc>"));
            }
            out.push_str("</w:tr>");
        }
        out.push_str("</w:tbl>");
        out
    }
}
