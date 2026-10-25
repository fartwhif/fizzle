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
use acgeo::{region_contains_xy, WorldProbe};
use acproto::codec::{Position, Quat, Vec3};
use acproto::motion::{
    autonomous_position_body, command, hold_key, jump_body, MoveToState,
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
const SUBSTEP: f32 = 0.4; // max distance per collision sub-step
/// Horizontal margin (units) before a position counts as outside an env cell.
const ENV_EXIT_MARGIN: f32 = 0.25;
const PI: f32 = std::f32::consts::PI;

/// Normalize a block-local coordinate into `[0, 192)`: server positions
/// (saved rows, teleports) may carry an unwrapped form (the shard holds
/// e.g. y = -28.48 for a 0x8602-block position), while all cell/region
/// math runs in the block frame.
#[must_use]
fn block_wrap(v: f32) -> f32 {
    v.rem_euclid(192.0)
}

/// Block-local `(cell, x, y)` → global (unwrapped) horizontal coordinates.
/// The cell's 16-bit landblock encodes the block grid (high byte = x, low
/// byte = y); a block is 192 u on a side (the same frame
/// `adjust_to_outside` uses for its carries).
#[must_use]
pub fn global_xy(cell: u32, x: f32, y: f32) -> (f32, f32) {
    let bx = ((cell >> 16) & 0xFF00) >> 8;
    let by = (cell >> 16) & 0xFF;
    (bx as f32 * 192.0 + x, by as f32 * 192.0 + y)
}

/// One approach tick decision (the server-issued `MoveToObject` approach:
/// turn in place until facing the target, then walk until within
/// `stop_distance`). `dx`/`dy`/`dist` are the horizontal offset from the
/// player to the target in global coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ApproachStep {
    pub turn_left: bool,
    pub turn_right: bool,
    pub forward: bool,
    /// Within the stop distance: clear the approach.
    pub arrived: bool,
}

/// Stop early by at most one run-step (5 u × 0.05 s) so the last
/// integration step cannot overshoot the use radius.
const APPROACH_ARRIVAL_EPS: f32 = 0.25;
/// Past this yaw error the approach turns in place (no forward drift).
const APPROACH_TURN_SNAP: f32 = 0.15;
/// Below this yaw error no correction turn is issued (≈ one 50 ms
/// walk-turn increment, avoiding a left/right oscillation at the boundary).
const APPROACH_TURN_DEAD: f32 = 0.075;

#[must_use]
pub fn approach_step(heading: f32, dx: f32, dy: f32, dist: f32, stop_distance: f32) -> ApproachStep {
    if dist <= stop_distance + APPROACH_ARRIVAL_EPS {
        return ApproachStep { turn_left: false, turn_right: false, forward: false, arrived: true };
    }
    // `forward_vec = (sin h, cos h)` → the heading facing (dx, dy) is
    // atan2(dx, dy).
    let mut delta = dx.atan2(dy) - heading;
    while delta > PI {
        delta -= 2.0 * PI;
    }
    while delta < -PI {
        delta += 2.0 * PI;
    }
    if delta.abs() > APPROACH_TURN_SNAP {
        // Turn in place until roughly facing.
        return ApproachStep { turn_left: delta < 0.0, turn_right: delta > 0.0, forward: false, arrived: false };
    }
    ApproachStep {
        turn_left: delta < -APPROACH_TURN_DEAD,
        turn_right: delta > APPROACH_TURN_DEAD,
        forward: true,
        arrived: false,
    }
}

/// The retail `adjust_to_outside` (called by `store_position` for land
/// cells): recompute the land cell id (0x00xx) from block-local `(x, y)` and
/// wrap `(x, y)` into `[0, 192)` with a block carry (negative coordinates
/// carry into the adjacent block). Pure math, no dats. Out of Dereth's land
/// (global cell coords outside 0..2040) the cell is returned unchanged.
#[must_use]
pub fn adjust_to_outside(cell: u32, x: &mut f32, y: &mut f32) -> u32 {
    const CELL_LEN: f32 = 24.0;
    const BLOCK_LEN: f32 = 192.0;
    const LAND_LEN: i32 = 2040; // 85 blocks of 24u cells
    let bx = ((cell >> 16) & 0xFF00) >> 8;
    let by = (cell >> 16) & 0xFF;
    let gx = bx as i32 * 8 + (*x / CELL_LEN).floor() as i32;
    let gy = by as i32 * 8 + (*y / CELL_LEN).floor() as i32;
    if !(0..LAND_LEN).contains(&gx) || !(0..LAND_LEN).contains(&gy) {
        return cell;
    }
    if x.abs() < 0.0002 {
        *x = 0.0;
    }
    if y.abs() < 0.0002 {
        *y = 0.0;
    }
    *x -= (*x / BLOCK_LEN).floor() * BLOCK_LEN;
    *y -= (*y / BLOCK_LEN).floor() * BLOCK_LEN;
    let block = (((gx >> 3) << 8) | (gy >> 3)) as u32;
    let idx = (1 + ((gx & 7) << 3) + (gy & 7)) as u32;
    (block << 16) | idx
}

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
        let pos = Vec3 { x: block_wrap(pos.x), y: block_wrap(pos.y), z: pos.z };
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

    /// The current world position (block-local) + facing quaternion. The
    /// vertical rotation is -heading (ACE yaw: a heading of h is a rotation of
    /// -h about the vertical axis; +h is +Z -> +X, matching `forward_vec`).
    #[must_use]
    pub fn current_position(&self) -> Position {
        let half = self.heading / 2.0;
        Position {
            cell: self.cell,
            pos: self.pos,
            rot: Quat { w: half.cos(), x: 0.0, y: 0.0, z: -half.sin() },
        }
    }

    /// The dominant retail motion command the sim is currently in, for the 3D
    /// self-actor's animation (the retail MCMD space). Walking outranks
    /// strafing and turning (a curved "turn while moving" still plays the
    /// walk); a lone turn or strafe falls through to its own command. Running
    /// emits `RUN_FORWARD`: the motion table defines a distinct run cycle per
    /// stance (separate from the walk), so a running combat stance plays the
    /// run animation and timing rather than the walk-forward nudge.
    #[must_use]
    pub fn motion_cmd(&self) -> u32 {
        let s = &self.last_sig;
        if s.forward != command::READY {
            return s.forward;
        }
        if s.sidestep != command::READY {
            return s.sidestep;
        }
        if s.turn != command::READY {
            return s.turn;
        }
        command::READY
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

    fn build_move_to_state(&self, sig: &MotionSig, walking: bool, style: u32) -> Vec<u8> {
        let speed = if walking { WALK_SPEED } else { RUN_SPEED };
        let strafe = if walking { STRAFE_WALK } else { STRAFE_RUN };
        let turn = if walking { TURN_RATE } else { TURN_RATE * TURN_RUN_MULT };
        MoveToState {
            packed_flags: PACKED_FLAGS_ALL,
            current_hold_key: sig.hold_key,
            current_style: style,
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
    /// `probe` supplies terrain height + solid/door blocking; `None` keeps the
    /// flat-plane behavior (no retail dats).
    pub fn tick(&mut self, dt: f32, held: &mut HeldKeys, walking: bool, now: Instant, mut probe: Option<&mut dyn WorldProbe>, style: u32) -> Vec<MoveAction> {
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

        // Integrate horizontal position (sub-stepped against the probe so a
        // long step cannot tunnel a thin wall).
        let (dx, dy) = (vx * dt, vy * dt);
        let dist = (dx * dx + dy * dy).sqrt();
        if dist > 0.0 {
            let n = if probe.is_some() {
                ((dist / SUBSTEP).ceil()).max(1.0) as usize
            } else {
                1
            };
            let (sx, sy) = (dx / n as f32, dy / n as f32);
            for _ in 0..n {
                self.step_once(sx, sy, &mut probe);
            }
        }

        // Terrain following + vertical (the probe's terrain is the ground).
        if let Some(g) = probe.as_mut().and_then(|p| p.height_at(self.pos.x, self.pos.y)) {
            self.ground_z = g;
        }
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
            if self.pos.z <= self.ground_z && self.vz <= 0.0 {
                self.pos.z = self.ground_z;
                self.vz = 0.0;
                self.grounded = true;
            }
        } else if probe.is_some() {
            self.pos.z = self.ground_z;
        }

        // Keep (cell, position) consistent so our saves spawn (the server
        // relocates to the lifestone when placement fails).
        self.maintain_cell(&mut probe);

        self.moving = !held.is_idle() || !self.grounded;

        // Send MoveToState when the signature changed. It carries our current
        // position (the server adopts and echoes it), so it refreshes
        // `last_sent` — otherwise the echo of a `MoveToState` sent up to 1 s
        // after the last `AutonomousPosition` reads as a correction.
        let sig = self.signature(held, walking);
        if sig != self.last_sig {
            out.push(MoveAction::Send(0xF61C, self.build_move_to_state(&sig, walking, style)));
            self.last_sig = sig;
            self.last_sent = self.pos;
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

    /// Keep the `(cell, position)` pair consistent — the invariant the
    /// server relies on to place us at login (a mismatched save cannot be
    /// spawned and we get relocated to the lifestone).
    ///
    /// * Land cell (< 0x100): recompute from the position
    ///   (`adjust_to_outside`), then switch to an env cell when the position
    ///   is inside one (probe).
    /// * Env cell (>= 0x100): switch to the land cell once the position
    ///   leaves the cell's region (probe).
    ///
    /// The check is horizontal (x, y): indoors the sim's z is the outdoor
    /// terrain height and is unreliable for containment.
    fn maintain_cell(&mut self, probe: &mut Option<&mut dyn WorldProbe>) {
        // Land vs env is the low 16 bits (cell number): land cells are
        // 0x0001..0x00FF, env cells 0x0100..0xFFFD (retail `cellidx < 0x100`).
        if (self.cell & 0xFFFF) < 0x100 {
            self.cell = adjust_to_outside(self.cell, &mut self.pos.x, &mut self.pos.y);
            let Some(p) = probe.as_mut() else {
                return;
            };
            let lb = self.cell >> 16;
            if let Some(env) = p.env_cell_at(lb, self.pos.x, self.pos.y) {
                self.cell = env;
            }
        } else {
            let Some(p) = probe.as_mut() else {
                return;
            };
            let lb = self.cell >> 16;
            if let Some(b) = p.env_cell_region(lb, self.cell & 0xFFFF) {
                if !region_contains_xy(&b, self.pos.x, self.pos.y, ENV_EXIT_MARGIN) {
                    self.cell = adjust_to_outside(lb << 16, &mut self.pos.x, &mut self.pos.y);
                }
            }
        }
    }

    /// Advance one collision sub-step; a solid blocks the full step, then each
    /// axis separately (the player slides along a wall instead of stopping
    /// dead). With no probe this is a plain horizontal step.
    fn step_once(&mut self, sx: f32, sy: f32, probe: &mut Option<&mut dyn WorldProbe>) {
        if probe.is_none() {
            self.pos.x += sx;
            self.pos.y += sy;
            return;
        }
        let p = probe.as_mut().expect("checked above");
        let (x0, y0, z0) = (self.pos.x, self.pos.y, self.pos.z);
        let (tx, ty) = (x0 + sx, y0 + sy);
        let gt = p.height_at(tx, ty).unwrap_or(z0);
        if !p.blocked(x0, y0, z0, tx, ty, gt) {
            self.pos.x = tx;
            self.pos.y = ty;
        } else {
            let gx = p.height_at(tx, y0).unwrap_or(z0);
            if !p.blocked(x0, y0, z0, tx, y0, gx) {
                self.pos.x = tx;
            } else {
                let gy = p.height_at(x0, ty).unwrap_or(z0);
                if !p.blocked(x0, y0, z0, x0, ty, gy) {
                    self.pos.y = ty;
                }
            }
        }
        if self.grounded {
            if let Some(g) = p.height_at(self.pos.x, self.pos.y) {
                self.pos.z = g;
                self.ground_z = g;
            }
        }
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
            // Server positions may be unwrapped (see `block_wrap`); adopt
            // them in the block frame.
            let pos = Vec3 { x: block_wrap(pos.x), y: block_wrap(pos.y), z: pos.z };
            self.pos = pos;
            self.cell = cell;
            self.ground_z = pos.z;
            self.vz = 0.0;
            self.grounded = true;
            self.last_autopos = None;
            // Force a fresh MoveToState on the next tick.
            self.last_sig = MotionSig::default();
            self.last_sent = pos;
            return true;
        }
        self.cell = cell;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{adjust_to_outside, approach_step, global_xy, HeldKeys, Movement, MoveAction, TICK};
    use acproto::codec::Vec3;
    use acgeo::WorldProbe;
    use acproto::motion::command;
    use std::time::Instant;

    fn mv() -> Movement {
        Movement::new(0, Vec3 { x: 0.0, y: 0.0, z: 0.0 })
    }

    fn drive(m: &mut Movement, held: HeldKeys, walking: bool) -> Vec<MoveAction> {
        let mut h = held;
        m.tick(TICK, &mut h, walking, Instant::now(), None, acproto::motion::MOTION_STYLE)
    }

    #[test]
    fn motion_cmd_walks_forward() {
        let mut m = mv();
        drive(&mut m, HeldKeys { forward: true, ..Default::default() }, true);
        assert_eq!(m.motion_cmd(), command::WALK_FORWARD);
    }

    #[test]
    fn motion_cmd_run_emits_run_forward() {
        let mut m = mv();
        // Running emits RUN_FORWARD so the renderer plays the stance's
        // distinct run cycle (not the walk-forward nudge).
        drive(&mut m, HeldKeys { forward: true, ..Default::default() }, false);
        assert_eq!(m.motion_cmd(), command::RUN_FORWARD);
    }

    #[test]
    fn motion_cmd_turns_right() {
        let mut m = mv();
        drive(&mut m, HeldKeys { turn_right: true, ..Default::default() }, true);
        assert_eq!(m.motion_cmd(), command::TURN_RIGHT);
    }

    #[test]
    fn motion_cmd_prefers_forward_over_turn() {
        let mut m = mv();
        drive(&mut m, HeldKeys { forward: true, turn_right: true, ..Default::default() }, true);
        assert_eq!(m.motion_cmd(), command::WALK_FORWARD);
    }

    #[test]
    fn motion_cmd_idle_is_ready() {
        let mut m = mv();
        drive(&mut m, HeldKeys::default(), true);
        assert_eq!(m.motion_cmd(), command::READY);
    }

    /// Regression: `MoveToState` must carry the character's *current* motion
    /// stance (the `style` argument), not a fixed NonCombat — otherwise moving
    /// while in combat drops the self back to the peace stance (the server
    /// echoes the sent style back via S2C Motion).
    #[test]
    fn move_to_state_carries_current_stance() {
        let mut m = mv();
        let actions = m.tick(TICK, &mut HeldKeys { forward: true, ..Default::default() }, true, Instant::now(), None, 0x8000003E);
        let body = actions
            .iter()
            .find_map(|a| match a {
                MoveAction::Send(t, b) if *t == 0xF61C => Some(b.as_slice()),
                _ => None,
            })
            .expect("starting to walk sends a MoveToState");
        // current_style is the 3rd u32 (offset 8): packed_flags, hold_key, style.
        let style = u32::from_le_bytes([body[8], body[9], body[10], body[11]]);
        assert_eq!(style, 0x8000003E, "MoveToState must carry the current stance, not NonCombat");
    }

    /// Regression: `MoveToState` carries our current position, and the server
    /// adopts and echoes it back via the self `UpdatePosition` (PROTOCOL.md
    /// §20.6). `last_sent` must therefore track *every* position we send, not
    /// just the 1 Hz `AutonomousPosition`. If it lags, the echo of a
    /// `MoveToState` position — up to a full second (≈5 u at run speed) behind
    /// the last `AutonomousPosition` — reads as a server correction and the
    /// character snaps backward on every direction change while moving.
    #[test]
    fn move_to_state_echo_is_not_a_correction() {
        let mut m = mv();
        // Start walking: a fresh `MoveToState` and an immediate
        // `AutonomousPosition` go out from the first integrated step.
        drive(&mut m, HeldKeys { forward: true, ..Default::default() }, true);
        let first_sent = m.pos;
        // Walk another 0.5 s (10 ticks) without a signature change: the 1 Hz
        // `AutonomousPosition` has not re-fired, so the last *sent* position
        // is still `first_sent` (~1.5 u behind the live sim).
        for _ in 0..10 {
            drive(&mut m, HeldKeys { forward: true, ..Default::default() }, true);
        }
        // Turn: the signature changes, so a `MoveToState` goes out carrying
        // the current position, and the server will echo exactly that back.
        let sent = m.pos;
        let dist = ((sent.x - first_sent.x).powi(2) + (sent.y - first_sent.y).powi(2)).sqrt();
        assert!(dist > 0.25, "the echo must be farther than the snap threshold from the stale position");
        drive(&mut m, HeldKeys { turn_right: true, ..Default::default() }, true);

        // The echo of our own `MoveToState` position must be classified as an
        // echo, not a correction: no snap, no forced re-broadcast.
        assert!(
            !m.apply_correction(m.cell, &sent, false),
            "echo of our own MoveToState position must not be treated as a server correction"
        );
        assert_eq!(m.pos, sent, "an echo must not move the sim");
        assert_eq!(m.motion_cmd(), command::TURN_RIGHT, "an echo must not clear the motion signature");
    }

    /// The retail `adjust_to_outside` fixtures: the first four are real
    /// saved (cell, position) rows from the shard (Arwic + the lifestone
    /// block); the rest exercise the block carry, including negative
    /// coordinates (verified against the server's re-normalization of
    /// `[62.26, -15.43] @ 0xC6A9` to `[62.26, 176.57] @ 0xC6A8`).
    #[test]
    fn adjust_to_outside_fixtures() {
        let cases = [
            // (cell, x, y) -> (expected cell, expected x, expected y)
            (0xC6A9000A, 45.5635, 25.5546, 0xC6A9000A, 45.5635, 25.5546),
            (0xC6A90009, 46.805, 4.219, 0xC6A90009, 46.805, 4.219),
            (0xC6A90012, 58.8723, 37.8527, 0xC6A90012, 58.8723, 37.8527),
            (0xC6A80018, 62.26, 176.57, 0xC6A80018, 62.26, 176.57),
            (0xC6A90012, 62.26, -15.43, 0xC6A80018, 62.26, 176.57),
            (0xC6A90012, 193.5, 10.0, 0xC7A90001, 1.5, 10.0),
            (0xC6A90001, -1.0, 10.0, 0xC5A90039, 191.0, 10.0),
        ];
        for (cell, x0, y0, want_cell, want_x, want_y) in cases {
            let (mut x, mut y) = (x0, y0);
            let got = adjust_to_outside(cell, &mut x, &mut y);
            assert_eq!(got, want_cell, "cell for {cell:#x} [{x0}, {y0}]");
            assert!((x - want_x).abs() < 0.001, "x for {cell:#x} [{x0}, {y0}]: {x}");
            assert!((y - want_y).abs() < 0.001, "y for {cell:#x} [{x0}, {y0}]: {y}");
        }
    }

    /// Out of Dereth's land the cell is returned unchanged (no wrap, no
    /// recompute) so the sim does not invent a block at the world edge.
    #[test]
    fn adjust_to_outside_out_of_land_is_noop() {
        let (mut x, mut y) = (-300.0f32, 10.0f32);
        let got = adjust_to_outside(0x0001_0001, &mut x, &mut y);
        assert_eq!(got, 0x0001_0001);
        assert_eq!((x, y), (-300.0, 10.0));
    }

    /// A fake probe with one env cell region (block 0x8602, cell 0x01AD).
    struct Fake {
        region: [f32; 6],
        env: u32,
    }

    impl WorldProbe for Fake {
        fn height_at(&mut self, _x: f32, _y: f32) -> Option<f32> {
            None
        }
        fn blocked(&mut self, _x0: f32, _y0: f32, _z0: f32, _x1: f32, _y1: f32, _z1: f32) -> bool {
            false
        }
        fn env_cell_region(&mut self, _lb: u32, _cell: u32) -> Option<[f32; 6]> {
            Some(self.region)
        }
        fn env_cell_at(&mut self, lb: u32, x: f32, y: f32) -> Option<u32> {
            (self.region[0] <= x && x <= self.region[3] && self.region[1] <= y && y <= self.region[4])
                .then(|| (lb << 16) | self.env)
        }
    }

    /// Regression: the lifestone respawn loop. The saved row held the env
    /// cell 0x01AD while the position was outside the structure, so the
    /// server could not spawn it and relocated the character to the
    /// lifestone on every login. Leaving the env cell's region must switch
    /// to the land cell (and keep the pair consistent).
    #[test]
    fn env_cell_exit_switches_to_land() {
        // Inside the region: the env cell is kept.
        let mut m = Movement::new(0x860201AD, Vec3 { x: 12.32, y: 163.52, z: 0.0 });
        let mut fake = Fake { region: [10.0, 160.0, -1.0, 15.0, 167.0, 3.0], env: 0x01AD };
        m.tick(TICK, &mut HeldKeys::default(), true, Instant::now(), Some(&mut fake), acproto::motion::MOTION_STYLE);
        assert_eq!(m.cell, 0x860201AD, "position inside the region keeps the env cell");
        // Step outside the region: the cell must become the land cell for
        // (30, 100) in block 0x8602 (cell 0x000D) and stay consistent with
        // the position.
        m.pos = Vec3 { x: 30.0, y: 100.0, z: 0.0 };
        let mut fake = Fake { region: [10.0, 160.0, -1.0, 15.0, 167.0, 3.0], env: 0x01AD };
        m.tick(TICK, &mut HeldKeys::default(), true, Instant::now(), Some(&mut fake), acproto::motion::MOTION_STYLE);
        assert_eq!(m.cell, 0x8602000D, "leaving the region switches to the land cell");
    }

    /// The inverse: on a land cell, entering an env cell's region switches to
    /// that env cell (indoor positions must carry the env cell id or the
    /// server cannot place them).
    #[test]
    fn land_cell_entry_switches_to_env() {
        let mut m = Movement::new(0x8602000D, Vec3 { x: 30.0, y: 100.0, z: 0.0 });
        let mut fake = Fake { region: [28.0, 98.0, 0.0, 35.0, 105.0, 3.0], env: 0x01AD };
        m.tick(TICK, &mut HeldKeys::default(), true, Instant::now(), Some(&mut fake), acproto::motion::MOTION_STYLE);
        assert_eq!(m.cell, 0x860201AD, "entering the region switches to the env cell");
    }

    /// No probe (dats unavailable): the cell math still runs for land cells
    /// — the stale-cell save was the bug: a position that sits in a
    /// different land cell than the adopted one must move the cell.
    #[test]
    fn no_probe_still_recomputes_land_cell() {
        let mut m = Movement::new(0xC6A9000A, Vec3 { x: 58.87, y: 37.85, z: 0.0 });
        drive(&mut m, HeldKeys::default(), true);
        assert_eq!(m.cell, 0xC6A90012);
    }

    /// Saved rows may carry unwrapped block-local coordinates (the user's
    /// lifestone row is y = -28.48 in block 0x8602); world entry must
    /// adopt them in the block frame so region math works from tick one.
    #[test]
    fn new_wraps_unwrapped_server_position() {
        let m = Movement::new(0x860201AD, Vec3 { x: 12.3199, y: -28.482, z: 0.005 });
        assert!((m.pos.x - 12.3199).abs() < 1e-4);
        assert!((m.pos.y - 163.518).abs() < 0.001);
        assert_eq!(m.last_sent.y, m.pos.y);
    }

    #[test]
    fn global_xy_unwraps_blocks() {
        // Block 0xC6A9: grid x = 0xC6 = 198, y = 0xA9 = 169.
        let (gx, gy) = global_xy(0xC6A9000A, 10.0, 20.0);
        assert!((gx - (198.0 * 192.0 + 10.0)).abs() < 1e-4);
        assert!((gy - (169.0 * 192.0 + 20.0)).abs() < 1e-4);

        // Cross-block offset: 4 u to the east across the x-block boundary
        // (the landblock's HIGH byte is the x block).
        let (a_x, _) = global_xy(0xC6A9000A, 190.0, 20.0);
        let (b_x, _) = global_xy(0xC7A9000A, 2.0, 20.0);
        assert!((b_x - a_x - 4.0).abs() < 1e-4);
    }

    #[test]
    fn approach_turns_in_place_until_facing() {
        // Target straight ahead (+y at heading 0): walk immediately.
        let s = approach_step(0.0, 0.0, 50.0, 50.0, 2.5);
        assert!(s.forward && !s.turn_left && !s.turn_right && !s.arrived);

        // Target 90 degrees to the right: turn right in place, no forward.
        let s = approach_step(0.0, 50.0, 0.0, 50.0, 2.5);
        assert!(s.turn_right && !s.turn_left && !s.forward && !s.arrived);

        // Target 90 degrees to the left: turn left in place.
        let s = approach_step(0.0, -50.0, 0.0, 50.0, 2.5);
        assert!(s.turn_left && !s.turn_right && !s.forward);

        // Target behind: must still turn in place (no walking backwards).
        let s = approach_step(0.0, 0.0, -50.0, 50.0, 2.5);
        assert!(!s.forward && (s.turn_left || s.turn_right));
    }

    #[test]
    fn approach_walking_drift_corrects_with_turn() {
        // A small yaw error while walking: inside the dead band no
        // correction turn; just past it, a turn key joins the forward.
        let s = approach_step(0.05, 0.0, 50.0, 50.0, 2.5);
        assert!(s.forward && !s.turn_left && !s.turn_right);
        let s = approach_step(0.1, 0.0, 50.0, 50.0, 2.5);
        assert!(s.forward && s.turn_left && !s.turn_right);
        // A large yaw error turns in place (no forward drift).
        let s = approach_step(0.5, 0.0, 50.0, 50.0, 2.5);
        assert!(!s.forward && s.turn_left && !s.turn_right);
    }

    #[test]
    fn approach_arrives_within_stop_distance() {
        // Far: keep walking.
        assert!(!approach_step(0.0, 0.0, 30.0, 30.0, 2.5).arrived);
        // Within the radius: arrived (no forward, so no overshoot).
        let s = approach_step(0.0, 0.0, 2.0, 2.0, 2.5);
        assert!(s.arrived && !s.forward);
        // Just past the radius by less than the arrival epsilon: stop
        // here rather than overshoot the use radius by a full step.
        let s = approach_step(0.0, 0.0, 2.6, 2.6, 2.5);
        assert!(s.arrived && !s.forward);
    }

}
