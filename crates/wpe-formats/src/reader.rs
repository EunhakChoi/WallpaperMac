use crate::{Error, MAX_ALLOCATION, Result};

pub(crate) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub pos: usize,
    pub end: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            end: bytes.len(),
        }
    }
    pub fn remaining(&self) -> usize {
        self.end.saturating_sub(self.pos)
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| Error::parse(self.pos, "byte range overflow"))?;
        if end > self.end {
            return Err(Error::parse(
                self.pos,
                format!("need {n} bytes, {} remain", self.remaining()),
            ));
        }
        let bytes = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub fn zstr(&mut self, max: usize) -> Result<String> {
        let start = self.pos;
        let available = &self.bytes[start..self.end];
        let stop = available
            .iter()
            .take(max.saturating_add(1))
            .position(|b| *b == 0)
            .ok_or_else(|| Error::parse(start, "unterminated or over-limit UTF-8 string"))?;
        let s = std::str::from_utf8(&available[..stop])
            .map_err(|e| Error::parse(start + e.valid_up_to(), "invalid UTF-8 string"))?
            .to_owned();
        self.pos += stop + 1;
        Ok(s)
    }
    pub fn sized_str(&mut self, max: usize) -> Result<String> {
        let n = self.count(max, "string bytes")?;
        let start = self.pos;
        let s = std::str::from_utf8(self.take(n)?)
            .map_err(|e| Error::parse(start + e.valid_up_to(), "invalid UTF-8 string"))?;
        Ok(s.to_owned())
    }
    pub fn count(&mut self, max: usize, what: &str) -> Result<usize> {
        let start = self.pos;
        let n = self.u32()? as usize;
        if n > max {
            return Err(Error::limit(
                start,
                format!("{what} count {n} exceeds {max}"),
            ));
        }
        Ok(n)
    }
    pub fn blob(&mut self) -> Result<(usize, Vec<u8>)> {
        let n = self.count(MAX_ALLOCATION, "buffer bytes")?;
        let off = self.pos;
        Ok((off, self.take(n)?.to_vec()))
    }
}
