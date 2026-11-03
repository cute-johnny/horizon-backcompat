//! The few Minecraft wire types the pipeline reads itself: VarInt, strings, booleans, shorts.
use crate::{Error, Result};

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.buf.len())
            .ok_or_else(|| Error::new("packet ended early"))?;
        let bytes = &self.buf[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            b => Err(Error::new(format!("bad boolean {b}"))),
        }
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    /// VarInt: at most five bytes, seven bits each, least significant group first.
    pub fn varint(&mut self) -> Result<i32> {
        let mut value: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            value |= u32::from(b & 0x7F) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(Error::new("VarInt too big"))
    }

    /// A VarInt-prefixed UTF-8 string of at most `max` characters.
    pub fn string(&mut self, max: usize) -> Result<String> {
        let len = usize::try_from(self.varint()?).map_err(|_| Error::new("negative string length"))?;
        if len > max.saturating_mul(3) {
            return Err(Error::new(format!("string of {len} bytes is longer than {max} characters")));
        }
        let text = std::str::from_utf8(self.take(len)?).map_err(|_| Error::new("string is not UTF-8"))?;
        if text.chars().count() > max {
            return Err(Error::new(format!("string is longer than {max} characters")));
        }
        Ok(text.to_owned())
    }

    /// Everything not read yet.
    pub fn rest(&mut self) -> &'a [u8] {
        let rest = &self.buf[self.pos..];
        self.pos = self.buf.len();
        rest
    }
}

pub fn write_varint(out: &mut Vec<u8>, value: i32) {
    let mut v = value as u32;
    loop {
        if v & !0x7F == 0 {
            out.push(v as u8);
            return;
        }
        out.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
}

pub fn write_string(out: &mut Vec<u8>, text: &str) {
    write_varint(out, text.len() as i32);
    out.extend_from_slice(text.as_bytes());
}

pub fn varint(value: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    write_varint(&mut out, value);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip_at_their_boundaries() {
        for v in [0, 1, 127, 128, 255, 25565, 2_097_151, i32::MAX, -1, i32::MIN] {
            let bytes = varint(v);
            assert_eq!(Reader::new(&bytes).varint().unwrap(), v, "{v}");
        }
        assert_eq!(varint(300), [0xAC, 0x02]);
        assert_eq!(varint(-1), [0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    }

    #[test]
    fn a_six_byte_varint_is_refused() {
        assert!(Reader::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]).varint().is_err());
    }

    #[test]
    fn strings_are_length_prefixed_and_bounded() {
        let mut out = Vec::new();
        write_string(&mut out, "play.example.com");
        let mut r = Reader::new(&out);
        assert_eq!(r.string(255).unwrap(), "play.example.com");
        assert!(Reader::new(&out).string(4).is_err());
        assert!(Reader::new(&[5, b'a']).string(255).is_err());
    }
}
