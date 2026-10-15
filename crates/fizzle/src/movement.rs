//! Local movement simulation (RETAIL.md §3).
//!
//! A 50 ms semi-implicit Euler integrator over block-local coordinates. It
//! produces the C2S motion actions the server expects:
//!
//! * `MoveToState` (0xF61C) — sent whenever the discrete motion signature
//!   changes (forward/strafe/turn command + walk/run + grounded).
//! * `AutonomousPosition` (0xF753) — at most 1 Hz while moving.
//! * `Jump` (0xF61B) — on takeoff.
//!
//! The server is authoritative: it echoes our own `UpdatePosition` and we snap
//! when the divergence exceeds `CORRECTION_THRESHOLD` (or unconditionally after
//! a `PlayerTeleport`).

use std::time::Instant;

use acproto::codec::{Position, Quat, Vec3};
use acproto::motion::{
    autonomous_position_body, command, hold_key, jump_body, MoveToState, MOTION_STYLE,
    PACKED_FLAGS_ALL,
};

pub const TICK: f32 = 0.05; // 50 ms
const WALK_SPEED: f32 = 3.12;
const RUN_SPEED: f32 = 5.0;
const BACKWARD_MULT: f32 = 0.65;
const STRAFE_WALK: f32 = 1.56;
const STRAFE_RUN: f32 = 1.95;
const TURN_RATE: f32 = 1.5; // rad/s
const TURN_RUN_MULT: f32 = 1.5;
const VEL_CLAMP: f32 = 5.0;
const JUMP_VZ0: f32 = 2.75;
const GRAVITY: f32 = 9.8;
const AUTOPOS_INTERVAL: f32 = 1.0; // seconds
const CORRECTION_THRESHOLD: f32 = 0.25;
const PI: f32 = std::f32::consts::PI;

/// The discrete keys that affect the motion signature.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeldKeys {
    pub forward: bool,
    pub backward: bool,
    pub turn_left: bool,
    pub turn_right: bool,
    pub strafe_left: bool,
    pub strafe_right: bool,
    pub stop: bool,
    /// Edge-triggered jump request (consumed by the next `tick`).
    pub jump: bool,
}

impl HeldKeys {
    #[must_use]
    pub fn is_idle(&self) -> bool {
        !self.forward
            && !self.backward
            && !self.turn_left
            && !self.turn_right
            && !self.strafe_left
            && !self.strafe_right
            && !self.stop
    }
}

/// One motion action the session should send to the server.
#[derive(Clone, Debug)]
pub enum MoveAction {
    /// (action_type, body)
    Send(u32, Vec<u8>),
}

/// The motion signature: which discrete state we last reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MotionSig {
    forward: u32,
    sidestep: u32,
    turn: u32,
    hold_key: u32,
    grounded: bool,
}

/// The client-side character motion simulator.
#[derive(Clone, Debug)]
pub struct Movement {
    pub pos: Vec3,
    pub cell: u32,
    pub heading: f32,
    pub grounded: bool,
    pub vz: f32,
    ground_z: f32,
    last_sig: MotionSig,
    last_autopos: Option<Instant>,
    /// The last position we told the server (for correction comparison).
    last_sent: Vec3,
    pub moving: bool,
}

impl Movement {
    #[must_use]
    pub fn new(cell: u32, pos: Vec3) -> Self {
        let grounded = true;
        Self {
            pos,
            cell,
            heading: 0.0,
            grounded,
            vz: 0.0,
            ground_z: pos.z,
            last_sig: MotionSig::default(),
            last_autopos: None,
            last_sent: pos,
            moving: false,
        }
    }

    /// The current world position (block-local) + facing quaternion.
    #[must_use]
    pub fn current_position(&self) -> Position {
        let half = self.heading / 2.0;
        Position {
            cell: self.cell,
            pos: self.pos,
            rot: Quat { w: half.cos(), x: 0.0, y: 0.0, z: half.sin() },
        }
    }

    fn forward_vec(&self) -> (f32, f32) {
        (self.heading.sin(), self.heading.cos())
    }

    fn right_vec(&self) -> (f32, f32) {
        (self.heading.cos(), -self.heading.sin())
    }

    /// Build the discrete motion signature for the given inputs.
    fn signature(&self, held: &HeldKeys, walking: bool) -> MotionSig {
        let hold_key = if walking { hold_key::NONE } else { hold_key::RUN };
        let forward = if held.stop {
            command::READY
        } else if held.forward {
            if walking { command::WALK_FORWARD } else { command::RUN_FORWARD }
        } else if held.backward {
            command::WALK_BACKWARD
        } else {
            command::READY
        };
        let sidestep = if held.stop || held.strafe_right {
            if held.stop { command::READY } else { command::SIDESTEP_RIGHT }
        } else if held.strafe_left {
            command::SIDESTEP_LEFT
        } else {
            command::READY
        };
        let turn = if held.stop {
            command::READY
        } else if held.turn_right {
            command::TURN_RIGHT
        } else if held.turn_left {
            command::TURN_LEFT
        } else {
            command::READY
        };
        MotionSig { forward, sidestep, turn, hold_key, grounded: self.grounded }
    }

    fn build_move_to_state(&self, sig: &MotionSig, walking: bool) -> Vec<u8> {
        let speed = if walking { WALK_SPEED } else { RUN_SPEED };
        let strafe = if walking { STRAFE_WALK } else { STRAFE_RUN };
        let turn = if walking { TURN_RATE } else { TURN_RATE * TURN_RUN_MULT };
        MoveToState {
            packed_flags: PACKED_FLAGS_ALL,
            current_hold_key: sig.hold_key,
            current_style: MOTION_STYLE,
            forward_command: sig.forward,
            forward_hold_key: sig.hold_key,
            forward_speed: speed,
            sidestep_command: sig.sidestep,
            sidestep_hold_key: sig.hold_key,
            sidestep_speed: strafe,
            turn_command: sig.turn,
            turn_hold_key: sig.hold_key,
            turn_speed: turn,
            position: self.current_position(),
            flags: self.grounded as u8,
        }
        .body()
    }

    /// Advance the simulation by `dt` and return the actions to send.
    ///
    /// `jump` is edge-triggered and consumed. Call at the 50 ms cadence.
    pub fn tick(&mut self, dt: f32, held: &mut HeldKeys, walking: bool, now: Instant) -> Vec<MoveAction> {
        let mut out = Vec::new();

        let speed = if walking { WALK_SPEED } else { RUN_SPEED };
        let strafe = if walking { STRAFE_WALK } else { STRAFE_RUN };
        let turn = if walking { TURN_RATE } else { TURN_RATE * TURN_RUN_MULT };
        let (fx, fy) = self.forward_vec();
        let (rx, ry) = self.right_vec();

        let mut vx = 0.0f32;
        let mut vy = 0.0f32;
        let mut turning = 0.0f32;
        if !held.stop {
            if held.forward {
                vx += fx * speed;
                vy += fy * speed;
            }
            if held.backward {
                vx -= fx * speed * BACKWARD_MULT;
                vy -= fy * speed * BACKWARD_MULT;
            }
            if held.strafe_right {
                vx += rx * strafe;
                vy += ry * strafe;
            }
            if held.strafe_left {
                vx -= rx * strafe;
                vy -= ry * strafe;
            }
            if held.turn_right {
                turning += turn;
            }
            if held.turn_left {
                turning -= turn;
            }
        }
        let mag = (vx * vx + vy * vy).sqrt();
        if mag > VEL_CLAMP {
            vx = vx / mag * VEL_CLAMP;
            vy = vy / mag * VEL_CLAMP;
        }

        // Integrate heading.
        self.heading += turning * dt;
        while self.heading > PI {
            self.heading -= 2.0 * PI;
        }
        while self.heading < -PI {
            self.heading += 2.0 * PI;
        }

        // Integrate horizontal position.
        self.pos.x += vx * dt;
        self.pos.y += vy * dt;

        // Jump + gravity.
        if held.jump && self.grounded {
            self.vz = JUMP_VZ0;
            self.grounded = false;
            self.ground_z = self.pos.z;
            held.jump = false;
            let vel = Vec3 { x: vx, y: vy, z: JUMP_VZ0 };
            out.push(MoveAction::Send(0xF61B, jump_body(0.0, vel)));
        }
        if !self.grounded {
            self.vz -= GRAVITY * dt;
            self.pos.z += self.vz * dt;
            if self.pos.z <= self.ground_z {
                self.pos.z = self.ground_z;
                self.vz = 0.0;
                self.grounded = true;
            }
        }

        self.moving = !held.is_idle() || !self.grounded;

        // Send MoveToState when the signature changed.
        let sig = self.signature(held, walking);
        if sig != self.last_sig {
            out.push(MoveAction::Send(0xF61C, self.build_move_to_state(&sig, walking)));
            self.last_sig = sig;
        }

        // Send AutonomousPosition at most 1 Hz while moving.
        if self.moving {
            let due = match self.last_autopos {
                Some(t) => now.duration_since(t).as_secs_f32() >= AUTOPOS_INTERVAL,
                None => true,
            };
            if due {
                out.push(MoveAction::Send(0xF753, autonomous_position_body(&self.current_position(), self.grounded)));
                self.last_sent = self.pos;
                self.last_autopos = Some(now);
            }
        }

        out
    }

    /// Adopt a server correction. Returns true if a fresh `MoveToState` is
    /// forced (signature cleared).
    ///
    /// * `force` (after `PlayerTeleport`): snap unconditionally.
    /// * Else if `|recv - last_sent| > 0.25`: snap (adopt cell).
    /// * Else: keep local position but adopt the cell.
    pub fn apply_correction(&mut self, cell: u32, pos: &Vec3, force: bool) -> bool {
        let dist = ((self.last_sent.x - pos.x).powi(2)
            + (self.last_sent.y - pos.y).powi(2)
            + (self.last_sent.z - pos.z).powi(2))
            .sqrt();
        let snap = force || dist > CORRECTION_THRESHOLD;
        if snap {
            self.pos = *pos;
            self.cell = cell;
            self.ground_z = pos.z;
            self.vz = 0.0;
            self.grounded = true;
            self.last_autopos = None;
            // Force a fresh MoveToState on the next tick.
            self.last_sig = MotionSig::default();
            self.last_sent = *pos;
            return true;
        }
        self.cell = cell;
        false
    }
}
