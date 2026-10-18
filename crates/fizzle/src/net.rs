//! UDP transport for the ACE protocol.
//!
//! A dedicated reader thread owns S2C fragment reassembly and the ISAAC
//! checksum stream; the owning thread sends C2S. Sequence rules (verified
//! against the server's `NetworkSession`):
//!
//! * C2S packet `Sequence`: `LoginRequest`/`ConnectResponse` are 0, then
//!   strictly `2, 3, 4, …` for real packets (the server's
//!   `lastReceivedPacketSequence` starts at 1 and only real packets advance
//!   it). PAKs (`AckSequence`) reuse the current value and never consume a
//!   number — see `send_pak`.
//! * C2S fragment `Sequence`: strictly `1, 2, 3, …` (per message; split
//!   parts of one message share the message's sequence).
//! * S2C fragments: per-message `Sequence`, split into `Count` parts ordered
//!   by `Index`; a message is complete once all `Count` parts arrive.

use std::collections::HashMap;
use std::net::{UdpSocket, SocketAddr};
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;

use acproto::crypto::Isaac;
use acproto::packet::{self, flags, Header, FragHeader, ParsedPacket, FRAG_HEADER_SIZE, HEADER_SIZE};

/// The retail client version string the server expects.
pub const CLIENT_VERSION: &str = "1802";

/// Default game port (the connect port is `game port + 1`).
pub const DEFAULT_GAME_PORT: u16 = 9000;

/// Events delivered from the reader thread to the application.
/// Some fields are diagnostic protocol data the session does not consume.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum NetEvent {
    /// A fully reassembled S2C game message (starts with the `u32` opcode).
    GameMessage(Vec<u8>),
    /// Optional-header control fields from an S2C packet.
    Control {
        ack: Option<u32>,
        time_sync: Option<f64>,
        echo_response: Option<(f32, f32)>,
    },
    /// The S2C `ConnectRequest` handshake (32-byte body).
    ConnectRequest {
        server_time: f64,
        cookie: u64,
        net_id: u32,
        server_seed: [u8; 4],
        iteration: u16,
    },
    /// S2C ISAAC checksum sanity check failed (logged, non-fatal).
    CryptoMismatch { expected: u32, got: u32 },
    /// A diagnostic message (parse problems, checksum drift, …).
    Debug(String),
}

/// In-flight S2C fragment reassembly for one message.
struct FragReassembly {
    parts: Vec<Option<Vec<u8>>>,
}

/// The UDP transport.
pub struct Net {
    socket: UdpSocket,
    host: String,
    game_port: u16,
    connect_port: u16,
    net_id: u16,
    iteration: u16,
    c2s_seq: u32,
    c2s_frag_seq: u32,
    max_s2c_seq: Arc<AtomicU32>,
    rx: mpsc::Receiver<NetEvent>,
    close_tx: mpsc::Sender<()>,
}

impl Net {
    /// Bind an ephemeral local UDP socket and start the reader thread.
    pub fn connect(host: &str, game_port: u16) -> anyhow::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        let (tx, rx) = mpsc::channel();
        let (close_tx, close_rx) = mpsc::channel();
        let reader = socket.try_clone()?;
        reader.set_read_timeout(Some(std::time::Duration::from_millis(100)))?;
        let max_s2c_seq = Arc::new(AtomicU32::new(0));
        let reader_seq = max_s2c_seq.clone();
        thread::Builder::new()
            .name("fizzle-net".into())
            .spawn(move || reader_loop(reader, tx, close_rx, reader_seq))?;
        Ok(Self {
            socket,
            host: host.to_string(),
            game_port,
            connect_port: game_port.wrapping_add(1),
            net_id: 0,
            iteration: 0,
            c2s_seq: 1,
            c2s_frag_seq: 0,
            max_s2c_seq,
            rx,
            close_tx,
        })
    }

    /// The current NetID (0 until `ConnectRequest` is seen).
    #[allow(dead_code)]
    #[must_use]
    pub fn net_id(&self) -> u16 {
        self.net_id
    }

    /// Signal the reader thread to stop (also happens on drop).
    pub fn close(&self) {
        let _ = self.close_tx.send(());
    }

    /// Drain pending events from the reader thread.
    #[must_use]
    pub fn poll(&self) -> Vec<NetEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            out.push(ev);
        }
        out
    }

    fn target(&self, port: u16) -> SocketAddr {
        format!("{}:{}", self.host, port).parse().expect("valid host:port")
    }

    fn send(&self, bytes: &[u8], port: u16) -> anyhow::Result<()> {
        self.socket.send_to(bytes, self.target(port))?;
        Ok(())
    }

    fn next_packet_seq(&mut self) -> u32 {
        self.c2s_seq += 1;
        self.c2s_seq
    }

    fn next_frag_seq(&mut self) -> u32 {
        self.c2s_frag_seq += 1;
        self.c2s_frag_seq
    }

    /// `LoginRequest` (seq 0, id 0) → game port, body inline (flag 0x10000).
    pub fn send_login(&self, account: &str, password: &str) -> anyhow::Result<()> {
        let body = acproto::messages::login_request_body(CLIENT_VERSION, account, password);
        let header = Header { sequence: 0, flags: flags::LOGIN_REQUEST, id: 0, ..Default::default() };
        self.send(&packet::build_handshake(header, &body), self.game_port)
    }

    /// `ConnectResponse` (seq 0, id = NetID) → connect port, body = u64 cookie.
    pub fn send_connect_response(&mut self, cookie: u64, net_id: u32, iteration: u16) -> anyhow::Result<()> {
        self.net_id = net_id as u16;
        self.iteration = iteration;
        self.send_connect_response_cookie(cookie)
    }

    /// Resend the `ConnectResponse` for an already-stored cookie/NetID.
    ///
    /// The server marks the session `AuthConnectResponse` only after a slow
    /// DB write that runs *after* the `ConnectRequest` is already flushed, so
    /// a first attempt sent immediately on receiving the `ConnectRequest`
    /// races ahead of that state update and is silently dropped.
    /// Retransmitting is safe: a matching response sets the session to
    /// `AuthConnected`, so later copies no longer match and are ignored.
    pub fn retransmit_connect_response(&self, cookie: u64) -> anyhow::Result<()> {
        self.send_connect_response_cookie(cookie)
    }

    fn send_connect_response_cookie(&self, cookie: u64) -> anyhow::Result<()> {
        let body = acproto::messages::connect_response_body(cookie);
        let header = Header {
            sequence: 0,
            flags: flags::CONNECT_RESPONSE,
            id: self.net_id,
            iteration: self.iteration,
            ..Default::default()
        };
        self.send(&packet::build_handshake(header, &body), self.connect_port)
    }

    /// A C2S game message carried as a single fragment.
    pub fn send_game_message(&mut self, opcode: u32, body: &[u8]) -> anyhow::Result<()> {
        self.send_fragment(&acproto::messages::game_message(opcode, body))
    }

    /// A C2S `GameAction` (0xF7B1) carried as a single fragment.
    pub fn send_game_action(&mut self, client_seq: u32, action_type: u32, body: &[u8]) -> anyhow::Result<()> {
        self.send_fragment(&acproto::messages::game_action(client_seq, action_type, body))
    }

    fn send_fragment(&mut self, message: &[u8]) -> anyhow::Result<()> {
        anyhow::ensure!(
            message.len() <= packet::MAX_FRAGMENT_DATA,
            "C2S fragment too large: {} bytes (max {})",
            message.len(),
            packet::MAX_FRAGMENT_DATA
        );
        let header = Header {
            sequence: self.next_packet_seq(),
            flags: flags::BLOB_FRAGMENTS,
            id: self.net_id,
            iteration: self.iteration,
            ..Default::default()
        };
        let fh = FragHeader {
            sequence: self.next_frag_seq(),
            id: 0,
            count: 1,
            size: (FRAG_HEADER_SIZE + message.len()) as u16,
            index: 0,
            queue: 0,
        };
        self.send(&packet::build_message_packet(header, fh, message), self.game_port)
    }

    /// The highest S2C packet `Sequence` seen so far (for the PAK keepalive).
    #[must_use]
    pub fn max_s2c_seq(&self) -> u32 {
        self.max_s2c_seq.load(Ordering::Relaxed)
    }

    /// Exit broadcast: `ClientNet::LogOffServer` (acclient.c:371645, 0x543EF0)
    /// sends a bare 20-byte packet — flag `0x8000` (Disconnect), recID = NetID,
    /// iteration = the S2C iteration, no payload, plain checksum — so the
    /// server drops the session immediately instead of waiting out its
    /// 15 s auth / 60 s session timeout. Retail fires it on every exit path
    /// (`Client::CleanupNet`, 0x411D00).
    pub fn send_disconnect(&self) -> anyhow::Result<()> {
        let header = Header {
            sequence: 0,
            flags: flags::DISCONNECT,
            id: self.net_id,
            iteration: self.iteration,
            ..Default::default()
        };
        self.send(&packet::build_packet(header, &[], &[]), self.game_port)
    }

    /// Keepalive: `SharedNet::EnqueuePak` (acclient.c:371419, 0x543B10) sends an
    /// `AckSequence` optional header (flag `0x4000`, body = highest S2C packet
    /// sequence received) at most once per 2.0 s on every connected session.
    /// Any valid C2S packet resets the server's session timeout
    /// (PROTOCOL.md §13), which is what keeps an idle session alive.
    ///
    /// The PAK reuses `c2s_seq` (the last real-packet sequence, which equals
    /// the server's `lastReceivedPacketSequence`) instead of consuming a new
    /// number: the server advances `lastReceivedPacketSequence` only for
    /// non-`AckSequence` packets (`NetworkSession.HandleOrderedPacket`), so a
    /// PAK that consumed a number would desync the counter and leave the next
    /// real packet ahead of `desiredSeq` — parked in `outOfOrderPackets` and
    /// never processed. A PAK with `Sequence == lastReceivedPacketSequence`
    /// is explicitly accepted by the server's duplicate check
    /// (`NetworkSession.ProcessPacket`).
    /// Returns the C2S packet sequence used.
    pub fn send_pak(&mut self, ack: u32) -> anyhow::Result<u32> {
        let seq = self.c2s_seq;
        let header = Header {
            sequence: seq,
            flags: flags::ACK_SEQUENCE,
            id: self.net_id,
            iteration: self.iteration,
            ..Default::default()
        };
        self.send(&packet::build_packet(header, &ack.to_le_bytes(), &[]), self.game_port)?;
        Ok(seq)
    }
}

impl Drop for Net {
    fn drop(&mut self) {
        self.close();
    }
}

/// The reader thread: receive, parse, reassemble, verify, and emit events.
fn reader_loop(
    socket: UdpSocket,
    tx: mpsc::Sender<NetEvent>,
    close_rx: mpsc::Receiver<()>,
    max_s2c_seq: Arc<AtomicU32>,
) {
    let mut buf = vec![0u8; 65536];
    let mut isaac: Option<Isaac> = None;
    let mut pending: HashMap<u32, FragReassembly> = HashMap::new();

    loop {
        if close_rx.try_recv().is_ok() {
            break;
        }
        let n = match socket.recv_from(&mut buf) {
            Ok((n, _)) => n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => break,
        };
        if n < HEADER_SIZE {
            continue;
        }
        let buf = &buf[..n];
        let header = Header::unpack(buf[..HEADER_SIZE].try_into().unwrap());
        max_s2c_seq.fetch_max(header.sequence as u32, Ordering::Relaxed);

        // The ConnectRequest handshake carries a 32-byte body, not fragments.
        if header.has_flag(flags::CONNECT_REQUEST) {
            let payload = &buf[HEADER_SIZE..];
            if payload.len() >= 32 {
                let server_time = f64::from_le_bytes(payload[0..8].try_into().unwrap());
                let cookie = u64::from_le_bytes(payload[8..16].try_into().unwrap());
                let net_id = u32::from_le_bytes(payload[16..20].try_into().unwrap());
                let server_seed: [u8; 4] = payload[20..24].try_into().unwrap();
                // S2C encryption uses a plain ISAAC stream seeded with the
                // server seed (no discarded first value).
                isaac = Some(Isaac::new(&server_seed));
                let _ = tx.send(NetEvent::ConnectRequest { server_time, cookie, net_id, server_seed, iteration: header.iteration });
            }
            continue;
        }

        let parsed = match ParsedPacket::parse(buf) {
            Ok(p) => p,
            Err(e) => {
                let _ = tx.send(NetEvent::Debug(format!("S2C parse error: {e}")));
                continue;
            }
        };

        // Checksum sanity. Encrypted packets must match the next ISAAC value;
        // plain packets must match headerHash + payloadHash.
        if parsed.header.has_flag(flags::ENCRYPTED_CHECKSUM) {
            let got = parsed.encrypted_key();
            if let Some(stream) = isaac.as_mut() {
                let expected = stream.next();
                if expected != got {
                    let _ = tx.send(NetEvent::CryptoMismatch { expected, got });
                }
            }
        } else if !parsed.verify_plain() {
            let _ = tx.send(NetEvent::Debug(format!(
                "S2C plain checksum mismatch (hh={:#010x} payload={:#010x})",
                parsed.header.header_hash(),
                parsed.payload_hash
            )));
        }

        if parsed.optional.ack_sequence.is_some()
            || parsed.optional.time_sync.is_some()
            || parsed.optional.echo_response.is_some()
        {
            let _ = tx.send(NetEvent::Control {
                ack: parsed.optional.ack_sequence,
                time_sync: parsed.optional.time_sync,
                echo_response: parsed.optional.echo_response,
            });
        }

        if parsed.header.has_flag(flags::BLOB_FRAGMENTS) {
            for frag in &parsed.fragments {
                let fh = &frag.header;
                if fh.count == 0 {
                    continue;
                }
                let entry = pending.entry(fh.sequence).or_insert_with(|| FragReassembly {
                    parts: vec![None; fh.count as usize],
                });
                if (fh.index as usize) < entry.parts.len() {
                    entry.parts[fh.index as usize] = Some(frag.data.clone());
                }
                if entry.parts.iter().all(|p| p.is_some()) {
                    let mut data = Vec::new();
                    for part in entry.parts.drain(..) {
                        data.extend_from_slice(part.as_ref().expect("part present"));
                    }
                    pending.remove(&fh.sequence);
                    let _ = tx.send(NetEvent::GameMessage(data));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real listener socket on an ephemeral port: `Net::connect` targets it,
    /// so the send path is exercised end-to-end without touching a server.
    fn capture_net() -> (Net, UdpSocket) {
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let net = Net::connect("127.0.0.1", port).unwrap();
        (net, listener)
    }

    #[test]
    fn disconnect_is_bare_20_byte_header() {
        let (mut net, listener) = capture_net();
        net.net_id = 7;
        net.iteration = 3;
        net.send_disconnect().unwrap();

        let mut buf = [0u8; 64];
        let n = listener.recv_from(&mut buf).unwrap().0;
        assert_eq!(n, HEADER_SIZE, "Disconnect has no payload");
        let parsed = ParsedPacket::parse(&buf[..n]).unwrap();
        assert_eq!(parsed.header.flags, flags::DISCONNECT);
        assert_eq!(parsed.header.sequence, 0);
        assert_eq!(parsed.header.id, 7);
        assert_eq!(parsed.header.iteration, 3);
        assert_eq!(parsed.header.size, 0);
        assert!(parsed.verify_plain());
    }

    #[test]
    fn pak_reuses_packet_seq_without_consuming() {
        let (net, listener) = capture_net();
        let mut net = net;
        net.net_id = 9;
        net.iteration = 1;

        // The first PAK reuses c2s_seq (1): the server's
        // lastReceivedPacketSequence starts at 1 and PAKs never advance it,
        // so the PAK must carry exactly that value.
        let seq = net.send_pak(42).unwrap();
        assert_eq!(seq, 1, "PAK reuses the current packet seq; no consumption");

        let mut buf = [0u8; 128];
        let n = listener.recv_from(&mut buf).unwrap().0;
        assert_eq!(n, HEADER_SIZE + 4);
        let parsed = ParsedPacket::parse(&buf[..n]).unwrap();
        assert_eq!(parsed.header.flags, flags::ACK_SEQUENCE);
        assert_eq!(parsed.header.sequence, 1);
        assert_eq!(parsed.header.id, 9);
        assert_eq!(parsed.header.iteration, 1);
        assert_eq!(parsed.header.size, 4);
        assert_eq!(parsed.optional.ack_sequence, Some(42));
        assert!(parsed.verify_plain());

        // A second PAK still carries the same sequence.
        assert_eq!(net.send_pak(43).unwrap(), 1);
        let n = listener.recv_from(&mut buf).unwrap().0;
        let parsed = ParsedPacket::parse(&buf[..n]).unwrap();
        assert_eq!(parsed.header.flags, flags::ACK_SEQUENCE);
        assert_eq!(parsed.header.sequence, 1);
        assert_eq!(parsed.optional.ack_sequence, Some(43));

        // A real game message consumes the next packet sequence (2).
        net.send_game_message(0xF656, &[0, 0, 0, 0]).unwrap();
        let n = listener.recv_from(&mut buf).unwrap().0;
        let parsed = ParsedPacket::parse(&buf[..n]).unwrap();
        assert_eq!(parsed.header.flags, flags::BLOB_FRAGMENTS);
        assert_eq!(parsed.header.sequence, 2);

        // The PAK now follows the real packet's sequence (== the server's
        // lastReceivedPacketSequence once it processes the message).
        assert_eq!(net.send_pak(44).unwrap(), 2);
        let n = listener.recv_from(&mut buf).unwrap().0;
        let parsed = ParsedPacket::parse(&buf[..n]).unwrap();
        assert_eq!(parsed.header.flags, flags::ACK_SEQUENCE);
        assert_eq!(parsed.header.sequence, 2);
        assert_eq!(parsed.optional.ack_sequence, Some(44));
    }
}
