//! Motion: constants, C2S movement-action body builders (MoveToState,
//! AutonomousPosition, Jump), and the S2C `Motion` (0xF74C) message layout.
//!
//! Layouts verified against `ACE.Server.Network.Motion` @ 47edade and
//! `docs/PROTOCOL.md`.

use crate::codec::{Position, Vec3, Writer};

/// RawMotionState style used for all autonomous movement.
pub const MOTION_STYLE: u32 = 0x8000_003D;
/// Packed flags covering all 11 RawMotionState fields.
pub const PACKED_FLAGS_ALL: u32 = 0x7FF;

pub mod hold_key {
    pub const INVALID: u32 = 0;
    pub const NONE: u32 = 1;
    pub const RUN: u32 = 2;
}

pub mod command {
    pub const READY: u32 = 0x4100_0003;
    pub const WALK_FORWARD: u32 = 0x4500_0005;
    pub const WALK_BACKWARD: u32 = 0x4500_0006;
    pub const RUN_FORWARD: u32 = 0x4400_0007;
    pub const TURN_RIGHT: u32 = 0x6500_000D;
    pub const TURN_LEFT: u32 = 0x6500_000E;
    pub const SIDESTEP_RIGHT: u32 = 0x6500_000F;
    pub const SIDESTEP_LEFT: u32 = 0x6500_0010;
}

/// A complete RawMotionState (the body of the MoveToState 0xF61C action).
/// Fixed 89 bytes: packed flags + 11 fields + 32 B position + 4 u16 + 1 u8.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoveToState {
    /// u32 PackedFlags (0x7ff = all fields present).
    pub packed_flags: u32,
    pub current_hold_key: u32,
    pub current_style: u32,
    pub forward_command: u32,
    pub forward_hold_key: u32,
    pub forward_speed: f32,
    pub sidestep_command: u32,
    pub sidestep_hold_key: u32,
    pub sidestep_speed: f32,
    pub turn_command: u32,
    pub turn_hold_key: u32,
    pub turn_speed: f32,
    pub position: Position,
    /// bit0 = contact, bit1 = long jump.
    pub flags: u8,
}

impl MoveToState {
    pub const BODY_SIZE: usize = 89;

    /// Build the 89-byte body.
    #[must_use]
    pub fn body(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(self.packed_flags);
        w.u32(self.current_hold_key);
        w.u32(self.current_style);
        w.u32(self.forward_command);
        w.u32(self.forward_hold_key);
        w.f32(self.forward_speed);
        w.u32(self.sidestep_command);
        w.u32(self.sidestep_hold_key);
        w.f32(self.sidestep_speed);
        w.u32(self.turn_command);
        w.u32(self.turn_hold_key);
        w.f32(self.turn_speed);
        self.position.pack(&mut w);
        w.zeros(8); // 4 x u16
        w.u8(self.flags);
        w.out
    }
}

/// The body of the AutonomousPosition 0xF753 action: 32 B position + 4 u16 +
/// 1 u8 contact flag = 41 bytes.
#[must_use]
pub fn autonomous_position_body(pos: &Position, contact: bool) -> Vec<u8> {
    let mut w = Writer::new();
    pos.pack(&mut w);
    w.zeros(8); // 4 x u16
    w.u8(contact as u8);
    w.out
}

/// The body of the Jump 0xF61B action: f32 extent, f32 vel x/y/z, 4 u16, 2 u32
/// = 32 bytes.
#[must_use]
pub fn jump_body(extent: f32, vel: Vec3) -> Vec<u8> {
    let mut w = Writer::new();
    w.f32(extent);
    w.f32(vel.x);
    w.f32(vel.y);
    w.f32(vel.z);
    w.zeros(8); // 4 x u16
    w.u32(0);
    w.u32(0);
    w.out
}

/// S2C `Motion` (0xF74C) message: u32 guid, u16 instanceSeq, then a
/// MovementData in header form.
#[derive(Clone, Copy, Debug)]
pub struct MotionEvent {
    pub guid: u32,
    pub instance_seq: u16,
    /// The 8-bit movement type from the MovementData body.
    pub movement_type: u8,
}

/// Parse an S2C Motion message. The MovementData is only scanned far enough to
/// report the movement type (the radar relies on UpdatePosition for positions).
pub fn parse_motion(data: &[u8]) -> Option<MotionEvent> {
    if data.len() < 4 + 2 + 2 + 2 + 1 + 1 {
        return None;
    }
    let guid = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let instance_seq = u16::from_le_bytes([data[4], data[5]]);
    // u16 movementSeq (6), u16 serverControlSeq (8), u8 isAutonomous (10),
    // alignment to 11, u8 movementType (11).
    let movement_type = data[11];
    Some(MotionEvent { guid, instance_seq, movement_type })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_to_state_size() {
        let m = MoveToState {
            packed_flags: PACKED_FLAGS_ALL,
            current_hold_key: hold_key::RUN,
            current_style: MOTION_STYLE,
            forward_command: command::RUN_FORWARD,
            forward_hold_key: hold_key::RUN,
            forward_speed: 1.0,
            sidestep_command: 0,
            sidestep_hold_key: hold_key::INVALID,
            sidestep_speed: 1.0,
            turn_command: 0,
            turn_hold_key: hold_key::INVALID,
            turn_speed: 1.0,
            position: Position::default(),
            flags: 1,
        };
        assert_eq!(m.body().len(), MoveToState::BODY_SIZE);
    }

    #[test]
    fn autonomous_and_jump_sizes() {
        assert_eq!(autonomous_position_body(&Position::default(), true).len(), 41);
        assert_eq!(jump_body(0.1, Vec3::default()).len(), 32);
    }

    #[test]
    fn parse_motion_guid() {
        let mut d = vec![0u8; 24];
        d[0..4].copy_from_slice(&1234u32.to_le_bytes());
        d[4..6].copy_from_slice(&7u16.to_le_bytes());
        d[11] = 3; // MoveToPosition
        let e = parse_motion(&d).unwrap();
        assert_eq!(e.guid, 1234);
        assert_eq!(e.instance_seq, 7);
        assert_eq!(e.movement_type, 3);
    }

}
