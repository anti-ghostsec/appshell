//! Picture sniffing, sizes, and writing pictures without their metadata.
//!
//! A photo carries where and when it was taken, the camera, sometimes a
//! thumbnail of the uncropped picture. Pictures are written into documents
//! without any of that; the pixel data is copied as it is, never re-encoded.
//! What stays is what the picture needs to look the same: its colour profile,
//! JFIF and Adobe colour markers, and the EXIF Orientation on its own.

use crate::text::crc32;

pub fn sniff_image(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

pub fn ext_of(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpeg"),
        "image/gif" => Some("gif"),
        "image/bmp" => Some("bmp"),
        _ => None,
    }
}

fn be(d: &[u8], a: usize, b: usize) -> i64 {
    d.get(a..b.min(d.len())).map(|s| s.iter().fold(0i64, |n, &x| (n << 8) | x as i64)).unwrap_or(0)
}
fn le(d: &[u8], a: usize, b: usize) -> i64 {
    d.get(a..b.min(d.len())).map(|s| s.iter().rev().fold(0i64, |n, &x| (n << 8) | x as i64)).unwrap_or(0)
}
fn le_signed32(d: &[u8], a: usize) -> i64 {
    let s = match d.get(a..(a + 4).min(d.len())) {
        Some(s) => s,
        None => return 0,
    };
    let v = le(d, a, a + 4);
    if s.is_empty() {
        return 0;
    }
    let bits = 8 * s.len() as u32;
    if v & (1 << (bits - 1)) != 0 {
        v - (1i64 << bits)
    } else {
        v
    }
}

/// Pixel size of a PNG/GIF/JPEG/BMP, or (0, 0).
pub fn image_size(data: &[u8]) -> (i64, i64) {
    if data.starts_with(b"\x89PNG") {
        return (be(data, 16, 20), be(data, 20, 24));
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return (le(data, 6, 8), le(data, 8, 10));
    }
    if data.starts_with(b"BM") {
        return (le_signed32(data, 18), le_signed32(data, 22).abs());
    }
    if data.starts_with(b"\xff\xd8") {
        let mut i = 2usize;
        while i + 9 < data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            if matches!(marker, 0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE | 0xCF) {
                return (be(data, i + 7, i + 9), be(data, i + 5, i + 7));
            }
            i += 2 + be(data, i + 2, i + 4) as usize;
        }
    }
    (0, 0)
}

/// The same picture without EXIF, XMP, IPTC, comments, text or dates. A file
/// that can't be followed to its end is returned as it was.
pub fn strip_image_metadata(data: &[u8]) -> Vec<u8> {
    let r = if data.starts_with(b"\xff\xd8") {
        strip_jpeg(data)
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        strip_png(data)
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        strip_gif(data)
    } else {
        None
    };
    r.unwrap_or_else(|| data.to_vec())
}

/// The Orientation (1-8) in an EXIF block's TIFF data; 1 if there's none.
fn exif_orientation(tiff: &[u8]) -> u8 {
    let big = match tiff.get(..2) {
        Some(b"II") => false,
        Some(b"MM") => true,
        _ => return 1,
    };
    let num = |a: usize, b: usize| if big { be(tiff, a, b) } else { le(tiff, a, b) };
    if num(2, 4) != 42 {
        return 1;
    }
    let ifd = num(4, 8) as usize;
    let count = if ifd + 2 <= tiff.len() { num(ifd, ifd + 2) } else { 0 };
    for n in 0..count.min(512) as usize {
        let a = ifd + 2 + 12 * n;
        if a + 12 > tiff.len() {
            continue;
        }
        if num(a, a + 2) == 0x0112 {
            let value = num(a + 8, a + 10);
            if num(a + 2, a + 4) == 3 && num(a + 4, a + 8) == 1 && (1..=8).contains(&value) {
                return value as u8;
            }
            break;
        }
    }
    1
}

fn orientation_exif(o: u8) -> Vec<u8> {
    let mut v = b"MM\x00\x2a\x00\x00\x00\x08\x00\x01\x01\x12\x00\x03\x00\x00\x00\x01".to_vec();
    v.extend_from_slice(&[0, o, 0, 0]);
    v.extend_from_slice(&[0, 0, 0, 0]);
    v
}

fn find_byte(d: &[u8], b: u8, from: usize) -> Option<usize> {
    d.get(from..)?.iter().position(|&x| x == b).map(|p| p + from)
}

fn strip_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    let mut kept: Vec<&[u8]> = Vec::new();
    let mut orientation = 1u8;
    let n = data.len();
    let mut i = 2usize;
    loop {
        if i + 1 >= n || data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0xD9 {
            kept.push(b"\xff\xd9");
            break;
        }
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            kept.push(&data[i..i + 2]);
            i += 2;
            continue;
        }
        let size = be(data, i + 2, i + 4) as usize;
        if i + 4 > n || size < 2 || i + 2 + size > n {
            return None;
        }
        let body = &data[i + 4..i + 2 + size];
        let mut end = i + 2 + size;
        if marker == 0xDA {
            loop {
                match find_byte(data, 0xFF, end) {
                    Some(e) if e + 1 < n => {
                        end = e;
                        let nx = data[end + 1];
                        if nx == 0x00 || (0xD0..=0xD7).contains(&nx) {
                            end += 2;
                        } else if nx == 0xFF {
                            end += 1;
                        } else {
                            break;
                        }
                    }
                    _ => {
                        end = n;
                        break;
                    }
                }
            }
            kept.push(&data[i..end]);
            if end == n {
                break;
            }
        } else if marker == 0xE1 && body.starts_with(b"Exif\x00\x00") {
            orientation = exif_orientation(&body[6..]);
        } else if marker == 0xFE || (0xE0..=0xEF).contains(&marker) {
            if (marker == 0xE0 && body.starts_with(b"JFIF\x00"))
                || (marker == 0xE2 && body.starts_with(b"ICC_PROFILE\x00"))
                || (marker == 0xEE && body.starts_with(b"Adobe"))
            {
                kept.push(&data[i..end]);
            }
        } else {
            kept.push(&data[i..end]);
        }
        i = end;
    }
    let mut block = Vec::new();
    if orientation != 1 {
        let mut exif = b"Exif\x00\x00".to_vec();
        exif.extend(orientation_exif(orientation));
        block.extend_from_slice(b"\xff\xe1");
        block.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        block.extend(exif);
    }
    let mut out = b"\xff\xd8".to_vec();
    let at = if !kept.is_empty() && kept[0].starts_with(b"\xff\xe0") { 1 } else { 0 };
    for (k, seg) in kept.iter().enumerate() {
        if k == at && !block.is_empty() {
            out.extend_from_slice(&block);
        }
        out.extend_from_slice(seg);
    }
    if at >= kept.len() && !block.is_empty() {
        out.extend_from_slice(&block);
    }
    Some(out)
}

const PNG_KEEP: [&[u8; 4]; 20] = [
    b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS", b"gAMA", b"cHRM", b"sRGB", b"iCCP", b"sBIT", b"bKGD", b"pHYs", b"hIST",
    b"sPLT", b"cICP", b"mDCv", b"cLLi", b"acTL", b"fcTL", b"fdAT",
];

fn png_chunk(kind: &[u8], body: &[u8]) -> Vec<u8> {
    let mut v = (body.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    let mut kb = kind.to_vec();
    kb.extend_from_slice(body);
    v.extend_from_slice(&crc32(&kb).to_be_bytes());
    v
}

fn strip_png(data: &[u8]) -> Option<Vec<u8>> {
    let mut chunks: Vec<(&[u8], &[u8])> = Vec::new();
    let mut orientation = 1u8;
    let n = data.len();
    let mut i = 8usize;
    loop {
        if i + 12 > n {
            return None;
        }
        let size = be(data, i, i + 4) as usize;
        let kind = &data[i + 4..i + 8];
        let end = i + 12 + size;
        if end > n {
            return None;
        }
        if kind == b"eXIf" {
            orientation = exif_orientation(&data[i + 8..i + 8 + size]);
        } else if PNG_KEEP.iter().any(|k| &k[..] == kind) {
            chunks.push((kind, &data[i..end]));
        }
        i = end;
        if kind == b"IEND" {
            break;
        }
    }
    let mut out = data[..8].to_vec();
    for (kind, raw) in chunks {
        if kind == b"IDAT" && orientation != 1 {
            out.extend(png_chunk(b"eXIf", &orientation_exif(orientation)));
            orientation = 1;
        }
        out.extend_from_slice(raw);
    }
    Some(out)
}

fn strip_gif(data: &[u8]) -> Option<Vec<u8>> {
    let table = |flags: u8| if flags & 0x80 != 0 { 3usize << ((flags & 7) + 1) } else { 0 };
    let blocks = |mut at: usize| -> Option<usize> {
        loop {
            let len = *data.get(at)? as usize;
            if len == 0 {
                return Some(at + 1);
            }
            at += 1 + len;
        }
    };
    let mut i = 13 + table(*data.get(10)?);
    let mut out = data.get(..i)?.to_vec();
    loop {
        let kind = *data.get(i)?;
        let end;
        if kind == 0x3B {
            out.push(b';');
            break;
        } else if kind == 0x2C {
            end = blocks(i + 10 + table(*data.get(i + 9)?) + 1)?;
            out.extend_from_slice(data.get(i..end)?);
        } else if kind == 0x21 {
            let label = *data.get(i + 1)?;
            end = blocks(i + 2)?;
            let app = data.get(i + 3..(i + 14).min(data.len())).unwrap_or(b"");
            if label == 0xF9 || label == 0x01 || (label == 0xFF && (app == b"NETSCAPE2.0" || app == b"ANIMEXTS1.0" || app == b"ICCRGBG1012")) {
                out.extend_from_slice(data.get(i..end)?);
            }
        } else {
            return None;
        }
        if end > data.len() {
            return None;
        }
        i = end;
    }
    Some(out)
}
