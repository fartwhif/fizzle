//! ACE length-prefixed string encodings (`ReadString16L` / `ReadString32L` in
//! `ACE.Common.Extensions` @ 47edade).

/// Append a `String16L`: `u16 len + bytes`, padded so the total (2 + len) is a
/// multiple of 4.
pub fn push_string16l(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    out.extend_from_slice(&(b.len() as u16).to_le_bytes());
    out.extend_from_slice(b);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// Read a `String16L` at `p`; returns the string and the new position.
/// Returns `None` if the buffer is truncated.
pub fn read_string16l(buf: &[u8], p: usize) -> Option<(&str, usize)> {
    let len = u16::from_le_bytes([buf[p], buf[p + 1]]) as usize;
    let content_start = p + 2;
    let end = content_start.checked_add(len)?;
    if end > buf.len() {
        return None;
    }
    let s = std::str::from_utf8(&buf[content_start..end]).ok()?;
    // Total size (2-byte length + content) padded up to a multiple of 4.
    let total = (2 + len).div_ceil(4) * 4;
    if p + total > buf.len() {
        return None;
    }
    Some((s, p + total))
}

/// Append a `String32L` (used for the login password): `u32 X` where
/// `X = 1+len` (or `2+len` for len > 255), then the packed length (1 or 2
/// bytes), then the content, then `CalculatePadMultiple(4+len, 4)` zero bytes.
/// The server pads against `4 + len` (not `4 + X`), so the total size is
/// `packed + 4*ceil((4+len)/4)`.
pub fn push_string32l(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    let n = b.len() as u32;
    let x = if n <= 255 { n + 1 } else { n + 2 };
    out.extend_from_slice(&x.to_le_bytes());
    if n <= 255 {
        out.push(n as u8);
    } else {
        out.push(n as u8);
        out.push((n >> 8) as u8);
    }
    out.extend_from_slice(b);
    let base = 4 + n;
    let pad = base.div_ceil(4) * 4 - base;
    out.extend(std::iter::repeat_n(0u8, pad as usize));
}

/// Read a `String32L` at `p`; returns the string and the new position.
/// Returns `None` if the buffer is truncated.
pub fn read_string32l(buf: &[u8], p: usize) -> Option<(&str, usize)> {
    let mut q = p;
    let x = u32::from_le_bytes([buf[q], buf[q + 1], buf[q + 2], buf[q + 3]]);
    q += 4;
    if x == 0 {
        return Some(("", q));
    }
    let mut len = x;
    q += 1;
    len -= 1;
    if len > 255 {
        q += 1;
        len -= 1;
    }
    let end = q.checked_add(len as usize)?;
    if end > buf.len() {
        return None;
    }
    let s = std::str::from_utf8(&buf[q..end]).ok()?;
    let pad = (4 + len).div_ceil(4) * 4 - (4 + len);
    let total = (q - p) + len as usize + pad as usize;
    if p + total > buf.len() {
        return None;
    }
    Some((s, p + total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string16l_roundtrip() {
        for s in ["", "a", "abcd", "1802", "Gharu'ndim", &"x".repeat(100)] {
            let mut v = Vec::new();
            push_string16l(&mut v, s);
            let (got, end) = read_string16l(&v, 0).unwrap();
            assert_eq!(got, s);
            assert_eq!(end, v.len());
        }
    }

    #[test]
    fn string16l_padding_exact() {
        // "1802": 2 + 4 = 6 -> pad 2 -> 8 bytes.
        let mut v = Vec::new();
        push_string16l(&mut v, "1802");
        assert_eq!(v.len(), 8);
        assert_eq!(&v[2..6], b"1802");
        assert_eq!(&v[6..], &[0, 0]);
    }

    #[test]
    fn string32l_roundtrip() {
        for s in ["", "pw", "hunter2", &"x".repeat(255), &"y".repeat(300)] {
            let mut v = Vec::new();
            push_string32l(&mut v, s);
            let (got, end) = read_string32l(&v, 0).unwrap();
            assert_eq!(got, s);
            assert_eq!(end, v.len());
        }
    }

    #[test]
    fn string32l_layout_password() {
        // "abc": X = 4, packed 1, base = 4+3 = 7 -> pad 1. Total 9 bytes:
        // 04 00 00 00 | 03 | a b c | 00
        let mut v = Vec::new();
        push_string32l(&mut v, "abc");
        assert_eq!(v.len(), 9);
        assert_eq!(v, [4, 0, 0, 0, 3, b'a', b'b', b'c', 0]);
    }
}
