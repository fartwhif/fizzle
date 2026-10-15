//! `acproto` — Asheron's Call wire protocol: packets, crypto, message codecs,
//! and character-generation data.
//!
//! Layouts are byte-exact against the ACE emulator server source
//! (`ACE.Common.Cryptography`, `ACE.Server.Network` @ 47edade) and the specs in
//! `docs/PROTOCOL.md`.

pub mod chargen;
pub mod codec;
pub mod crypto;
pub mod messages;
pub mod motion;
pub mod packet;
pub mod strings;
