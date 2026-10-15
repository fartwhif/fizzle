//! ACE packet crypto: Hash32 checksum, ISAAC PRNG, and the CryptoSystem key
//! stream. Byte-exact against `ACE.Common.Cryptography` @ 47edade.

use std::collections::HashSet;

/// The 32-bit checksum used for packet headers and payloads.
/// `h = len<<16 + each LE u32 word + tail bytes shifted 24/16/8/0`.
#[must_use]
pub fn hash32(data: &[u8]) -> u32 {
    let mut checksum = (data.len() as u32) << 16;
    let full = data.len() - data.len() % 4;
    for i in (0..full).step_by(4) {
        checksum = checksum.wrapping_add(u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]));
    }
    let mut shift = 3;
    for &b in &data[full..] {
        checksum = checksum.wrapping_add(u32::from(b) << (8 * shift));
        shift -= 1;
    }
    checksum
}

/// ISAAC PRNG as used by ACE. The seed contributes only its first 4 bytes.
#[derive(Debug)]
pub struct Isaac {
    offset: usize,
    a: u32,
    b: u32,
    c: u32,
    mm: [u32; 256],
    randrsl: [u32; 256],
}

impl Isaac {
    /// Create a new ISAAC stream from a seed (only the first 4 bytes matter).
    #[must_use]
    pub fn new(seed: &[u8]) -> Self {
        let mut mm = [0u32; 256];
        let randrsl = [0u32; 256];

        let mut abcdefgh = [0x9E37_79B9u32; 8];
        for _ in 0..4 {
            shuffle(&mut abcdefgh);
        }
        for i in 0..2 {
            for j in (0..256).step_by(8) {
                for k in 0..8 {
                    abcdefgh[k] = abcdefgh[k].wrapping_add(if i == 0 { randrsl[j + k] } else { mm[j + k] });
                }
                shuffle(&mut abcdefgh);
                mm[j..j + 8].copy_from_slice(&abcdefgh);
            }
        }

        let mut seed4 = [0u8; 4];
        for (slot, &b) in seed4.iter_mut().zip(seed.iter().take(4)) {
            *slot = b;
        }
        let a = u32::from_le_bytes(seed4);
        let c = a;
        let b = a;

        let mut isaac = Isaac { offset: 255, a, b, c, mm, randrsl };
        isaac.scramble();
        isaac
    }

    fn scramble(&mut self) {
        self.c = self.c.wrapping_add(1);
        self.b = self.b.wrapping_add(self.c);
        for i in 0..256 {
            let x = self.mm[i];
            match i & 3 {
                0 => self.a ^= self.a << 13,
                1 => self.a ^= self.a >> 6,
                2 => self.a ^= self.a << 2,
                _ => self.a ^= self.a >> 16,
            }
            self.a = self.a.wrapping_add(self.mm[(i + 128) & 0xFF]);
            let y = self.mm[(x >> 2) as usize & 0xFF]
                .wrapping_add(self.a)
                .wrapping_add(self.b);
            self.mm[i] = y;
            self.b = self
                .mm[(y >> 10) as usize & 0xFF]
                .wrapping_add(x);
            self.randrsl[i] = self.b;
        }
    }

    /// Next 32-bit value from the stream.
    #[allow(clippy::should_implement_trait)] // mirrors C# `Next()`
    pub fn next(&mut self) -> u32 {
        let v = self.randrsl[self.offset];
        if self.offset > 0 {
            self.offset -= 1;
        } else {
            self.scramble();
            self.offset = 255;
        }
        v
    }
}

fn shuffle(x: &mut [u32; 8]) {
    x[0] ^= x[1] << 11;
    x[3] = x[3].wrapping_add(x[0]);
    x[1] = x[1].wrapping_add(x[2]);
    x[1] ^= x[2] >> 2;
    x[4] = x[4].wrapping_add(x[1]);
    x[2] = x[2].wrapping_add(x[3]);
    x[2] ^= x[3] << 8;
    x[5] = x[5].wrapping_add(x[2]);
    x[3] = x[3].wrapping_add(x[4]);
    x[3] ^= x[4] >> 16;
    x[6] = x[6].wrapping_add(x[3]);
    x[4] = x[4].wrapping_add(x[5]);
    x[4] ^= x[5] << 10;
    x[7] = x[7].wrapping_add(x[4]);
    x[5] = x[5].wrapping_add(x[6]);
    x[5] ^= x[6] >> 4;
    x[0] = x[0].wrapping_add(x[5]);
    x[6] = x[6].wrapping_add(x[7]);
    x[6] ^= x[7] << 8;
    x[1] = x[1].wrapping_add(x[6]);
    x[7] = x[7].wrapping_add(x[0]);
    x[7] ^= x[0] >> 9;
    x[2] = x[2].wrapping_add(x[7]);
    x[0] = x[0].wrapping_add(x[1]);
}

/// ISAAC stream that tracks a current key and a search window, used to verify
/// encrypted checksums on the C2S direction. The first `Next()` is consumed in
/// the constructor.
#[derive(Debug)]
pub struct CryptoSystem {
    isaac: Isaac,
    current_key: u32,
    xors: HashSet<u32>,
}

impl CryptoSystem {
    const MAX_EFFORT: usize = 256;

    /// Create a CryptoSystem from a 4-byte seed; the first ISAAC value is
    /// consumed as the initial key.
    #[must_use]
    pub fn new(seed: &[u8]) -> Self {
        let mut isaac = Isaac::new(seed);
        let current_key = isaac.next();
        Self { isaac, current_key, xors: HashSet::new() }
    }

    /// Advance if `x` is the current key; otherwise drop it from the window.
    pub fn consume_key(&mut self, x: u32) {
        if self.current_key == x {
            self.current_key = self.isaac.next();
        } else {
            self.xors.remove(&x);
        }
    }

    /// Try to match `x`, searching up to `MAX_EFFORT` future keys.
    #[must_use]
    pub fn search(&mut self, x: u32) -> bool {
        if self.current_key == x {
            return true;
        }
        if self.xors.contains(&x) {
            return true;
        }
        let budget = Self::MAX_EFFORT - self.xors.len();
        for _ in 0..budget {
            self.xors.insert(self.current_key);
            self.consume_key(self.current_key);
            if self.current_key == x {
                return true;
            }
        }
        false
    }

    /// The current key (useful for tests/debugging).
    #[must_use]
    pub fn current_key(&self) -> u32 {
        self.current_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash32_empty() {
        assert_eq!(hash32(&[]), 0);
    }

    #[test]
    fn hash32_single_byte() {
        // len<<16 = 1<<16, plus byte at shift 24.
        assert_eq!(hash32(&[0xAB]), (1u32 << 16) | (0xABu32 << 24));
    }

    #[test]
    fn hash32_four_bytes() {
        // len<<16 + LE u32.
        let d = [1, 2, 3, 4];
        assert_eq!(hash32(&d), (4u32 << 16) | 0x0403_0201);
    }

    #[test]
    fn hash32_tail_shifts() {
        // 5 bytes: word(1,2,3,4) + byte 5 at shift 24.
        let d = [1, 2, 3, 4, 5];
        let expect = (5u32 << 16).wrapping_add(0x0403_0201).wrapping_add(5u32 << 24);
        assert_eq!(hash32(&d), expect);
    }

    /// Values from an independent transcription of
    /// `ACE.Common.Cryptography.Hash32` (47edade) — pins byte-exactness.
    #[test]
    fn hash32_known_values() {
        assert_eq!(hash32(&[1, 2, 3, 4]), 0x0407_0201);
        assert_eq!(hash32(&[1, 2, 3, 4, 5]), 0x0908_0201);
        assert_eq!(hash32(b"hello world"), 0x4E5A_E9D7);
    }

    #[test]
    fn isaac_deterministic() {
        let mut a = Isaac::new(&[1, 2, 3, 4]);
        let mut b = Isaac::new(&[1, 2, 3, 4]);
        for _ in 0..512 {
            assert_eq!(a.next(), b.next());
        }
    }

    /// Values from an independent transcription of
    /// `ACE.Common.Cryptography.ISAAC` (47edade) — pins byte-exactness.
    #[test]
    fn isaac_known_values() {
        let mut a = Isaac::new(&[1, 2, 3, 4]);
        let expected = [
            0x0772_5DC2, 0xF79A_D8E4, 0xF2A1_8F92, 0x6BC5_7969, 0xE5C0_B864,
            0x18ED_0F35, 0x6345_241F, 0x1B3F_B4DA,
        ];
        for e in expected {
            assert_eq!(a.next(), e);
        }
        let mut b = Isaac::new(&[9, 8, 7, 6]);
        let expected = [
            0x0206_E3C0, 0xA38B_D50C, 0x2758_83E8, 0x34B3_239E, 0x9299_20E5,
            0x5C2D_0AF7, 0x65EF_DF17, 0x9046_D866,
        ];
        for e in expected {
            assert_eq!(b.next(), e);
        }
    }

    #[test]
    fn isaac_seed_only_first_4_bytes() {
        let mut a = Isaac::new(&[9, 8, 7, 6]);
        let mut b = Isaac::new(&[9, 8, 7, 6, 0xFF, 0xFF, 0xFF, 0xFF]);
        for _ in 0..256 {
            assert_eq!(a.next(), b.next());
        }
    }

    #[test]
    fn isaac_seed_sensitivity() {
        let mut a = Isaac::new(&[1, 0, 0, 0]);
        let mut b = Isaac::new(&[2, 0, 0, 0]);
        let mut differ = false;
        for _ in 0..64 {
            if a.next() != b.next() {
                differ = true;
                break;
            }
        }
        assert!(differ);
    }

    #[test]
    fn crypto_first_value_discarded() {
        let mut raw = Isaac::new(&[5, 5, 5, 5]);
        let first = raw.next();
        let mut cs = CryptoSystem::new(&[5, 5, 5, 5]);
        // Current key must be the first ISAAC value; next() must match raw stream.
        assert_eq!(cs.current_key(), first);
        assert_eq!(cs.isaac.next(), raw.next());
    }

    #[test]
    fn crypto_search_and_consume() {
        let mut cs = CryptoSystem::new(&[7, 7, 7, 7]);
        // The current key is always found.
        assert!(cs.search(cs.current_key()));
        let k = cs.current_key();
        cs.consume_key(k);
        assert_ne!(cs.current_key(), k);
    }
}
