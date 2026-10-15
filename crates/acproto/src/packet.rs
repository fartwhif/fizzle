//! ACE packet framing: 20-byte header, optional-header fields, 16-byte fragment
//! headers, and the Hash32 checksum. Byte-exact against `ACE.Server.Network`
//! @ 47edade.
//!
//! Model (both directions):
//!   payload_hash = Hash32(opt_bytes) + Σ (Hash32(frag_hdr) + Hash32(frag_data))
//!   plain:        Checksum = header_hash + payload_hash
//!   encrypted:    Checksum = header_hash + (payload_hash ^ issac_key)
//!
//! C2S game messages are always carried as fragments (`BlobFragments`); the
//! handshake packets (`LoginRequest`/`ConnectResponse`) carry their body inline
//! as the optional-header bytes.

use crate::crypto::hash32;

pub const HEADER_SIZE: usize = 20;
pub const FRAG_HEADER_SIZE: usize = 16;
/// Max fragment data size (464 - 16).
pub const MAX_FRAGMENT_DATA: usize = 448;

pub mod flags {
    pub const RETRANSMISSION: u32 = 0x0000_0001;
    pub const ENCRYPTED_CHECKSUM: u32 = 0x0000_0002;
    pub const BLOB_FRAGMENTS: u32 = 0x0000_0004;
    pub const SERVER_SWITCH: u32 = 0x0000_0100;
    pub const LOGON_SERVER_ADDR: u32 = 0x0000_0200;
    pub const EMPTY_HEADER1: u32 = 0x0000_0400;
    pub const REFERRAL: u32 = 0x0000_0800;
    pub const REQUEST_RETRANSMIT: u32 = 0x0000_1000;
    pub const REJECT_RETRANSMIT: u32 = 0x0000_2000;
    pub const ACK_SEQUENCE: u32 = 0x0000_4000;
    pub const DISCONNECT: u32 = 0x0000_8000;
    pub const LOGIN_REQUEST: u32 = 0x0001_0000;
    pub const WORLD_LOGIN_REQUEST: u32 = 0x0002_0000;
    pub const CONNECT_REQUEST: u32 = 0x0004_0000;
    pub const CONNECT_RESPONSE: u32 = 0x0008_0000;
    pub const NET_ERROR: u32 = 0x0010_0000;
    pub const NET_ERROR_DISCONNECT: u32 = 0x0020_0000;
    pub const CICMD_COMMAND: u32 = 0x0040_0000;
    pub const TIME_SYNC: u32 = 0x0100_0000;
    pub const ECHO_REQUEST: u32 = 0x0200_0000;
    pub const ECHO_RESPONSE: u32 = 0x0400_0000;
    pub const FLOW: u32 = 0x0800_0000;
}

/// 20-byte packet header (little-endian).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub sequence: u32,
    pub flags: u32,
    pub checksum: u32,
    pub id: u16,
    pub time: u16,
    pub size: u16,
    pub iteration: u16,
}

impl Header {
    #[must_use]
    pub fn pack(&self) -> [u8; HEADER_SIZE] {
        let mut b = [0u8; HEADER_SIZE];
        b[0..4].copy_from_slice(&self.sequence.to_le_bytes());
        b[4..8].copy_from_slice(&self.flags.to_le_bytes());
        b[8..12].copy_from_slice(&self.checksum.to_le_bytes());
        b[12..14].copy_from_slice(&self.id.to_le_bytes());
        b[14..16].copy_from_slice(&self.time.to_le_bytes());
        b[16..18].copy_from_slice(&self.size.to_le_bytes());
        b[18..20].copy_from_slice(&self.iteration.to_le_bytes());
        b
    }

    #[must_use]
    pub fn unpack(buf: &[u8; HEADER_SIZE]) -> Self {
        Self {
            sequence: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            flags: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            checksum: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            id: u16::from_le_bytes([buf[12], buf[13]]),
            time: u16::from_le_bytes([buf[14], buf[15]]),
            size: u16::from_le_bytes([buf[16], buf[17]]),
            iteration: u16::from_le_bytes([buf[18], buf[19]]),
        }
    }

    /// Hash32 of the packed header with the Checksum slot forced to
    /// `0xBADD70DD`.
    #[must_use]
    pub fn header_hash(&self) -> u32 {
        let mut h = *self;
        h.checksum = 0xBADD_70DD;
        hash32(&h.pack())
    }

    #[must_use]
    pub fn has_flag(&self, f: u32) -> bool {
        self.flags & f != 0
    }
}

/// 16-byte fragment header (little-endian).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FragHeader {
    pub sequence: u32,
    pub id: u32,
    pub count: u16,
    pub size: u16,
    pub index: u16,
    pub queue: u16,
}

impl FragHeader {
    #[must_use]
    pub fn pack(&self) -> [u8; FRAG_HEADER_SIZE] {
        let mut b = [0u8; FRAG_HEADER_SIZE];
        b[0..4].copy_from_slice(&self.sequence.to_le_bytes());
        b[4..8].copy_from_slice(&self.id.to_le_bytes());
        b[8..10].copy_from_slice(&self.count.to_le_bytes());
        b[10..12].copy_from_slice(&self.size.to_le_bytes());
        b[12..14].copy_from_slice(&self.index.to_le_bytes());
        b[14..16].copy_from_slice(&self.queue.to_le_bytes());
        b
    }

    #[must_use]
    pub fn unpack(buf: &[u8; FRAG_HEADER_SIZE]) -> Self {
        Self {
            sequence: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            id: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            count: u16::from_le_bytes([buf[8], buf[9]]),
            size: u16::from_le_bytes([buf[10], buf[11]]),
            index: u16::from_le_bytes([buf[12], buf[13]]),
            queue: u16::from_le_bytes([buf[14], buf[15]]),
        }
    }
}

/// Decoded optional-header fields present in a packet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionalInfo {
    pub ack_sequence: Option<u32>,
    pub time_sync: Option<f64>,
    pub echo_request: Option<f32>,
    /// (client_time, server_delta)
    pub echo_response: Option<(f32, f32)>,
    pub flow: Option<(u32, u16)>,
    pub retransmit: Vec<u32>,
}

/// A parsed fragment.
#[derive(Clone, Debug)]
pub struct Fragment {
    pub header: FragHeader,
    pub data: Vec<u8>,
}

/// Error while parsing a packet.
#[derive(Debug)]
pub enum PacketError {
    TooShort(usize),
    BadSize,
    TruncatedFragment,
}

impl std::fmt::Display for PacketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort(n) => write!(f, "packet too short: {n} bytes"),
            Self::BadSize => write!(f, "header size mismatch"),
            Self::TruncatedFragment => write!(f, "truncated fragment"),
        }
    }
}

impl std::error::Error for PacketError {}

/// A fully parsed packet (typically S2C).
#[derive(Clone, Debug)]
pub struct ParsedPacket {
    pub header: Header,
    pub optional: OptionalInfo,
    /// Raw optional-header bytes (for checksum verification).
    pub opt_bytes: Vec<u8>,
    pub fragments: Vec<Fragment>,
    /// `Hash32(opt_bytes) + Σ(Hash32(frag_hdr)+Hash32(frag_data))`.
    pub payload_hash: u32,
}

impl ParsedPacket {
    /// Parse a packet from its raw bytes (header + payload).
    pub fn parse(buf: &[u8]) -> Result<Self, PacketError> {
        if buf.len() < HEADER_SIZE {
            return Err(PacketError::TooShort(buf.len()));
        }
        let hdr = Header::unpack(buf[..HEADER_SIZE].try_into().unwrap());
        let payload_len = hdr.size as usize;
        if buf.len() < HEADER_SIZE + payload_len {
            return Err(PacketError::BadSize);
        }
        let payload = &buf[HEADER_SIZE..HEADER_SIZE + payload_len];

        let (opt_bytes, optional, rest) = parse_optional(payload, hdr.flags);

        let mut fragments = Vec::new();
        let mut payload_hash = hash32(&opt_bytes);
        if hdr.has_flag(flags::BLOB_FRAGMENTS) {
            let mut p = 0;
            while p < rest.len() {
                if p + FRAG_HEADER_SIZE > rest.len() {
                    return Err(PacketError::TruncatedFragment);
                }
                let fh = FragHeader::unpack(rest[p..p + FRAG_HEADER_SIZE].try_into().unwrap());
                let data_len = (fh.size as usize).saturating_sub(FRAG_HEADER_SIZE);
                let ds = p + FRAG_HEADER_SIZE;
                if ds + data_len > rest.len() {
                    return Err(PacketError::TruncatedFragment);
                }
                let data = rest[ds..ds + data_len].to_vec();
                payload_hash = payload_hash
                    .wrapping_add(hash32(&fh.pack()))
                    .wrapping_add(hash32(&data));
                fragments.push(Fragment { header: fh, data });
                p = ds + data_len;
            }
        }

        Ok(Self {
            header: hdr,
            optional,
            opt_bytes,
            fragments,
            payload_hash,
        })
    }

    /// True if the plain (non-encrypted) checksum matches.
    #[must_use]
    pub fn verify_plain(&self) -> bool {
        self.header.checksum == self.header.header_hash().wrapping_add(self.payload_hash)
    }

    /// The ISAAC key implied by an encrypted checksum:
    /// `(checksum - header_hash) ^ payload_hash`.
    #[must_use]
    pub fn encrypted_key(&self) -> u32 {
        self.header.checksum.wrapping_sub(self.header.header_hash()) ^ self.payload_hash
    }
}

/// Consume optional-header fields in canonical order; return the raw bytes
/// consumed, the decoded info, and the remainder (fragment area).
fn parse_optional(payload: &[u8], flags: u32) -> (Vec<u8>, OptionalInfo, &[u8]) {
    let mut info = OptionalInfo::default();
    let mut out = Vec::new();
    let mut p = 0usize;

    macro_rules! take {
        ($n:expr) => {{
            let n = $n;
            if p + n > payload.len() {
                return (out, info, &[]);
            }
            out.extend_from_slice(&payload[p..p + n]);
            p += n;
        }};
    }
    macro_rules! u32 {
        () => {{
            if p + 4 > payload.len() {
                return (out, info, &[]);
            }
            let v = u32::from_le_bytes([payload[p], payload[p + 1], payload[p + 2], payload[p + 3]]);
            out.extend_from_slice(&payload[p..p + 4]);
            p += 4;
            v
        }};
    }

    if flags & flags::SERVER_SWITCH != 0 {
        take!(8);
    }
    if flags & flags::REQUEST_RETRANSMIT != 0 {
        let count = u32!();
        for _ in 0..count {
            let v = u32!();
            info.retransmit.push(v);
        }
    }
    if flags & flags::REJECT_RETRANSMIT != 0 {
        let count = u32!();
        for _ in 0..count {
            take!(4);
        }
    }
    if flags & flags::ACK_SEQUENCE != 0 {
        info.ack_sequence = Some(u32!());
    }
    if flags & flags::LOGIN_REQUEST != 0 {
        // "the rest" of the payload is the login body.
        out.extend_from_slice(&payload[p..]);
        return (out, info, &[]);
    }
    if flags & flags::WORLD_LOGIN_REQUEST != 0 {
        take!(8);
    }
    if flags & flags::CONNECT_RESPONSE != 0 {
        take!(8);
    }
    if flags & flags::CICMD_COMMAND != 0 {
        take!(8);
    }
    if flags & flags::TIME_SYNC != 0 {
        if p + 8 <= payload.len() {
            let b: [u8; 8] = payload[p..p + 8].try_into().unwrap();
            info.time_sync = Some(f64::from_le_bytes(b));
            out.extend_from_slice(&payload[p..p + 8]);
            p += 8;
        } else {
            return (out, info, &[]);
        }
    }
    if flags & flags::ECHO_REQUEST != 0 {
        if p + 4 <= payload.len() {
            let b: [u8; 4] = payload[p..p + 4].try_into().unwrap();
            info.echo_request = Some(f32::from_le_bytes(b));
            out.extend_from_slice(&payload[p..p + 4]);
            p += 4;
        } else {
            return (out, info, &[]);
        }
    }
    if flags & flags::ECHO_RESPONSE != 0 {
        if p + 8 <= payload.len() {
            let b: [u8; 8] = payload[p..p + 8].try_into().unwrap();
            info.echo_response = Some((
                f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                f32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            ));
            out.extend_from_slice(&payload[p..p + 8]);
            p += 8;
        } else {
            return (out, info, &[]);
        }
    }
    if flags & flags::FLOW != 0 {
        if p + 6 <= payload.len() {
            let bytes = u32!();
            if p + 2 <= payload.len() {
                let interval = u16::from_le_bytes([payload[p], payload[p + 1]]);
                out.extend_from_slice(&payload[p..p + 2]);
                p += 2;
                info.flow = Some((bytes, interval));
            } else {
                return (out, info, &[]);
            }
        } else {
            return (out, info, &[]);
        }
    }

    (out, info, &payload[p..])
}

/// Build a complete C2S packet with a plain checksum. `opt_bytes` are the
/// inline optional-header bytes (handshake bodies) and `fragments` is a list of
/// (header, data) pairs. The header `size` and `checksum` are filled in.
#[must_use]
pub fn build_packet(mut header: Header, opt_bytes: &[u8], fragments: &[(FragHeader, &[u8])]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(opt_bytes.len());
    let mut payload_hash = hash32(opt_bytes);
    payload.extend_from_slice(opt_bytes);
    for (fh, data) in fragments {
        payload.extend_from_slice(&fh.pack());
        payload.extend_from_slice(data);
        payload_hash = payload_hash
            .wrapping_add(hash32(&fh.pack()))
            .wrapping_add(hash32(data));
    }
    header.size = payload.len() as u16;
    header.checksum = header.header_hash().wrapping_add(payload_hash);
    let mut out = Vec::with_capacity(HEADER_SIZE + payload.len());
    out.extend_from_slice(&header.pack());
    out.extend_from_slice(&payload);
    out
}

/// Convenience: build a C2S packet carrying a single inline handshake body.
#[must_use]
pub fn build_handshake(header: Header, body: &[u8]) -> Vec<u8> {
    build_packet(header, body, &[])
}

/// Convenience: build a C2S packet carrying one game-message fragment.
#[must_use]
pub fn build_message_packet(header: Header, frag_header: FragHeader, message: &[u8]) -> Vec<u8> {
    build_packet(header, &[], &[(frag_header, message)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let h = Header {
            sequence: 7,
            flags: flags::BLOB_FRAGMENTS | flags::ACK_SEQUENCE,
            checksum: 0xDEAD_BEEF,
            id: 0x1234,
            time: 5,
            size: 64,
            iteration: 2,
        };
        assert_eq!(Header::unpack(&h.pack()), h);
    }

    #[test]
    fn frag_header_roundtrip() {
        let f = FragHeader {
            sequence: 1,
            id: 0x8000_0000,
            count: 3,
            size: 100,
            index: 2,
            queue: 9,
        };
        assert_eq!(FragHeader::unpack(&f.pack()), f);
    }

    #[test]
    fn message_packet_roundtrip_and_checksum() {
        let header = Header {
            sequence: 2,
            flags: flags::BLOB_FRAGMENTS,
            id: 42,
            ..Default::default()
        };
        let fh = FragHeader {
            sequence: 1,
            id: 0,
            count: 1,
            size: (FRAG_HEADER_SIZE + 8) as u16,
            index: 0,
            queue: 0,
        };
        let msg = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let bytes = build_message_packet(header, fh, &msg);
        let parsed = ParsedPacket::parse(&bytes).unwrap();
        assert!(parsed.verify_plain(), "plain checksum should match");
        assert_eq!(parsed.fragments.len(), 1);
        assert_eq!(parsed.fragments[0].data, msg);
        assert_eq!(parsed.fragments[0].header, fh);
    }

    #[test]
    fn handshake_packet_roundtrip() {
        let header = Header {
            sequence: 0,
            flags: flags::CONNECT_RESPONSE,
            id: 7,
            ..Default::default()
        };
        let body = 0x1122_3344_5566_7788u64.to_le_bytes();
        let bytes = build_handshake(header, &body);
        let parsed = ParsedPacket::parse(&bytes).unwrap();
        assert!(parsed.verify_plain());
        assert_eq!(parsed.opt_bytes, body);
        assert!(parsed.fragments.is_empty());
    }

    #[test]
    fn multi_fragment_payload_hash() {
        let header = Header {
            sequence: 3,
            flags: flags::BLOB_FRAGMENTS,
            ..Default::default()
        };
        let f1 = FragHeader { sequence: 5, count: 2, size: (16 + 4) as u16, index: 0, ..Default::default() };
        let f2 = FragHeader { sequence: 5, count: 2, size: (16 + 6) as u16, index: 1, ..Default::default() };
        let bytes = build_packet(header, &[], &[(f1, &[1, 2, 3, 4]), (f2, &[5, 6, 7, 8, 9, 10])]);
        let parsed = ParsedPacket::parse(&bytes).unwrap();
        assert!(parsed.verify_plain());
        assert_eq!(parsed.fragments.len(), 2);
    }

    #[test]
    fn encrypted_key_roundtrip() {
        // Simulate an encrypted S2C: checksum = headerHash + (payload ^ key).
        let header = Header {
            sequence: 2,
            flags: flags::BLOB_FRAGMENTS | flags::ENCRYPTED_CHECKSUM,
            id: 0x000B,
            ..Default::default()
        };
        let fh = FragHeader {
            sequence: 1, id: 0x8000_0000, count: 1, size: (16 + 4) as u16, index: 0, queue: 1,
        };
        let msg = [9u8, 8, 7, 6];
        let key = 0x1234_5678;
        // Build then patch the checksum to the encrypted form.
        let mut bytes = build_message_packet(header, fh, &msg);
        let parsed = ParsedPacket::parse(&bytes).unwrap();
        let hh = parsed.header.header_hash();
        let enc = hh.wrapping_add(parsed.payload_hash ^ key);
        bytes[8..12].copy_from_slice(&enc.to_le_bytes());
        let reparsed = ParsedPacket::parse(&bytes).unwrap();
        assert!(!reparsed.verify_plain());
        assert_eq!(reparsed.encrypted_key(), key);
    }

    #[test]
    fn optional_time_sync_parse() {
        let mut opt = Vec::new();
        opt.extend_from_slice(&12345.5f64.to_le_bytes());
        let header = Header {
            sequence: 4,
            flags: flags::TIME_SYNC,
            size: 8,
            ..Default::default()
        };
        let header_hash = header.header_hash();
        let payload_hash = hash32(&opt);
        let mut bytes = header.pack().to_vec();
        bytes.extend_from_slice(&opt);
        // set checksum plain
        let mut h = header;
        h.checksum = header_hash.wrapping_add(payload_hash);
        bytes[..HEADER_SIZE].copy_from_slice(&h.pack());
        let parsed = ParsedPacket::parse(&bytes).unwrap();
        assert_eq!(parsed.optional.time_sync, Some(12345.5));
        assert!(parsed.verify_plain());
    }
}
