//! Deeply nested files: refused with an error (or flattened, for the editor's
//! HTML), never a stack overflow that takes the app down. Each case runs on a
//! thread with a 1 MiB stack, the size of a Windows main thread, when
//! optimised; unoptimised builds use far bigger frames, so they get 4 MiB.
//! (Optimised, a file at the limit needs under 256 KiB.)

use leaflet_formats::{self as lf, xml::TOO_DEEP, zipio::make_zip, MAX_DEPTH};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

fn on_small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().stack_size(if cfg!(debug_assertions) { 4 << 20 } else { 1 << 20 }).spawn(f).unwrap().join().expect("panicked or overflowed");
}

fn docx(body: &str) -> Vec<u8> {
    let xml = format!("<w:document xmlns:w=\"{W}\"><w:body>{body}</w:body></w:document>");
    make_zip(&[("word/document.xml".to_string(), xml.into_bytes())], None)
}

fn round_trip(doc: &lf::Document) {
    let html = lf::to_html(doc);
    let back = lf::from_html(&html);
    lf::save(&back, "docx").unwrap();
    lf::save(&back, "odt").unwrap();
}

#[test]
fn deeply_nested_docx_is_an_error() {
    on_small_stack(|| {
        let n = 5000;
        let body = format!("<w:p>{}<w:r><w:t>x</w:t></w:r>{}</w:p>", "<w:smartTag>".repeat(n), "</w:smartTag>".repeat(n));
        let data = docx(&body);
        assert!(data.len() < 4096, "the file stays tiny: {} bytes", data.len());
        assert_eq!(lf::load(&data, "docx").unwrap_err().0, TOO_DEEP);
    });
}

#[test]
fn deeply_nested_odt_is_an_error() {
    on_small_stack(|| {
        let n = 5000;
        let content = format!(
            "<office:document-content xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
             xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\"><office:body><office:text><text:p>{}x{}</text:p>\
             </office:text></office:body></office:document-content>",
            "<text:span>".repeat(n),
            "</text:span>".repeat(n)
        );
        let data = make_zip(
            &[
                ("mimetype".to_string(), b"application/vnd.oasis.opendocument.text".to_vec()),
                ("content.xml".to_string(), content.into_bytes()),
            ],
            Some("mimetype"),
        );
        assert_eq!(lf::load(&data, "odt").unwrap_err().0, TOO_DEEP);
    });
}

#[test]
fn docx_nested_up_to_the_limit_still_reads() {
    on_small_stack(|| {
        // document, body, p, then the smart tags, then r and t
        let n = MAX_DEPTH - 5;
        let body = format!("<w:p>{}<w:r><w:t>deep</w:t></w:r>{}</w:p>", "<w:smartTag>".repeat(n), "</w:smartTag>".repeat(n));
        let doc = lf::load(&docx(&body), "docx").unwrap();
        assert_eq!(lf::model::plain_text(&doc.blocks).trim(), "deep");
        round_trip(&doc);

        // tables inside tables, three levels each (tbl, tr, tc), as deep as allowed
        let n = (MAX_DEPTH - 2 - 3) / 3;
        let open = "<w:tbl><w:tr><w:tc>".repeat(n);
        let close = "</w:tc></w:tr></w:tbl>".repeat(n);
        let body = format!("{open}<w:p><w:r><w:t>cell</w:t></w:r></w:p>{close}");
        let doc = lf::load(&docx(&body), "docx").unwrap();
        assert!(lf::model::plain_text(&doc.blocks).contains("cell"));
        round_trip(&doc);

        // one level more is refused
        let n = MAX_DEPTH - 4;
        let body = format!("<w:p>{}<w:r><w:t>deep</w:t></w:r>{}</w:p>", "<w:smartTag>".repeat(n), "</w:smartTag>".repeat(n));
        assert_eq!(lf::load(&docx(&body), "docx").unwrap_err().0, TOO_DEEP);
    });
}

#[test]
fn deeply_nested_html_is_flattened() {
    on_small_stack(|| {
        // inline and block wrappers keep their text; lists and tables past the
        // limit may lose what ends up loose inside a <ul> or <tr>, but don't crash
        for (open, close, keeps_text) in [("<span>", "</span>", true), ("<b>", "</b>", true), ("<div>", "</div>", true),
            ("<ul><li>", "</li></ul>", false), ("<table><tr><td>", "</td></tr></table>", false)]
        {
            let html = format!("{}text{}", open.repeat(5000), close.repeat(5000));
            let doc = lf::from_html(&html);
            if keeps_text {
                assert!(lf::model::plain_text(&doc.blocks).contains("text"), "{open}: the text is lost");
            }
            round_trip(&doc);
        }
    });
}
