use crate::{Error, Result, reader::Reader};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Member {
    pub name: String,
    pub offset: usize,
    pub size: usize,
}

#[derive(Clone, Debug)]
pub struct Package {
    pub version: u32,
    bytes: Vec<u8>,
    members: Vec<Member>,
}
impl Package {
    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        let mut r = Reader::new(&bytes);
        let magic = r.sized_str(32)?;
        let version = magic
            .strip_prefix("PKGV")
            .filter(|s| s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| Error::unsupported(4, format!("package magic {magic:?}")))?;
        if !(1..=24).contains(&version) {
            return Err(Error::unsupported(
                4,
                format!("PKGV version {version}; supported index layout is versions1–24"),
            ));
        }
        let count = r.count(1_000_000, "package members")?;
        if count > r.remaining() / 12 {
            return Err(Error::parse(
                r.pos,
                "package count exceeds available index bytes",
            ));
        }
        let mut members = Vec::with_capacity(count);
        for _ in 0..count {
            let name = r.sized_str(1024 * 1024)?;
            if name.contains('\0') {
                return Err(Error::parse(r.pos, "NUL in package member name"));
            }
            let offset = r.u32()? as usize;
            let size = r.u32()? as usize;
            members.push(Member { name, offset, size });
        }
        let base = r.pos;
        for m in &mut members {
            m.offset = m
                .offset
                .checked_add(base)
                .ok_or_else(|| Error::parse(base, "package offset overflow"))?;
            let end = m
                .offset
                .checked_add(m.size)
                .ok_or_else(|| Error::parse(m.offset, "package size overflow"))?;
            if end > bytes.len() {
                return Err(Error::parse(m.offset, "package member extends beyond file"));
            }
        }
        Ok(Self {
            version,
            bytes,
            members,
        })
    }
    pub fn members(&self) -> &[Member] {
        &self.members
    }
    /// Exact member spelling. Empty files are returned; VFS priority is a caller policy.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.members
            .iter()
            .find(|m| m.name == name)
            .map(|m| &self.bytes[m.offset..m.offset + m.size])
    }
    /// Exact index row, allowing the VFS to implement folded names and duplicate priority.
    pub fn member_data(&self, index: usize) -> Option<&[u8]> {
        self.members
            .get(index)
            .map(|m| &self.bytes[m.offset..m.offset + m.size])
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"version":self.version,"file_bytes":self.bytes.len(),"member_count":self.members.len(),"members":self.members})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(8u32.to_le_bytes());
        b.extend(b"PKGV0001");
        b.extend(2u32.to_le_bytes());
        for (name, off, size) in [("a", 0u32, 3u32), ("empty", 3, 0)] {
            b.extend((name.len() as u32).to_le_bytes());
            b.extend(name.as_bytes());
            b.extend(off.to_le_bytes());
            b.extend(size.to_le_bytes());
        }
        b.extend(b"abc");
        b
    }
    #[test]
    fn members_and_empty_file() {
        let p = Package::parse(fixture()).unwrap();
        assert_eq!(p.get("a"), Some(&b"abc"[..]));
        assert_eq!(p.get("empty"), Some(&b""[..]));
        assert_eq!(p.get("missing"), None);
    }
    #[test]
    fn every_truncated_index_or_payload_is_error() {
        let b = fixture();
        for n in 0..b.len() {
            assert!(Package::parse(b[..n].to_vec()).is_err(), "prefix {n}");
        }
    }
}
