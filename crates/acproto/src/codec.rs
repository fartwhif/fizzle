//! Low-level little-endian read/write helpers and shared geometry types.

use crate::strings;

/// Cursor over a byte slice for reading.
#[derive(Debug)]
pub struct Reader<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn u8(&mut self) -> u8 {
        let v = self.buf[self.pos];
        self.pos += 1;
        v
    }

    pub fn u16(&mut self) -> u16 {
        let v = u16::from_le_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        v
    }

    pub fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]);
        self.pos += 4;
        v
    }

    pub fn i32(&mut self) -> i32 {
        self.u32() as i32
    }

    pub fn i64(&mut self) -> i64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.buf[self.pos..self.pos + 8]);
        self.pos += 8;
        i64::from_le_bytes(b)
    }

    pub fn u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.buf[self.pos..self.pos + 8]);
        self.pos += 8;
        u64::from_le_bytes(b)
    }
    pub fn f32(&mut self) -> f32 {
        let v = f32::from_le_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]);
        self.pos += 4;
        v
    }

    pub fn f64(&mut self) -> f64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.buf[self.pos..self.pos + 8]);
        self.pos += 8;
        f64::from_le_bytes(b)
    }

    /// Read `n` raw bytes, advancing. Panics if truncated (callers guard with
    /// `remaining()`).
    pub fn raw(&mut self, n: usize) -> &'a [u8] {
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        s
    }

    /// Read `n` raw bytes into a fresh vec.
    pub fn raw_vec(&mut self, n: usize) -> Vec<u8> {
        self.raw(n).to_vec()
    }

    pub fn string16l(&mut self) -> String {
        let (s, new_pos) = strings::read_string16l(self.buf, self.pos)
            .unwrap_or_else(|| panic!("truncated string16l at {}", self.pos));
        self.pos = new_pos;
        s.to_string()
    }

    /// Align the cursor up to a multiple of 4.
    pub fn align4(&mut self) {
        while !self.pos.is_multiple_of(4) {
            self.pos += 1;
        }
    }
}

/// Growable little-endian writer.
#[derive(Debug, Default)]
pub struct Writer {
    pub out: Vec<u8>,
}

impl Writer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn raw(&mut self, b: &[u8]) {
        self.out.extend_from_slice(b);
    }

    pub fn zeros(&mut self, n: usize) {
        self.out.extend(std::iter::repeat_n(0u8, n));
    }

    pub fn string16l(&mut self, s: &str) {
        strings::push_string16l(&mut self.out, s);
    }

    pub fn string32l(&mut self, s: &str) {
        strings::push_string32l(&mut self.out, s);
    }

    /// Align the output up to a multiple of 4.
    pub fn align4(&mut self) {
        while !self.out.len().is_multiple_of(4) {
            self.out.push(0);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quat {
    pub w: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// A 32-byte world position: cell id, position, rotation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Position {
    pub cell: u32,
    pub pos: Vec3,
    pub rot: Quat,
}

impl Position {
    pub const SIZE: usize = 32;

    pub fn pack(&self, w: &mut Writer) {
        w.u32(self.cell);
        w.f32(self.pos.x);
        w.f32(self.pos.y);
        w.f32(self.pos.z);
        w.f32(self.rot.w);
        w.f32(self.rot.x);
        w.f32(self.rot.y);
        w.f32(self.rot.z);
    }

    pub fn unpack(r: &mut Reader) -> Self {
        let cell = r.u32();
        let pos = Vec3 { x: r.f32(), y: r.f32(), z: r.f32() };
        let rot = Quat { w: r.f32(), x: r.f32(), y: r.f32(), z: r.f32() };
        Self { cell, pos, rot }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_roundtrip() {
        let p = Position {
            cell: 0x8602_01AD,
            pos: Vec3 { x: 1.5, y: -2.25, z: 97.125 },
            rot: Quat { w: 1.0, x: 0.0, y: 0.0, z: 0.0 },
        };
        let mut w = Writer::new();
        p.pack(&mut w);
        assert_eq!(w.out.len(), Position::SIZE);
        let mut r = Reader::new(&w.out);
        assert_eq!(Position::unpack(&mut r), p);
    }

    #[test]
    fn reader_string16l() {
        let mut w = Writer::new();
        w.string16l("Gharu'ndim");
        let mut r = Reader::new(&w.out);
        assert_eq!(r.string16l(), "Gharu'ndim");
        assert_eq!(r.remaining(), 0);
    }
}
