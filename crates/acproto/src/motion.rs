//! Motion: constants, C2S movement-action body builders (MoveToState,
//! AutonomousPosition, Jump), and the S2C `Motion` (0xF74C) message layout.
//!
//! Layouts verified against `ACE.Server.Network.Motion` @ 47edade and
//! `docs/PROTOCOL.md`.

use crate::codec::{Position, Reader, Vec3, Writer};
use crate::messages::cmd_from_u16;

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
    /// Crouched (rides the forward command field; MovementCommand substate).
    pub const CROUCH: u32 = 0x4100_0012;
    /// Sitting (substate).
    pub const SITTING: u32 = 0x4100_0013;
    /// Sleeping (substate).
    pub const SLEEPING: u32 = 0x4100_0014;
    /// Door opened (MotionCommand::On, rides the forward command field).
    pub const DOOR_ON: u32 = 0x4000_000B;
    /// Door closed (MotionCommand::Off).
    pub const DOOR_OFF: u32 = 0x4000_000C;
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

/// Packed flags for a MoveToState carrying one command-list entry
/// (`CommandListLength` = PackedFlags >> 11): all 11 motion fields plus a
/// single MotionItem.
pub const PACKED_FLAGS_WITH_COMMAND: u32 = PACKED_FLAGS_ALL | (1 << 11);

/// The body of a MoveToState (0xF61C) that performs one soul emote: the 11
/// idle motion fields, a single command-list item (`u16 rawCommand, u16
/// packedSequence, f32 speed`), then position + 4 u16 + 1 u8 = 97 bytes.
///
/// The server only accepts commands from its SoulEmotes set at exactly
/// speed 1.0 (anything else is logged as a non-standard action).
/// `seq` is the 15-bit motion sequence; the high (autonomous) bit is set
/// here because the client initiated it.
#[must_use]
pub fn soul_emote_body(pos: &Position, contact: bool, raw_command: u16, seq: u16) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(PACKED_FLAGS_WITH_COMMAND);
    w.u32(hold_key::NONE);
    w.u32(MOTION_STYLE);
    w.u32(command::READY);
    w.u32(hold_key::NONE);
    w.f32(0.0);
    w.u32(command::READY); // sidestep
    w.u32(hold_key::NONE);
    w.f32(0.0);
    w.u32(command::READY); // turn
    w.u32(hold_key::NONE);
    w.f32(0.0);
    // Command list: one MotionItem.
    w.u16(raw_command);
    w.u16(seq | 0x8000);
    w.f32(1.0);
    pos.pack(&mut w);
    w.zeros(8); // 4 x u16
    w.u8(contact as u8);
    w.out
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
#[derive(Clone, Debug)]
pub struct MotionEvent {
    pub guid: u32,
    pub instance_seq: u16,
    /// The 8-bit movement type from the MovementData body.
    pub movement_type: u8,
    /// The motion stance (`MotionStance`) low-16, e.g. NonCombat `0x003d`,
    /// SwordCombat `0x003e`, BowCombat `0x003f`, Magic `0x0049`. Always
    /// written in the header form; `0` if the body is too short to hold it.
    /// This is the value that switches the motion table to the combat cycles.
    pub style: u16,
    /// The parsed MotionCommands of the MovementData (movement type 0 only):
    /// the forward/sidestep/turn command plus the command list, each expanded
    /// to its full 32-bit `MotionCommand` id. Empty for other movement types.
    /// A door's open/close rides the forward command (`command::DOOR_ON` /
    /// `command::DOOR_OFF`).
    pub commands: Vec<u32>,
}

/// Parse an S2C Motion message. The MovementData is scanned far enough to
/// collect the parsed command list (door state); positions come from
/// UpdatePosition.
pub fn parse_motion(data: &[u8]) -> Option<MotionEvent> {
    if data.len() < 14 {
        return None;
    }
    let guid = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let instance_seq = u16::from_le_bytes([data[4], data[5]]);
    // u16 movementSeq (6), u16 serverControlSeq (8), u8 isAutonomous (10),
    // 1 pad byte (11), u8 movementType (12), u8 motionFlags (13), u16
    // currentStyle (14-15), then the union by movementType (16+). Layout
    // verified against the retail client's `unpack_movement` +
    // `InterpretedMotionState::UnPack`.
    let movement_type = data[12];
    let motion_flags = data[13];
    // The stance (low 16 bits of the MotionStance) is always written.
    let style = u16::from_le_bytes([data[14], data[15]]);
    let mut commands = Vec::new();
    // MovementData body (movement type 0 = General/Invalid): the packed
    // command flags + fields, then the command list.
    if movement_type == 0 && data.len() >= 20 {
        let mut r = Reader::new(&data[16..]);
        if r.remaining() >= 4 {
            let f = r.u32();
            let n_cmds = ((f >> 7) & 0x1f) as usize;
            // Field order matches the retail client's UnPack exactly.
            if f & 0x1 != 0 && r.remaining() >= 2 {
                r.u16(); // currentStyle (repeated, gated)
            }
            if f & 0x2 != 0 && r.remaining() >= 2 {
                let c = cmd_from_u16(r.u16() as u32);
                if c != 0 {
                    commands.push(c); // forwardCommand
                }
            }
            if f & 0x8 != 0 && r.remaining() >= 2 {
                let c = cmd_from_u16(r.u16() as u32);
                if c != 0 {
                    commands.push(c); // sidestepCommand
                }
            }
            if f & 0x20 != 0 && r.remaining() >= 2 {
                let c = cmd_from_u16(r.u16() as u32);
                if c != 0 {
                    commands.push(c); // turnCommand
                }
            }
            if f & 0x4 != 0 && r.remaining() >= 4 {
                r.f32(); // forwardSpeed
            }
            if f & 0x10 != 0 && r.remaining() >= 4 {
                r.f32(); // sidestepSpeed
            }
            if f & 0x40 != 0 && r.remaining() >= 4 {
                r.f32(); // turnSpeed
            }
            for _ in 0..n_cmds {
                if r.remaining() < 8 {
                    break;
                }
                let c = cmd_from_u16(r.u16() as u32); // command
                if c != 0 {
                    commands.push(c);
                }
                r.u16(); // packedSeq
                r.f32(); // speed
            }
            r.align4();
            if motion_flags & 0x1 != 0 && r.remaining() >= 4 {
                r.u32(); // stickyGuid
            }
        }
    }
    Some(MotionEvent { guid, instance_seq, movement_type, style, commands })
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
        d[12] = 3; // MoveToPosition
        d[14..16].copy_from_slice(&0x003du16.to_le_bytes()); // NonCombat stance
        let e = parse_motion(&d).unwrap();
        assert_eq!(e.guid, 1234);
        assert_eq!(e.instance_seq, 7);
        assert_eq!(e.movement_type, 3);
        assert_eq!(e.style, 0x003d);
    }

    #[test]
    fn parse_motion_door_commands() {
        // Movement type 0 (MoveToState) with the forward command = On (door
        // open): flags f = 0x2 (hasForwardCommand), forward u16 = 0x000b.
        // NonCombat stance (0x003d) is always written at data[14-15].
        let mut d = vec![0u8; 24];
        d[0..4].copy_from_slice(&0x0028_0001u32.to_le_bytes());
        d[4..6].copy_from_slice(&5u16.to_le_bytes());
        d[12] = 0; // movement type 0
        d[14..16].copy_from_slice(&0x003du16.to_le_bytes()); // NonCombat
        d[16..20].copy_from_slice(&0x2u32.to_le_bytes()); // f
        d[20..22].copy_from_slice(&0x000b_u16.to_le_bytes()); // forward = On
        let e = parse_motion(&d).unwrap();
        assert_eq!(e.guid, 0x0028_0001);
        assert_eq!(e.movement_type, 0);
        assert_eq!(e.style, 0x003d);
        assert_eq!(e.commands, vec![command::DOOR_ON]);

        // Off (door closed).
        d[20..22].copy_from_slice(&0x000c_u16.to_le_bytes());
        let e = parse_motion(&d).unwrap();
        assert_eq!(e.commands, vec![command::DOOR_OFF]);

        // Non-0 movement type yields an empty command list.
        d[12] = 3;
        let e = parse_motion(&d).unwrap();
        assert!(e.commands.is_empty());
    }

    #[test]
    fn parse_motion_combat_stance_and_command() {
        // SwordCombat stance (0x003e) with a command list carrying one melee
        // attack (ThrustMed, low-16 0x0058) at speed 1.0. The stance and the
        // raw attack code must both survive the parse (cmd_from_u16 passes
        // unmapped combat codes through).
        let mut d = vec![0u8; 32];
        d[0..4].copy_from_slice(&0x0000_00aa_u32.to_le_bytes());
        d[12] = 0; // movement type 0
        d[14..16].copy_from_slice(&0x003eu16.to_le_bytes()); // SwordCombat
        // packed: f = 0x2 (forward present) | (1 << 7) (one command) = 0x82.
        d[16..20].copy_from_slice(&0x82u32.to_le_bytes());
        d[20..22].copy_from_slice(&0x0003_u16.to_le_bytes()); // forward = Ready
        // Command item: cmd 0x0058 (ThrustMed), packedSeq 0x0001, speed 1.0.
        d[22..24].copy_from_slice(&0x0058_u16.to_le_bytes());
        d[24..26].copy_from_slice(&0x0001_u16.to_le_bytes());
        d[26..30].copy_from_slice(&1.0f32.to_le_bytes());
        let e = parse_motion(&d).unwrap();
        assert_eq!(e.style, 0x003e);
        // Ready (forward) + ThrustMed (command list, passed through raw).
        assert_eq!(e.commands, vec![0x4100_0003, 0x0058]);
    }

    #[test]
    fn soul_emote_body_layout() {
        let b = soul_emote_body(&Position::default(), true, 0x00F0, 37);
        assert_eq!(b.len(), 97); // 89-byte MoveToState + 8-byte MotionItem
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), PACKED_FLAGS_WITH_COMMAND);
        assert_eq!(r.u32(), hold_key::NONE);
        assert_eq!(r.u32(), MOTION_STYLE);
        assert_eq!(r.u32(), command::READY);
        assert_eq!(r.u32(), hold_key::NONE);
        assert!((r.f32() - 0.0).abs() < f32::EPSILON);
        assert_eq!(r.u32(), command::READY);
        assert_eq!(r.u32(), hold_key::NONE);
        assert!((r.f32() - 0.0).abs() < f32::EPSILON);
        assert_eq!(r.u32(), command::READY);
        assert_eq!(r.u32(), hold_key::NONE);
        assert!((r.f32() - 0.0).abs() < f32::EPSILON);
        // Command list item.
        assert_eq!(r.u16(), 0x00F0); // Point
        assert_eq!(r.u16(), 37 | 0x8000); // autonomous bit
        assert!((r.f32() - 1.0).abs() < f32::EPSILON);
        // Position (32) + 4 u16 + contact.
        assert_eq!(r.remaining(), 41);
        let pos = Position::unpack(&mut r);
        assert_eq!(pos, Position::default());
        assert_eq!(r.remaining(), 9);
        for _ in 0..4 {
            assert_eq!(r.u16(), 0);
        }
        assert_eq!(r.u8(), 1);
    }

}
