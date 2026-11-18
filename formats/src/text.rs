//! Small text helpers that behave the way the Python original's standard
//! library does (html.escape, int(), str.isdigit on ASCII, cp1252 with
//! errors='replace', base64 without validation...).

/// html.escape(text, quote=True)
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// Characters XML 1.0 can't hold: [\x00-\x08\x0b\x0c\x0e-\x1f￾￿]
pub fn xml_safe(text: &str) -> String {
    text.chars()
        .filter(|&c| {
            let n = c as u32;
            !(n <= 0x08 || n == 0x0b || n == 0x0c || (0x0e..=0x1f).contains(&n) || n == 0xfffe || n == 0xffff)
        })
        .collect()
}

/// Python's int() on a string: surrounding whitespace, a sign, digits with
/// single underscores between them.
pub fn py_int(s: &str) -> Option<i64> {
    let t = s.trim();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut n: i64 = 0;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(10)? as i64;
        n = n.saturating_mul(10).saturating_add(d);
    }
    Some(if neg { -n } else { n })
}

/// int(s, 16), which also takes a 0x prefix.
pub fn py_int16(s: &str) -> Option<i64> {
    let t = s.trim();
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let rest = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")).unwrap_or(rest);
    let rest = rest.strip_prefix('_').unwrap_or(rest);
    if rest.is_empty() || rest.ends_with('_') || rest.contains("__") {
        return None;
    }
    let mut n: i64 = 0;
    for c in rest.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(16)? as i64;
        n = n.saturating_mul(16).saturating_add(d);
    }
    Some(if neg { -n } else { n })
}

/// str.isdigit(), for the ASCII digits files use.
pub fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// float(s) the way Python reads one (no inf/nan needed here).
pub fn py_float(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let ok = t.bytes().all(|b| b.is_ascii_digit() || b == b'.' || b == b'-' || b == b'+' || b == b'e' || b == b'E');
    if !ok {
        return None;
    }
    t.parse::<f64>().ok()
}

// ------------------------------------------------------------ codepages

/// Python's cp1252: the five bytes it leaves undefined decode to U+FFFD.
const CP1252_HIGH: [u32; 32] = [
    0x20AC, 0xFFFD, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0xFFFD,
    0x017D, 0xFFFD, 0xFFFD, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A,
    0x0153, 0xFFFD, 0x017E, 0x0178,
];

pub fn cp1252(data: &[u8]) -> String {
    data.iter()
        .map(|&b| {
            if (0x80..0xA0).contains(&b) {
                char::from_u32(CP1252_HIGH[(b - 0x80) as usize]).unwrap_or('\u{FFFD}')
            } else {
                b as char
            }
        })
        .collect()
}

pub fn latin1(data: &[u8]) -> String {
    data.iter().map(|&b| b as char).collect()
}

const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

/// File names in a zip without the UTF-8 flag, as zipfile reads them.
pub fn cp437(data: &[u8]) -> String {
    let high: Vec<char> = CP437_HIGH.chars().collect();
    data.iter().map(|&b| if b < 0x80 { b as char } else { high[(b - 0x80) as usize] }).collect()
}

/// bytes.decode('cpNNN', errors='replace'). cp1252 is done exactly as Python
/// does it; other codepages through encoding_rs (the WHATWG tables, which
/// match Python's except for a few unassigned bytes).
pub fn decode_codepage(cp: u32, data: &[u8]) -> String {
    let label = match cp {
        1252 => return cp1252(data),
        874 => "windows-874",
        932 => "shift_jis",
        936 => "gbk",
        949 => "euc-kr",
        950 => "big5",
        866 => "ibm866",
        1250..=1258 => {
            let name = format!("windows-{cp}");
            return decode_label(&name, data).unwrap_or_else(|| cp1252(data));
        }
        10000 => "macintosh",
        20866 => "koi8-r",
        28591 => return latin1(data),
        65001 => return String::from_utf8_lossy(data).into_owned(),
        _ => return cp1252(data),
    };
    decode_label(label, data).unwrap_or_else(|| cp1252(data))
}

fn decode_label(label: &str, data: &[u8]) -> Option<String> {
    let enc = encoding_rs::Encoding::for_label(label.as_bytes())?;
    let (text, _) = enc.decode_without_bom_handling(data);
    Some(text.into_owned())
}

/// UTF-16 with a BOM, errors='replace'.
pub fn utf16_bom(data: &[u8]) -> String {
    let (le, body) = if data.starts_with(b"\xff\xfe") {
        (true, &data[2..])
    } else if data.starts_with(b"\xfe\xff") {
        (false, &data[2..])
    } else {
        (true, data)
    };
    utf16(body, le)
}

pub fn utf16(body: &[u8], le: bool) -> String {
    let units = body.chunks(2).filter(|c| c.len() == 2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) });
    let mut out: String = char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')).collect();
    if body.len() % 2 == 1 {
        out.push('\u{FFFD}');
    }
    out
}

// ------------------------------------------------------------ base64

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// base64.b64decode without validation: characters outside the alphabet are
/// skipped, padding has to be right.
pub fn b64decode(s: &str) -> Option<Vec<u8>> {
    let mut vals = Vec::with_capacity(s.len());
    let mut pad = 0usize;
    for b in s.bytes() {
        if b == b'=' {
            pad += 1;
            continue;
        }
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        if pad > 0 {
            // data after padding: binascii stops at complete padding; be strict instead
            return None;
        }
        vals.push(v);
    }
    let rem = vals.len() % 4;
    if rem == 1 || (rem != 0 && pad < 4 - rem) {
        return None;
    }
    let mut out = Vec::with_capacity(vals.len() * 3 / 4);
    for chunk in vals.chunks(4) {
        let mut n: u32 = 0;
        for (k, &v) in chunk.iter().enumerate() {
            n |= (v as u32) << (18 - 6 * k);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

// ------------------------------------------------------------ crc32

pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    });
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// Does `s` start with a scheme Leaflet lets links have: (?i)(https?|mailto):
pub fn is_web_link(s: &str) -> bool {
    let l = s.get(..7).unwrap_or(s).to_ascii_lowercase();
    l.starts_with("http:") || l.starts_with("https:") || l.starts_with("mailto:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        for n in 0..10 {
            let data: Vec<u8> = (0..n as u8).collect();
            assert_eq!(b64decode(&b64encode(&data)).unwrap(), data);
        }
        assert_eq!(b64decode("AAA"), None);
        assert_eq!(b64decode("QU\nJD"), Some(b"ABC".to_vec()));
    }

    #[test]
    fn ints() {
        assert_eq!(py_int(" 12 "), Some(12));
        assert_eq!(py_int("1_0"), Some(10));
        assert_eq!(py_int("x"), None);
        assert_eq!(py_int16("0xF020"), Some(0xF020));
    }

    #[test]
    fn crc() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn web_links() {
        assert!(is_web_link("HTTPS://x"));
        assert!(is_web_link("mailto:a"));
        assert!(!is_web_link("javascript:x"));
        assert!(!is_web_link("http"));
    }
}
