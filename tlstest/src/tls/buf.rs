//! The TLS byte grammar of RFC 8446 §3: fixed-width integers and length-prefixed vectors.
//!
//! [`Writer`] appends; [`Reader`] consumes and names the field it was reading when it ran
//! out of bytes, which is what turns "cannot decode" into
//! `server_hello.extensions[2].key_share.key_exchange: wanted 32 bytes, 12 left`.

use super::{TlsError, TlsResult};

/// Builds a byte string in the TLS grammar.
#[derive(Debug, Clone, Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    /// An empty writer.
    pub fn new() -> Writer {
        Writer { bytes: Vec::new() }
    }

    /// A writer that starts from bytes already built.
    pub fn from(bytes: Vec<u8>) -> Writer {
        Writer { bytes }
    }

    /// The bytes written so far.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Take the bytes.
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }

    /// How many bytes have been written.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// True when nothing has been written.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// `uint8`
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.bytes.push(v);
        self
    }

    /// `uint16`, big-endian like every integer in TLS.
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// `uint24`
    pub fn u24(&mut self, v: u32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_be_bytes()[1..]);
        self
    }

    /// `uint32`
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// `uint64`
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Raw bytes with no length prefix.
    pub fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(v);
        self
    }

    /// `opaque x<0..255>`: a one-byte length followed by the bytes.
    pub fn vec8(&mut self, v: &[u8]) -> &mut Self {
        self.bytes.push(v.len() as u8);
        self.bytes.extend_from_slice(v);
        self
    }

    /// `opaque x<0..2^16-1>`: a two-byte length followed by the bytes.
    pub fn vec16(&mut self, v: &[u8]) -> &mut Self {
        self.u16(v.len() as u16);
        self.bytes.extend_from_slice(v);
        self
    }

    /// `opaque x<0..2^24-1>`: a three-byte length followed by the bytes.
    pub fn vec24(&mut self, v: &[u8]) -> &mut Self {
        self.u24(v.len() as u32);
        self.bytes.extend_from_slice(v);
        self
    }

    /// Write a nested structure and prefix it with its own one-byte length.
    pub fn nest8(&mut self, f: impl FnOnce(&mut Writer)) -> &mut Self {
        let mut inner = Writer::new();
        f(&mut inner);
        self.vec8(inner.bytes())
    }

    /// Write a nested structure and prefix it with its own two-byte length.
    pub fn nest16(&mut self, f: impl FnOnce(&mut Writer)) -> &mut Self {
        let mut inner = Writer::new();
        f(&mut inner);
        self.vec16(inner.bytes())
    }

    /// Write a nested structure and prefix it with its own three-byte length.
    pub fn nest24(&mut self, f: impl FnOnce(&mut Writer)) -> &mut Self {
        let mut inner = Writer::new();
        f(&mut inner);
        self.vec24(inner.bytes())
    }

    /// Overwrite a two-byte length that was written earlier — the escape hatch the
    /// "extension length lies" tests need.
    pub fn patch_u16(&mut self, offset: usize, value: u16) {
        if offset + 2 <= self.bytes.len() {
            self.bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
        }
    }

    /// Overwrite a three-byte length that was written earlier.
    pub fn patch_u24(&mut self, offset: usize, value: u32) {
        if offset + 3 <= self.bytes.len() {
            self.bytes[offset..offset + 3].copy_from_slice(&value.to_be_bytes()[1..]);
        }
    }
}

/// Consumes a byte string in the TLS grammar, naming the field on failure.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    /// The path prefix every error message carries, e.g. `server_hello`.
    pub path: String,
}

impl<'a> Reader<'a> {
    /// A reader over `bytes`, whose errors are prefixed with `path`.
    pub fn new(bytes: &'a [u8], path: impl Into<String>) -> Reader<'a> {
        Reader {
            bytes,
            pos: 0,
            path: path.into(),
        }
    }

    /// How many bytes are left.
    pub fn left(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    /// True when every byte has been consumed.
    pub fn done(&self) -> bool {
        self.left() == 0
    }

    /// The current offset into the original byte string.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// The bytes not yet consumed.
    pub fn rest(&self) -> &'a [u8] {
        &self.bytes[self.pos.min(self.bytes.len())..]
    }

    fn fail<T>(&self, field: &str, want: usize) -> TlsResult<T> {
        Err(TlsError::Decode(format!(
            "{}.{field}: wanted {want} byte(s), {} left at offset {}",
            self.path,
            self.left(),
            self.pos
        )))
    }

    /// Take `n` bytes.
    pub fn take(&mut self, n: usize, field: &str) -> TlsResult<&'a [u8]> {
        if self.left() < n {
            return self.fail(field, n);
        }
        let out = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// `uint8`
    pub fn u8(&mut self, field: &str) -> TlsResult<u8> {
        Ok(self.take(1, field)?[0])
    }

    /// `uint16`
    pub fn u16(&mut self, field: &str) -> TlsResult<u16> {
        let b = self.take(2, field)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    /// `uint24`
    pub fn u24(&mut self, field: &str) -> TlsResult<u32> {
        let b = self.take(3, field)?;
        Ok(u32::from_be_bytes([0, b[0], b[1], b[2]]))
    }

    /// `uint32`
    pub fn u32(&mut self, field: &str) -> TlsResult<u32> {
        let b = self.take(4, field)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A one-byte-length-prefixed vector.
    pub fn vec8(&mut self, field: &str) -> TlsResult<&'a [u8]> {
        let n = self.u8(&format!("{field}.length"))? as usize;
        self.take(n, field)
    }

    /// A two-byte-length-prefixed vector.
    pub fn vec16(&mut self, field: &str) -> TlsResult<&'a [u8]> {
        let n = self.u16(&format!("{field}.length"))? as usize;
        self.take(n, field)
    }

    /// A three-byte-length-prefixed vector.
    pub fn vec24(&mut self, field: &str) -> TlsResult<&'a [u8]> {
        let n = self.u24(&format!("{field}.length"))? as usize;
        self.take(n, field)
    }

    /// A sub-reader over a two-byte-length-prefixed vector.
    pub fn sub16(&mut self, field: &str) -> TlsResult<Reader<'a>> {
        let bytes = self.vec16(field)?;
        Ok(Reader::new(bytes, format!("{}.{field}", self.path)))
    }

    /// A sub-reader over a one-byte-length-prefixed vector.
    pub fn sub8(&mut self, field: &str) -> TlsResult<Reader<'a>> {
        let bytes = self.vec8(field)?;
        Ok(Reader::new(bytes, format!("{}.{field}", self.path)))
    }

    /// Fail unless everything has been consumed.
    pub fn expect_done(&self, field: &str) -> TlsResult<()> {
        if self.done() {
            return Ok(());
        }
        Err(TlsError::Decode(format!(
            "{}.{field}: {} byte(s) left over after the structure ended",
            self.path,
            self.left()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_big_endian() {
        let mut w = Writer::new();
        w.u8(1).u16(0x0304).u24(0x010203).u32(0xdeadbeef);
        assert_eq!(
            w.bytes(),
            &[1, 0x03, 0x04, 0x01, 0x02, 0x03, 0xde, 0xad, 0xbe, 0xef]
        );
        let mut r = Reader::new(w.bytes(), "t");
        assert_eq!(r.u8("a").expect("u8"), 1);
        assert_eq!(r.u16("b").expect("u16"), 0x0304);
        assert_eq!(r.u24("c").expect("u24"), 0x010203);
        assert_eq!(r.u32("d").expect("u32"), 0xdeadbeef);
        assert!(r.done());
    }

    #[test]
    fn nested_vectors_carry_their_own_length() {
        let mut w = Writer::new();
        w.nest16(|inner| {
            inner.nest8(|i2| {
                i2.raw(b"hi");
            });
        });
        assert_eq!(w.bytes(), &[0, 3, 2, b'h', b'i']);
        let mut r = Reader::new(w.bytes(), "t");
        let mut sub = r.sub16("outer").expect("sub16");
        assert_eq!(sub.vec8("inner").expect("vec8"), b"hi");
        assert!(sub.done() && r.done());
    }

    #[test]
    fn a_short_read_names_the_field_and_the_offset() {
        let mut r = Reader::new(&[0, 1], "server_hello");
        let err = r.u32("random").expect_err("must fail");
        let msg = err.to_string();
        assert!(msg.contains("server_hello.random"), "{msg}");
        assert!(msg.contains("wanted 4"), "{msg}");
        assert!(msg.contains("2 left"), "{msg}");
    }

    #[test]
    fn leftover_bytes_are_an_error_when_the_caller_says_so() {
        let r = Reader::new(&[1, 2, 3], "x");
        assert!(r.expect_done("body").is_err());
    }

    #[test]
    fn a_length_can_be_patched_after_the_fact() {
        let mut w = Writer::new();
        w.u16(0).raw(b"abcd");
        w.patch_u16(0, 4);
        assert_eq!(w.bytes(), &[0, 4, b'a', b'b', b'c', b'd']);
        // And the lie the malformed-extension tests need.
        w.patch_u16(0, 400);
        assert_eq!(&w.bytes()[..2], &[1, 0x90]);
    }
}
