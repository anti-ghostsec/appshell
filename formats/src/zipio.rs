//! Zip reading with Leaflet's limits, and writing with fixed timestamps.
//! Reads what Python's zipfile reads for documents: stored and deflated
//! members, zip64 sizes, names in UTF-8 or cp437, the last entry winning
//! when a name repeats.

use crate::text::{cp437, crc32};
use crate::{FormatError, MAX_MEMBER, MAX_TOTAL};
use std::collections::HashMap;

struct Entry {
    name: String,
    method: u16,
    flags: u16,
    crc: u32,
    csize: u64,
    usize_: u64,
    offset: u64,
}

pub struct Zip<'a> {
    data: &'a [u8],
    entries: Vec<Entry>,
    by_name: HashMap<String, usize>,
    unpacked: u64,
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn u64_at(d: &[u8], at: usize) -> Option<u64> {
    d.get(at..at + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
}

const BAD: &str = "not a valid zip-based document";

impl<'a> Zip<'a> {
    /// The central directory, without the size checks (zipfile.ZipFile(...)).
    pub fn parse(data: &'a [u8]) -> Result<Zip<'a>, FormatError> {
        let bad = || FormatError::new(BAD);
        // End of central directory: the last one in the final 64 KB + 22 bytes.
        if data.len() < 22 {
            return Err(bad());
        }
        let start = data.len().saturating_sub(22 + 65535);
        let mut eocd = None;
        let mut i = data.len() - 22;
        loop {
            if &data[i..i + 4] == b"PK\x05\x06" {
                let comment = u16_at(data, i + 20).ok_or_else(bad)? as usize;
                if i + 22 + comment <= data.len() {
                    eocd = Some(i);
                    break;
                }
            }
            if i == start {
                break;
            }
            i -= 1;
        }
        let e = eocd.ok_or_else(bad)?;
        let mut count = u16_at(data, e + 10).ok_or_else(bad)? as u64;
        let mut cd_size = u32_at(data, e + 12).ok_or_else(bad)? as u64;
        let mut cd_off = u32_at(data, e + 16).ok_or_else(bad)? as u64;
        // zip64 end of central directory, through its locator
        if e >= 20 && &data[e - 20..e - 16] == b"PK\x06\x07" {
            let z64 = u64_at(data, e - 12).ok_or_else(bad)? as usize;
            if data.get(z64..z64 + 4) == Some(b"PK\x06\x06") {
                count = u64_at(data, z64 + 32).ok_or_else(bad)?;
                cd_size = u64_at(data, z64 + 40).ok_or_else(bad)?;
                cd_off = u64_at(data, z64 + 48).ok_or_else(bad)?;
            }
        }
        // Like zipfile, allow for data in front of the archive.
        let cd_end_expected = cd_off.checked_add(cd_size).ok_or_else(bad)?;
        let concat = (e as u64).saturating_sub(cd_end_expected);
        let mut at = (cd_off + concat) as usize;
        let mut entries = Vec::new();
        let mut by_name = HashMap::new();
        let mut n = 0u64;
        while n < count {
            if data.get(at..at + 4) != Some(b"PK\x01\x02") {
                return Err(bad());
            }
            let flags = u16_at(data, at + 8).ok_or_else(bad)?;
            let method = u16_at(data, at + 10).ok_or_else(bad)?;
            let crc = u32_at(data, at + 16).ok_or_else(bad)?;
            let mut csize = u32_at(data, at + 20).ok_or_else(bad)? as u64;
            let mut usize_ = u32_at(data, at + 24).ok_or_else(bad)? as u64;
            let name_len = u16_at(data, at + 28).ok_or_else(bad)? as usize;
            let extra_len = u16_at(data, at + 30).ok_or_else(bad)? as usize;
            let comment_len = u16_at(data, at + 32).ok_or_else(bad)? as usize;
            let mut offset = u32_at(data, at + 42).ok_or_else(bad)? as u64;
            let raw_name = data.get(at + 46..at + 46 + name_len).ok_or_else(bad)?;
            let extra = data.get(at + 46 + name_len..at + 46 + name_len + extra_len).ok_or_else(bad)?;
            // zip64 extra field: the values that were 0xFFFFFFFF, in order
            let mut x = 0;
            while x + 4 <= extra.len() {
                let id = u16::from_le_bytes([extra[x], extra[x + 1]]);
                let len = u16::from_le_bytes([extra[x + 2], extra[x + 3]]) as usize;
                let body = extra.get(x + 4..x + 4 + len).ok_or_else(bad)?;
                if id == 1 {
                    let mut k = 0;
                    if usize_ == 0xFFFF_FFFF {
                        usize_ = u64_at(body, k).ok_or_else(bad)?;
                        k += 8;
                    }
                    if csize == 0xFFFF_FFFF {
                        csize = u64_at(body, k).ok_or_else(bad)?;
                        k += 8;
                    }
                    if offset == 0xFFFF_FFFF {
                        offset = u64_at(body, k).ok_or_else(bad)?;
                    }
                }
                x += 4 + len;
            }
            let name = if flags & 0x800 != 0 {
                String::from_utf8_lossy(raw_name).into_owned()
            } else {
                cp437(raw_name)
            };
            by_name.insert(name.clone(), entries.len());
            entries.push(Entry { name, method, flags, crc, csize, usize_, offset: offset + concat });
            at += 46 + name_len + extra_len + comment_len;
            n += 1;
        }
        Ok(Zip { data, entries, by_name, unpacked: 0 })
    }

    /// open_zip(): the archive, if what it says about its sizes is within the limits.
    pub fn open(data: &'a [u8]) -> Result<Zip<'a>, FormatError> {
        let z = Zip::parse(data)?;
        let mut total: u64 = 0;
        for e in &z.entries {
            if e.usize_ > MAX_MEMBER as u64 {
                return Err(FormatError::new(format!("{} is too large", e.name)));
            }
            total += e.usize_;
        }
        if total > MAX_TOTAL as u64 {
            return Err(FormatError::new("the document unpacks to more than 256 MB"));
        }
        Ok(z)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    pub fn has(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// read_member(): counts what is really unpacked, whatever the headers say.
    pub fn read(&mut self, name: &str, required: bool) -> Result<Option<Vec<u8>>, FormatError> {
        let Some(&idx) = self.by_name.get(name) else {
            if required {
                return Err(FormatError::new(format!("{name} is missing")));
            }
            return Ok(None);
        };
        let room = (MAX_MEMBER as u64).min((MAX_TOTAL as u64).saturating_sub(self.unpacked));
        let data = self.unpack(idx, room + 1)?;
        self.unpacked += data.len() as u64;
        if data.len() as u64 > MAX_MEMBER as u64 {
            return Err(FormatError::new(format!("{name} is too large")));
        }
        if data.len() as u64 > room {
            return Err(FormatError::new("the document unpacks to more than 256 MB"));
        }
        Ok(Some(data))
    }

    fn unpack(&self, idx: usize, limit: u64) -> Result<Vec<u8>, FormatError> {
        let e = &self.entries[idx];
        let bad = || FormatError::new(format!("{} can't be read: the zip is damaged", e.name));
        if e.flags & 1 != 0 {
            return Err(FormatError::new(format!("{} is encrypted", e.name)));
        }
        let at = e.offset as usize;
        if self.data.get(at..at + 4) != Some(b"PK\x03\x04") {
            return Err(bad());
        }
        let name_len = u16_at(self.data, at + 26).ok_or_else(bad)? as usize;
        let extra_len = u16_at(self.data, at + 28).ok_or_else(bad)? as usize;
        let start = at + 30 + name_len + extra_len;
        let end = start.checked_add(e.csize as usize).ok_or_else(bad)?;
        let raw = self.data.get(start..end).ok_or_else(bad)?;
        let out = match e.method {
            0 => raw[..raw.len().min(limit as usize)].to_vec(),
            8 => inflate_limited(raw, limit as usize).map_err(|_| bad())?,
            _ => return Err(FormatError::new(format!("{} is packed in a way Leaflet can't unpack", e.name))),
        };
        // Like zipfile, check the CRC once the whole member was read.
        if (out.len() as u64) < limit && crc32(&out) != e.crc {
            return Err(FormatError::new(format!("{} is damaged (bad CRC)", e.name)));
        }
        Ok(out)
    }
}

fn inflate_limited(raw: &[u8], limit: usize) -> Result<Vec<u8>, ()> {
    use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
    use miniz_oxide::inflate::TINFLStatus;
    let mut state = Box::<DecompressorOxide>::default();
    let mut out = vec![0u8; limit.min(1 << 16).max(1)];
    let mut in_pos = 0;
    let mut out_pos = 0;
    loop {
        let flags = inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF
            | if in_pos < raw.len() { inflate_flags::TINFL_FLAG_HAS_MORE_INPUT } else { 0 };
        let (status, read, written) = decompress(&mut state, &raw[in_pos..], &mut out, out_pos, flags);
        in_pos += read;
        out_pos += written;
        match status {
            TINFLStatus::Done => {
                out.truncate(out_pos);
                return Ok(out);
            }
            TINFLStatus::HasMoreOutput => {
                if out_pos >= limit {
                    out.truncate(limit);
                    return Ok(out);
                }
                let grow = (out.len() * 2).min(limit).max(out.len() + 1);
                out.resize(grow, 0);
            }
            TINFLStatus::NeedsMoreInput => {
                // input ended early: what came out is all there is (zipfile would fail on the CRC)
                out.truncate(out_pos);
                return Err(());
            }
            _ => return Err(()),
        }
    }
}

/// make_zip(): every entry dated 1980-01-01, mode 0644, deflated except `stored_first`.
pub fn make_zip(files: &[(String, Vec<u8>)], stored: Option<&str>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let date: u16 = (0 << 9) | (1 << 5) | 1; // 1980-01-01
    let time: u16 = 0;
    for (name, data) in files {
        let crc = crc32(data);
        let deflate = Some(name.as_str()) != stored;
        let body = if deflate { miniz_oxide::deflate::compress_to_vec(data, 6) } else { data.clone() };
        let method: u16 = if deflate { 8 } else { 0 };
        let flags: u16 = if name.is_ascii() { 0 } else { 0x800 };
        let offset = out.len() as u32;
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&time.to_le_bytes());
        out.extend_from_slice(&date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&body);

        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&(0x0300u16 | 20).to_le_bytes()); // made by unix, 2.0
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&flags.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&time.to_le_bytes());
        central.extend_from_slice(&date.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(body.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&((0o100644u32) << 16).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_off = out.len() as u32;
    let cd_len = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let files = vec![
            ("mimetype".to_string(), b"application/vnd.oasis.opendocument.text".to_vec()),
            ("content.xml".to_string(), b"<a>hello hello hello hello</a>".repeat(50)),
        ];
        let z = make_zip(&files, Some("mimetype"));
        let mut r = Zip::open(&z).unwrap();
        assert_eq!(r.names().collect::<Vec<_>>(), vec!["mimetype", "content.xml"]);
        assert_eq!(r.read("content.xml", true).unwrap().unwrap(), files[1].1);
        assert_eq!(r.read("mimetype", true).unwrap().unwrap(), files[0].1);
        assert!(r.read("nope", false).unwrap().is_none());
        assert!(r.read("nope", true).is_err());
    }

    #[test]
    fn bomb_is_refused() {
        let big = vec![0u8; 70 * 1024 * 1024];
        let z = make_zip(&[("word/document.xml".to_string(), big)], None);
        assert!(Zip::open(&z).is_err());
    }
}
