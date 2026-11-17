//! Reader for the little-endian binary formats (`.rdo`, `.rdm`, `.hlm`, `.tlm`).

use crate::FormatError;
use crate::text::decode_cp1252;

/// Read cursor over a byte slice; every length is checked against the remaining length before reading.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn take(&mut self, len: usize, what: &str) -> Result<&'a [u8], FormatError> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| FormatError::Truncated {
                offset: self.pos,
                what: what.to_string(),
            })?;
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn skip(&mut self, len: usize, what: &str) -> Result<(), FormatError> {
        self.take(len, what).map(|_| ())
    }

    pub fn u32(&mut self, what: &str) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    pub fn i32(&mut self, what: &str) -> Result<i32, FormatError> {
        Ok(i32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    pub fn f32(&mut self, what: &str) -> Result<f32, FormatError> {
        Ok(f32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    pub fn vec3(&mut self, what: &str) -> Result<[f32; 3], FormatError> {
        Ok([self.f32(what)?, self.f32(what)?, self.f32(what)?])
    }

    pub fn vec4(&mut self, what: &str) -> Result<[f32; 4], FormatError> {
        Ok([
            self.f32(what)?,
            self.f32(what)?,
            self.f32(what)?,
            self.f32(what)?,
        ])
    }

    /// Count as `i32` in the range `0..=max`.
    pub fn count(&mut self, max: usize, what: &str) -> Result<usize, FormatError> {
        let offset = self.pos;
        let value = self.i32(what)?;
        usize::try_from(value)
            .ok()
            .filter(|&n| n <= max)
            .ok_or_else(|| {
                FormatError::Invalid(format!("{what} at offset {offset:#x} implausible: {value}"))
            })
    }

    /// NUL-terminated cp1252 text in a fixed-length field; the rest after the NUL is memory content of the
    /// exporter.
    pub fn fixed_str(&mut self, len: usize, what: &str) -> Result<String, FormatError> {
        let bytes = self.take(len, what)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(len);
        Ok(decode_cp1252(&bytes[..end]))
    }

    fn block(&mut self, count: usize, item: usize, what: &str) -> Result<&'a [u8], FormatError> {
        let len = count
            .checked_mul(item)
            .ok_or_else(|| FormatError::Truncated {
                offset: self.pos,
                what: what.to_string(),
            })?;
        self.take(len, what)
    }

    /// `count` groups of `N` `f32` each.
    pub fn floats<const N: usize>(
        &mut self,
        count: usize,
        what: &str,
    ) -> Result<Vec<[f32; N]>, FormatError> {
        let bytes = self.block(count, 4 * N, what)?;
        Ok(bytes
            .chunks_exact(4 * N)
            .map(|c| {
                std::array::from_fn(|k| f32::from_le_bytes(c[4 * k..4 * k + 4].try_into().unwrap()))
            })
            .collect())
    }

    /// `count` groups of `N` `i32` each.
    pub fn ints<const N: usize>(
        &mut self,
        count: usize,
        what: &str,
    ) -> Result<Vec<[i32; N]>, FormatError> {
        let bytes = self.block(count, 4 * N, what)?;
        Ok(bytes
            .chunks_exact(4 * N)
            .map(|c| {
                std::array::from_fn(|k| i32::from_le_bytes(c[4 * k..4 * k + 4].try_into().unwrap()))
            })
            .collect())
    }

    pub fn f32s(&mut self, count: usize, what: &str) -> Result<Vec<f32>, FormatError> {
        Ok(self
            .floats::<1>(count, what)?
            .into_iter()
            .map(|[v]| v)
            .collect())
    }

    pub fn i32s(&mut self, count: usize, what: &str) -> Result<Vec<i32>, FormatError> {
        Ok(self
            .ints::<1>(count, what)?
            .into_iter()
            .map(|[v]| v)
            .collect())
    }

    /// Requires that all bytes have been read.
    pub fn finish(&self) -> Result<(), FormatError> {
        if self.pos == self.data.len() {
            Ok(())
        } else {
            Err(FormatError::TrailingData {
                offset: self.pos,
                remaining: self.data.len() - self.pos,
            })
        }
    }
}

/// Index from the file that must be less than `len`.
pub(crate) fn index(value: i32, len: usize, what: &str) -> Result<u32, FormatError> {
    usize::try_from(value)
        .ok()
        .filter(|&v| v < len)
        .map(|v| v as u32)
        .ok_or_else(|| FormatError::Invalid(format!("{what} {value} outside 0..{len}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_checks_lengths_and_counts() {
        let data = [1, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, b'a', b'b', 0, b'x'];
        let mut r = Reader::new(&data);
        assert_eq!(r.count(5, "n").unwrap(), 1);
        assert!(matches!(r.count(5, "m"), Err(FormatError::Invalid(_))));
        assert_eq!(r.fixed_str(4, "name").unwrap(), "ab");
        assert!(r.finish().is_ok());
        let mut r = Reader::new(&data);
        assert!(matches!(
            r.floats::<3>(usize::MAX / 2, "huge"),
            Err(FormatError::Truncated { offset: 0, .. })
        ));
        assert!(matches!(
            r.finish(),
            Err(FormatError::TrailingData { remaining: 12, .. })
        ));
        assert!(index(3, 3, "i").is_err());
        assert_eq!(index(2, 3, "i").unwrap(), 2);
        assert!(index(-1, 3, "i").is_err());
    }
}
