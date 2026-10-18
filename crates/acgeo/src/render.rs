//! Software 3D rasterizer: third-person follow camera, perspective
//! projection, z-buffer, flat Lambert shading, distance fog, terrain +
//! statics + doors + animated actors, the per-actor glow halo, and the 2D
//! target reticle overlay. Ported from the renderer half of
//! `acclient-ts/src/world3d.ts`.
//!
//! Output is a `cols × rows` grid of RGB cells (the UI draws each as a
//! full-block `█`). Fidelity goal: a working, visually-plausible render —
//! not a pixel match of the TS (which emits quantized palette attrs).

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::acegeom::{self, Animation, AnimRef, Frame, MotionTable, IDENTITY};
use crate::scene::{mat_color, NULL_PART, Scene};
use crate::{wrap_block_coord, Aabb, Actor, Camera, Frame3D, ReticleTarget, Solid};

// ---------------------------------------------------------------------------
// camera / light / fog framing (the TS's tunable constants)
// ---------------------------------------------------------------------------

const CAM_BACK: f32 = 13.0;
const CAM_UP: f32 = 6.5;
const CAM_LOOK_UP: f32 = 1.5;
const CAM_FOCAL_K: f32 = 1.0;
const CAM_NEAR: f32 = 0.5;

const LIGHT: (f32, f32, f32) = (0.35, 0.35, 0.87);
const LIGHT_AMBIENT: f32 = 0.38;
const LIGHT_DIFFUSE: f32 = 0.8;

/// Linear distance fog (blends geometry toward the sky).
const FOG_START: f32 = 30.0;
const FOG_END: f32 = 165.0;
const FOG_COLOR: (f32, f32, f32) = (120.0, 168.0, 214.0);

/// 1/e time constant (seconds) of the zoom smoothing.
const ZOOM_TAU: f64 = 0.25;
/// Closest the camera gets to the player, as a fraction of CAM.back.
const ZOOM_MIN: f32 = 0.2;
/// How far (world units) in front of the blocking surface the camera stops.
const ZOOM_MARGIN: f32 = 0.75;
/// Closest the user's camera back distance may be (world units).
const BACK_MIN: f32 = 4.0;
/// Farthest the user's camera back distance may be (world units).
const BACK_MAX: f32 = 48.0;
/// Per-notch wheel zoom factor (zoom in multiplies by it, out divides).
const ZOOM_STEP: f32 = 0.8;

const RETICLE_PERIOD: f64 = 1.2;
const MIN_RETICLE: f32 = 8.0;
/// The reticle color (bright red — stands out against sky, terrain and
/// body meshes alike).
const RETICLE_COLOR: [u8; 3] = [235, 64, 64];

/// The glow anchor's height above the actor's feet (world units; chest).
const GLOW_ANCHOR_H: f32 = 1.2;
/// Assumed body half-height (world units) used to scale the glow radius
/// with the actor's apparent size.
const GLOW_HALF_H: f32 = 1.4;
/// Peak blend weight at the glow center (1.0 would paint the body cells
/// solid glow color).
const GLOW_STRENGTH: f32 = 0.8;
const GLOW_R_MIN: f32 = 2.0;
const GLOW_R_MAX: f32 = 4.0;
/// How many times the projected body half-height the glow reaches.
const GLOW_SPREAD: f32 = 1.6;

/// A flat building/prop color for the static physics triangles (the physics
/// leaves carry no material, unlike the TS's drawing-mesh soup).
const STATIC_COLOR: (f32, f32, f32) = (150.0, 132.0, 110.0);
/// Door slab colors: closed = solid wood, open = a muted ghost of the same.
const DOOR_CLOSED: (f32, f32, f32) = (132.0, 90.0, 52.0);
const DOOR_OPEN: (f32, f32, f32) = (92.0, 78.0, 62.0);

/// Motion commands (the TS's MCMD values from movement.ts).
const MCMD_READY: u32 = 0x41000003;
const MCMD_WALK_FORWARD: u32 = 0x45000005;
const MCMD_WALK_BACKWARDS: u32 = 0x45000006;
const MCMD_TURN_RIGHT: u32 = 0x6500000d;
const MCMD_TURN_LEFT: u32 = 0x6500000e;
const MCMD_SIDESTEP_RIGHT: u32 = 0x6500000f;
const MCMD_SIDESTEP_LEFT: u32 = 0x65000010;

// ---------------------------------------------------------------------------
// public state types
// ---------------------------------------------------------------------------

/// Camera zoom state (shared across frames): the user's desired back
/// distance, the smoothed current back distance, and the time of the last
/// update. The occlusion system may pull the actual camera in past
/// `user_back` (geometry between the player and the camera), but never lets
/// it go farther back than `user_back`.
pub struct Zoom {
    /// The user's desired base back distance (wheel-controlled, clamped to
    /// `[BACK_MIN, BACK_MAX]`).
    user_back: f32,
    /// The smoothed current back distance (occlusion-adjusted).
    back: f32,
    last_now: f64,
}

impl Zoom {
    #[must_use]
    pub fn new() -> Self {
        Self {
            user_back: CAM_BACK,
            back: CAM_BACK,
            last_now: -1.0,
        }
    }

    /// The user's desired base back distance (for UI display).
    #[must_use]
    pub fn user_back(&self) -> f32 {
        self.user_back
    }

    /// Zoom in (camera closer) by one wheel notch.
    pub fn zoom_in(&mut self) {
        self.user_back = (self.user_back * ZOOM_STEP).max(BACK_MIN);
    }

    /// Zoom out (camera farther) by one wheel notch.
    pub fn zoom_out(&mut self) {
        self.user_back = (self.user_back / ZOOM_STEP).min(BACK_MAX);
    }
}
impl Default for Zoom {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-actor animation clock, keyed by guid (persists across frames).
#[derive(Clone, Copy, Debug)]
pub struct AnimClock {
    /// Accumulated animation time (seconds).
    pub anim_time: f64,
    /// Last motion_cmd seen (a change resets `anim_time`).
    pub last_motion: u32,
    /// Last frame's `now` (seconds); negative = never rendered.
    pub last_now: f64,
}

impl Default for AnimClock {
    fn default() -> Self {
        Self {
            anim_time: 0.0,
            last_motion: 0,
            last_now: -1.0,
        }
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as f64 + d.subsec_micros() as f64 * 1e-6)
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// camera projection
// ---------------------------------------------------------------------------

/// The follow camera's basis: camera position, forward/right/up vectors,
/// focal length, and screen center.
struct Basis {
    cam: [f32; 3],
    f: [f32; 3],
    r: [f32; 3],
    u: [f32; 3],
    focal: f32,
    sc: [f32; 2],
}

/// Build the follow camera basis. `back` is the camera's distance behind the
/// player; the camera's height scales with it (a pure dolly), so a zoomed-in
/// camera keeps the same framing of the player.
fn camera_basis(cam: &Camera, cols: usize, rows: usize, back: f32) -> Basis {
    let h = cam.heading; // radians: 0 = +Y, + toward +X
    let (fx, fy) = (h.sin(), h.cos());
    let cam_x = cam.x - fx * back;
    let cam_y = cam.y - fy * back;
    let cam_z = cam.z + CAM_UP * (back / CAM_BACK);
    // View direction = toward a point at the player, slightly above.
    let (mut vfx, mut vfy, mut vfz) = (cam.x - cam_x, cam.y - cam_y, cam.z + CAM_LOOK_UP - cam_z);
    let mut fl = (vfx * vfx + vfy * vfy + vfz * vfz).sqrt();
    if fl < f32::EPSILON {
        (vfx, vfy, vfz, fl) = (0.0, 1.0, 0.0, 1.0);
    }
    vfx /= fl;
    vfy /= fl;
    vfz /= fl;
    // right = normalize(cross(fwd, up0)) = (vfy, -vfx, 0)
    let (mut rx, mut ry) = (vfy, -vfx);
    let mut rl = (rx * rx + ry * ry).sqrt();
    if rl < f32::EPSILON {
        (rx, ry, rl) = (1.0, 0.0, 1.0);
    }
    rx /= rl;
    ry /= rl;
    // up = cross(right, fwd)
    let (ux, uy, uz) = (ry * vfz, -rx * vfz, rx * vfy - ry * vfx);
    Basis {
        cam: [cam_x, cam_y, cam_z],
        f: [vfx, vfy, vfz],
        r: [rx, ry, 0.0],
        u: [ux, uy, uz],
        focal: rows as f32 * CAM_FOCAL_K,
        sc: [(cols - 1) as f32 / 2.0, (rows - 1) as f32 / 2.0],
    }
}

/// Project a world point with the camera basis into screen cells (floats);
/// `None` when behind the near plane. Returns `(sx, sy, zc)` with `zc` the
/// forward depth.
#[inline]
fn project(b: &Basis, px: f32, py: f32, pz: f32) -> Option<(f32, f32, f32)> {
    let (vx, vy, vz) = (px - b.cam[0], py - b.cam[1], pz - b.cam[2]);
    let zc = vx * b.f[0] + vy * b.f[1] + vz * b.f[2];
    if zc < CAM_NEAR {
        return None;
    }
    let s = b.focal / zc;
    Some((
        b.sc[0] + (vx * b.r[0] + vy * b.r[1]) * s,
        b.sc[1] - (vx * b.u[0] + vy * b.u[1] + vz * b.u[2]) * s,
        zc,
    ))
}

// ---------------------------------------------------------------------------
// camera occlusion zoom (the camera never crosses the world's geometry)
// ---------------------------------------------------------------------------

/// Distance along the (unit) ray to the triangle, or `f32::INFINITY` when the
/// ray misses (Möller–Trumbore, double-sided — a camera ray can hit either
/// face of a wall).
fn tri_t(
    ox: f32,
    oy: f32,
    oz: f32,
    dx: f32,
    dy: f32,
    dz: f32,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
    c: (f32, f32, f32),
) -> f32 {
    const EPS: f32 = 1e-9;
    let (e1x, e1y, e1z) = (b.0 - a.0, b.1 - a.1, b.2 - a.2);
    let (e2x, e2y, e2z) = (c.0 - a.0, c.1 - a.1, c.2 - a.2);
    let (hx, hy, hz) = (dy * e2z - dz * e2y, dz * e2x - dx * e2z, dx * e2y - dy * e2x);
    let det = e1x * hx + e1y * hy + e1z * hz;
    if det > -EPS && det < EPS {
        return f32::INFINITY;
    }
    let inv = 1.0 / det;
    let (sx, sy, sz) = (ox - a.0, oy - a.1, oz - a.2);
    let u = (sx * hx + sy * hy + sz * hz) * inv;
    if u < -EPS || u > 1.0 + EPS {
        return f32::INFINITY;
    }
    let (qx, qy, qz) = (sy * e1z - sz * e1y, sz * e1x - sx * e1z, sx * e1y - sy * e1x);
    let v = (dx * qx + dy * qy + dz * qz) * inv;
    if v < -EPS || u + v > 1.0 + EPS {
        return f32::INFINITY;
    }
    let t = (e2x * qx + e2y * qy + e2z * qz) * inv;
    if t > 1e-6 { t } else { f32::INFINITY }
}

/// The camera back distance that keeps the line of sight to the player:
/// `back_target`, unless the landblock's static geometry (terrain, building
/// meshes) crosses the ray from the player's chest to the ideal camera
/// position (at `back_target` behind), in which case just in front of the
/// nearest hit. The ray is the full chest->ideal-camera segment, so geometry
/// BEYOND the ideal camera never occludes.
fn occlude_camera_back(
    soup: Option<&[f32]>,
    statics: &[Solid],
    cam: &Camera,
    back_target: f32,
) -> f32 {
    let h = cam.heading;
    let (fx, fy) = (h.sin(), h.cos());
    let (ox, oy, oz) = (cam.x, cam.y, cam.z + CAM_LOOK_UP);
    // Ideal camera at `back_target` behind: horizontally `-fx*back_target`,
    // and `CAM_UP * (back_target / CAM_BACK)` above the feet (see
    // camera_basis — a pure dolly). The ray runs chest -> that point.
    let cam_h = CAM_UP * (back_target / CAM_BACK);
    let (ix, iy, iz) = (-fx * back_target, -fy * back_target, cam_h - CAM_LOOK_UP);
    let l = (ix * ix + iy * iy + iz * iz).sqrt();
    let (dx, dy, dz) = (ix / l, iy / l, iz / l);
    let mut best = l;
    if let Some(soup) = soup {
        let n = soup.len() / 12 * 12;
        for t in (0..n).step_by(12) {
            let tt = tri_t(
                ox, oy, oz, dx, dy, dz,
                (soup[t], soup[t + 1], soup[t + 2]),
                (soup[t + 3], soup[t + 4], soup[t + 5]),
                (soup[t + 6], soup[t + 7], soup[t + 8]),
            );
            if tt > 0.0 && tt < best {
                best = tt;
            }
        }
    }
    for s in statics {
        let tris = &s.tris;
        let n = tris.len() / 9 * 9;
        for i in (0..n).step_by(9) {
            let tt = tri_t(
                ox, oy, oz, dx, dy, dz,
                (tris[i], tris[i + 1], tris[i + 2]),
                (tris[i + 3], tris[i + 4], tris[i + 5]),
                (tris[i + 6], tris[i + 7], tris[i + 8]),
            );
            if tt > 0.0 && tt < best {
                best = tt;
            }
        }
    }
    if best >= l {
        back_target
    } else {
        (back_target * ZOOM_MIN).max(back_target * ((best - ZOOM_MARGIN) / l))
    }
}

/// One smoothing step: move `zoom.back` toward `target` with the 1/e time
/// constant ZOOM_TAU, at time `now` (seconds). The first call jumps straight
/// to the target; a non-positive dt holds.
fn smooth_camera_zoom(zoom: &mut Zoom, target: f32, now: f64) -> f32 {
    let dt = if zoom.last_now < 0.0 {
        0.0
    } else {
        (now - zoom.last_now).clamp(0.0, 0.5)
    };
    zoom.last_now = now;
    zoom.back = if dt <= 0.0 {
        target
    } else {
        let t = dt as f32;
        zoom.back + (target - zoom.back) * (1.0 - (-t / ZOOM_TAU as f32).exp())
    };
    zoom.back
}

// ---------------------------------------------------------------------------
// raster
// ---------------------------------------------------------------------------

#[inline]
fn cl8(v: f32) -> u8 {
    (v.min(255.0).max(0.0)) as u8
}

/// One directed triangle into the frame (backface cull, flat Lambert,
/// distance fog, z-buffer).
fn raster_tri(
    cells: &mut [[u8; 3]],
    depth: &mut [f32],
    b: &Basis,
    cols: usize,
    rows: usize,
    a: (f32, f32, f32),
    bb: (f32, f32, f32),
    c: (f32, f32, f32),
    color: (f32, f32, f32),
    light: (f32, f32, f32),
    fog_span: f32,
) {
    let (ax, ay, az) = a;
    let (bx, by, bz) = bb;
    let (cx, cy, cz) = c;
    // Backface cull: the directed triangle's world normal must face the
    // camera.
    let (ex1, ey1, ez1) = (bx - ax, by - ay, bz - az);
    let (ex2, ey2, ez2) = (cx - ax, cy - ay, cz - az);
    let nx = ey1 * ez2 - ez1 * ey2;
    let ny = ez1 * ex2 - ex1 * ez2;
    let nz = ex1 * ey2 - ey1 * ex2;
    if nx * (b.cam[0] - ax) + ny * (b.cam[1] - ay) + nz * (b.cam[2] - az) <= 0.0 {
        return;
    }
    let Some(pa) = project(b, ax, ay, az) else {
        return;
    };
    let Some(pb) = project(b, bx, by, bz) else {
        return;
    };
    let Some(pc) = project(b, cx, cy, cz) else {
        return;
    };
    // Flat Lambert from the (normalized) face normal.
    let nl = (nx * nx + ny * ny + nz * nz).sqrt();
    let lambert = if nl < f32::EPSILON { 0.0 } else {
        (nx * light.0 + ny * light.1 + nz * light.2) / nl
    };
    let shade = LIGHT_AMBIENT + LIGHT_DIFFUSE * lambert.max(0.0);
    // Linear distance fog from the camera to the centroid.
    let (dx, dy, dz) = (
        (ax + bx + cx) / 3.0 - b.cam[0],
        (ay + by + cy) / 3.0 - b.cam[1],
        (az + bz + cz) / 3.0 - b.cam[2],
    );
    let mut ft = ((dx * dx + dy * dy + dz * dz).sqrt() - FOG_START) / fog_span;
    if ft < 0.0 {
        ft = 0.0;
    } else if ft > 1.0 {
        ft = 1.0;
    }
    let color = (
        color.0 * shade + (FOG_COLOR.0 - color.0 * shade) * ft,
        color.1 * shade + (FOG_COLOR.1 - color.1 * shade) * ft,
        color.2 * shade + (FOG_COLOR.2 - color.2 * shade) * ft,
    );
    let color: [u8; 3] = [cl8(color.0), cl8(color.1), cl8(color.2)];

    // Raster: barycentric point-in-triangle over the screen bbox.
    let min_x = (pa.0.min(pb.0.min(pc.0)).floor() as i32).max(0) as usize;
    let max_x = (pa.0.max(pb.0.max(pc.0)).ceil() as i32).clamp(0, cols as i32 - 1) as usize;
    let min_y = (pa.1.min(pb.1.min(pc.1)).floor() as i32).max(0) as usize;
    let max_y = (pa.1.max(pb.1.max(pc.1)).ceil() as i32).clamp(0, rows as i32 - 1) as usize;
    if max_x < min_x || max_y < min_y {
        return;
    }
    let e = |p1: (f32, f32), p2: (f32, f32), p: (f32, f32)| {
        (p2.0 - p1.0) * (p.1 - p1.1) - (p2.1 - p1.1) * (p.0 - p1.0)
    };
    let (p0, p1, p2) = (
        (pa.0, pa.1),
        (pb.0, pb.1),
        (pc.0, pc.1),
    );
    let denom = e(p0, p1, p2);
    if denom.abs() < 1e-6 {
        return; // degenerate on screen
    }
    for r in min_y..=max_y {
        let py = r as f32 + 0.5;
        let row_base = r * cols;
        for cc in min_x..=max_x {
            let px = cc as f32 + 0.5;
            let la = e(p1, p2, (px, py)) / denom;
            if la < 0.0 || la > 1.0 {
                continue;
            }
            let lb = e(p2, p0, (px, py)) / denom;
            if lb < 0.0 || lb > 1.0 {
                continue;
            }
            let lc = 1.0 - la - lb;
            if lc < 0.0 || lc > 1.0 {
                continue;
            }
            let zc = la * pa.2 + lb * pb.2 + lc * pc.2;
            let idx = row_base + cc;
            if zc < depth[idx] {
                depth[idx] = zc;
                cells[idx] = color;
            }
        }
    }
}

/// Rasterize a triangle soup (12 floats/tri: 3 verts + rgb) into the frame.
fn raster_soup(
    cells: &mut [[u8; 3]],
    depth: &mut [f32],
    b: &Basis,
    cols: usize,
    rows: usize,
    soup: &[f32],
    light: (f32, f32, f32),
    fog_span: f32,
) {
    let n = soup.len() / 12 * 12;
    for t in (0..n).step_by(12) {
        let (cr, cg, cb) = (soup[t + 9], soup[t + 10], soup[t + 11]);
        raster_tri(
            cells,
            depth,
            b,
            cols,
            rows,
            (soup[t], soup[t + 1], soup[t + 2]),
            (soup[t + 3], soup[t + 4], soup[t + 5]),
            (soup[t + 6], soup[t + 7], soup[t + 8]),
            (cr, cg, cb),
            light,
            fog_span,
        );
    }
}

/// Rasterize the static solids (physics triangles — no material, so a flat
/// building color; both windings so backface culling keeps the visible side).
fn raster_statics(
    cells: &mut [[u8; 3]],
    depth: &mut [f32],
    b: &Basis,
    cols: usize,
    rows: usize,
    statics: &[Solid],
    light: (f32, f32, f32),
    fog_span: f32,
) {
    for s in statics {
        let tris = &s.tris;
        let n = tris.len() / 9 * 9;
        for i in (0..n).step_by(9) {
            let a = (tris[i], tris[i + 1], tris[i + 2]);
            let bb = (tris[i + 3], tris[i + 4], tris[i + 5]);
            let c = (tris[i + 6], tris[i + 7], tris[i + 8]);
            raster_tri(cells, depth, b, cols, rows, a, bb, c, STATIC_COLOR, light, fog_span);
            raster_tri(cells, depth, b, cols, rows, bb, a, c, STATIC_COLOR, light, fog_span);
        }
    }
}

/// Rasterize a door's collision slab as a box (outward-facing faces).
fn raster_door_box(
    cells: &mut [[u8; 3]],
    depth: &mut [f32],
    b: &Basis,
    cols: usize,
    rows: usize,
    aabb: &Aabb,
    open: bool,
    light: (f32, f32, f32),
    fog_span: f32,
) {
    let [x0, y0, z0, x1, y1, z1] = *aabb;
    let color = if open { DOOR_OPEN } else { DOOR_CLOSED };
    // 4 corners per face, CCW as seen from outside (outward normal).
    let faces: [[(f32, f32, f32); 4]; 6] = [
        [(x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)],
        [(x0, y0, z0), (x0, y1, z0), (x1, y1, z0), (x1, y0, z0)],
        [(x0, y1, z0), (x0, y1, z1), (x1, y1, z1), (x1, y1, z0)],
        [(x1, y0, z0), (x1, y0, z1), (x0, y0, z1), (x0, y0, z0)],
        [(x1, y0, z0), (x1, y1, z0), (x1, y1, z1), (x1, y0, z1)],
        [(x0, y0, z0), (x0, y0, z1), (x0, y1, z1), (x0, y1, z0)],
    ];
    for face in faces {
        let [v0, v1, v2, v3] = face;
        raster_tri(cells, depth, b, cols, rows, v0, v1, v2, color, light, fog_span);
        raster_tri(cells, depth, b, cols, rows, v0, v2, v3, color, light, fog_span);
    }
}

// ---------------------------------------------------------------------------
// target reticle (a 2D bounding box: four animated corner triangles pointing
// toward the target's 2D center, drawn as a HUD overlay on the 3D frame)
// ---------------------------------------------------------------------------

/// Project the eight corners of a world-space box — base center `(cx, cy,
/// cz)`, horizontal half-extent `r`, half-height `half_h` — through the
/// camera into screen cells and return the 2D bounding box of the projected
/// corners, clamped to the pane. `None` when every corner is behind the near
/// plane.
fn screen_box_for(
    b: &Basis,
    cols: usize,
    rows: usize,
    cx: f32,
    cy: f32,
    cz: f32,
    r: f32,
    half_h: f32,
) -> Option<(i32, i32, i32, i32)> {
    let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
    let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for dx in [-r, r] {
        for dy in [-r, r] {
            for dz in [0.0, half_h * 2.0] {
                if let Some(p) = project(b, cx + dx, cy + dy, cz + dz) {
                    min_x = min_x.min(p.0);
                    min_y = min_y.min(p.1);
                    max_x = max_x.max(p.0);
                    max_y = max_y.max(p.1);
                }
            }
        }
    }
    if !min_x.is_finite() {
        return None;
    }
    let x0 = (min_x.floor() as i32).max(0);
    let y0 = (min_y.floor() as i32).max(0);
    let x1 = (max_x.ceil() as i32).clamp(0, cols as i32 - 1);
    let y1 = (max_y.ceil() as i32).clamp(0, rows as i32 - 1);
    if x1 < x0 || y1 < y0 {
        return None;
    }
    Some((x0, y0, x1, y1))
}

/// The four corner triangles of the target reticle. Each triangle's base
/// lies along the two box edges meeting at a corner and its apex points along
/// the corner->center diagonal; the apex reach (and arm length) pulse with
/// `phase` (0..1) so the box "breathes" toward the target's 2D center. The
/// box is expanded about its center to at least MIN_RETICLE cells.
fn reticle_triangles(sb: (i32, i32, i32, i32), phase: f64) -> [[[f32; 2]; 3]; 4] {
    let (mut x0, mut y0) = (sb.0 as f32, sb.1 as f32);
    let (mut x1, mut y1) = (sb.2 as f32, sb.3 as f32);
    let cx = (x0 + x1) / 2.0;
    let cy = (y0 + y1) / 2.0;
    if x1 - x0 < MIN_RETICLE {
        x0 = cx - MIN_RETICLE / 2.0;
        x1 = cx + MIN_RETICLE / 2.0;
    }
    if y1 - y0 < MIN_RETICLE {
        y0 = cy - MIN_RETICLE / 2.0;
        y1 = cy + MIN_RETICLE / 2.0;
    }
    let m = (x1 - x0).min(y1 - y0);
    let pulse = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * phase as f32).sin();
    let arm = 0.32 * m;
    let reach = (0.18 + 0.5 * pulse) * m;
    let corners = [
        (x0, y0, 1.0f32, 1.0f32),
        (x1, y0, -1.0, 1.0),
        (x1, y1, -1.0, -1.0),
        (x0, y1, 1.0, -1.0),
    ];
    corners.map(|(px, py, ex, ey)| {
        let (ddx, ddy) = (cx - px, cy - py);
        let dl = (ddx * ddx + ddy * ddy).sqrt().max(1e-12);
        [
            [px + ddx / dl * reach, py + ddy / dl * reach], // apex (toward center)
            [px + ex * arm, py],                            // base along one box edge
            [px, py + ey * arm],                            // base along the other
        ]
    })
}

/// Rasterize one screen-space triangle with a barycentric fill. Overwrites
/// the cells beneath (a HUD overlay on the 3D frame — no depth test),
/// clamped to the pane.
fn fill_tri(out: &mut [[u8; 3]], cols: usize, rows: usize, t: &[[f32; 2]; 3], color: [u8; 3]) {
    let [p0, p1, p2] = t;
    let min_x = (p0[0].min(p1[0].min(p2[0])).floor() as i32).max(0) as usize;
    let max_x = (p0[0].max(p1[0].max(p2[0])).ceil() as i32).clamp(0, cols as i32 - 1) as usize;
    let min_y = (p0[1].min(p1[1].min(p2[1])).floor() as i32).max(0) as usize;
    let max_y = (p0[1].max(p1[1].max(p2[1])).ceil() as i32).clamp(0, rows as i32 - 1) as usize;
    if max_x < min_x || max_y < min_y {
        return;
    }
    let e = |a: [f32; 2], bb: [f32; 2], p: [f32; 2]| {
        (bb[0] - a[0]) * (p[1] - a[1]) - (bb[1] - a[1]) * (p[0] - a[0])
    };
    let denom = e(*p0, *p1, *p2);
    if denom.abs() < 1e-9 {
        return;
    }
    for r in min_y..=max_y {
        let py = r as f32 + 0.5;
        let row_base = r * cols;
        for c in min_x..=max_x {
            let px = c as f32 + 0.5;
            let la = e(*p1, *p2, [px, py]) / denom;
            if la < 0.0 || la > 1.0 {
                continue;
            }
            let lb = e(*p2, *p0, [px, py]) / denom;
            if lb < 0.0 || lb > 1.0 {
                continue;
            }
            let lc = 1.0 - la - lb;
            if lc < 0.0 || lc > 1.0 {
                continue;
            }
            out[row_base + c] = color;
        }
    }
}

/// Draw the four animated corner triangles for `box` into `out`, overwriting
/// the scene beneath. `phase` (0..1) drives the breathing pulse.
fn draw_target_reticle(
    out: &mut [[u8; 3]],
    cols: usize,
    rows: usize,
    sb: (i32, i32, i32, i32),
    phase: f64,
) {
    for t in reticle_triangles(sb, phase) {
        fill_tri(out, cols, rows, &t, RETICLE_COLOR);
    }
}

// ---------------------------------------------------------------------------
// actor glow (a soft halo around characters: at terminal resolution a body
// mesh is a handful of cells and a dark body vanishes against the terrain,
// so each character's projected chest lights up a small screen neighborhood
// — depth-tested, so the glow never bleeds through nearer walls)
// ---------------------------------------------------------------------------

/// Paint a soft halo around each glowing actor into `out` (over the rendered
/// scene). Each cell within the radius of the anchor's projection blends
/// toward the glow color with a quadratic falloff (GLOW_STRENGTH at the
/// center), but only where the scene depth at that cell is at least as far
/// as the anchor — a nearer surface (a wall between camera and actor) does
/// not light up.
fn draw_glow(
    out: &mut [[u8; 3]],
    depth: &[f32],
    b: &Basis,
    cols: usize,
    rows: usize,
    actors: &[Actor],
) {
    for a in actors {
        let Some(glow_rgb) = a.glow else {
            continue;
        };
        let (wx, wy, wz) = (wrap_block_coord(a.x), wrap_block_coord(a.y), a.z + GLOW_ANCHOR_H);
        let Some((sx, sy, zc)) = project(b, wx, wy, wz) else {
            continue;
        };
        let r = (GLOW_HALF_H * b.focal * GLOW_SPREAD) / zc;
        let r = r.clamp(GLOW_R_MIN, GLOW_R_MAX);
        let (gr, gg, gb) = (
            ((glow_rgb >> 16) & 0xff) as f32,
            ((glow_rgb >> 8) & 0xff) as f32,
            (glow_rgb & 0xff) as f32,
        );
        let x0 = ((sx - r).floor() as i32).max(0) as usize;
        let x1 = ((sx + r).ceil() as i32).clamp(0, cols as i32 - 1) as usize;
        let y0 = ((sy - r).floor() as i32).max(0) as usize;
        let y1 = ((sy + r).ceil() as i32).clamp(0, rows as i32 - 1) as usize;
        for row in y0..=y1 {
            let dy = row as f32 + 0.5 - sy;
            let row_base = row * cols;
            for c in x0..=x1 {
                let dx = c as f32 + 0.5 - sx;
                let d = (dx * dx + dy * dy).sqrt();
                if d > r {
                    continue;
                }
                let idx = row_base + c;
                if depth[idx] < zc - 1e-6 {
                    continue; // something nearer occupies it
                }
                let mut t = 1.0 - d / r;
                t = GLOW_STRENGTH * t * t;
                let (sr, sg, sb) = (out[idx][0] as f32, out[idx][1] as f32, out[idx][2] as f32);
                out[idx] = [
                    cl8(sr + (gr - sr) * t),
                    cl8(sg + (gg - sg) * t),
                    cl8(sb + (gb - sb) * t),
                ];
            }
        }
    }
}

// ---------------------------------------------------------------------------
// dynamic actors (players / creatures, animated)
// ---------------------------------------------------------------------------

/// The part pose to render: the sampled animation row (part frames with the
/// mirror already applied) plus its pos frame.
struct Pose {
    /// Part frames of the sampled row — slot `pi` is the part frame of setup
    /// slot `pi`; an empty list means identity for all parts.
    part_frames: Vec<Frame>,
    pos_frame: Frame,
}

/// The resolved (stance, motion) animation: the MotionData's anim refs plus
/// the client-side mirror/reverse derivations the retail client applies.
struct ResolvedCycle<'a> {
    anims: &'a [AnimRef],
    mirror: bool,
    reverse: bool,
}

fn cycle_key(style: u32, motion_cmd: u32) -> u32 {
    ((style & 0xffff) << 16) | (motion_cmd & 0xfffff)
}

fn resolve_cycle(
    mtable: &MotionTable,
    style: u32,
    motion_cmd: u32,
) -> Option<ResolvedCycle<'_>> {
    let key = cycle_key(style, motion_cmd);
    if let Some(md) = mtable.cycles.get(&key) {
        return Some(ResolvedCycle {
            anims: &md.anims,
            mirror: false,
            reverse: false,
        });
    }
    // Client-derived variants (no explicit link entry in the retail table).
    if motion_cmd == MCMD_WALK_BACKWARDS {
        if let Some(md) = mtable.cycles.get(&cycle_key(style, MCMD_WALK_FORWARD)) {
            return Some(ResolvedCycle {
                anims: &md.anims,
                mirror: false,
                reverse: true,
            });
        }
    }
    if motion_cmd == MCMD_TURN_LEFT {
        if let Some(md) = mtable.cycles.get(&cycle_key(style, MCMD_TURN_RIGHT)) {
            return Some(ResolvedCycle {
                anims: &md.anims,
                mirror: true,
                reverse: false,
            });
        }
    }
    if motion_cmd == MCMD_SIDESTEP_LEFT {
        if let Some(md) = mtable.cycles.get(&cycle_key(style, MCMD_SIDESTEP_RIGHT)) {
            return Some(ResolvedCycle {
                anims: &md.anims,
                mirror: true,
                reverse: false,
            });
        }
    }
    // Motion-mapping links (attack/ability chains).
    if let Some(inner) = mtable.links.get(&key) {
        if let Some(md) = inner.get(&motion_cmd) {
            return Some(ResolvedCycle {
                anims: &md.anims,
                mirror: false,
                reverse: false,
            });
        }
    }
    // Idle fallback.
    if let Some(md) = mtable.cycles.get(&cycle_key(style, MCMD_READY)) {
        return Some(ResolvedCycle {
            anims: &md.anims,
            mirror: false,
            reverse: false,
        });
    }
    None
}

/// Sample the frame index for an animation reference at a given time.
/// `reverse` is the client-side derivation (WalkBackwards); the authored
/// negative framerate (SideStepRight) reverses independently.
fn sample_frame(anim: &Animation, ref_: &AnimRef, reverse: bool, time: f64) -> usize {
    let low = if ref_.low_frame < 0 { 0 } else { ref_.low_frame };
    let mut hi = if ref_.high_frame == -1 {
        anim.num_frames as i32
    } else {
        ref_.high_frame.min(anim.num_frames as i32)
    };
    if hi <= low {
        hi = low + 1;
    }
    let mut rate = ref_.framerate.abs() as f64;
    if rate == 0.0 {
        rate = 1.0;
    }
    let duration = (hi - low) as f64 / rate;
    let progress = (time % duration) * rate;
    let rev = reverse != (ref_.framerate < 0.0);
    let ff = if rev { hi as f64 - progress } else { low as f64 + progress };
    let mut frame = ff.round() as i32;
    if frame < low {
        frame = low;
    }
    if frame > hi - 1 {
        frame = hi - 1;
    }
    frame as usize
}

/// Resolve the pose (animation row) for an actor, or the static setup pose.
/// The sampled row is copied out (a few hundred bytes) so no scene borrow
/// outlives the call.
fn resolve_pose(scene: &mut Scene, a: &Actor, anim_time: f64) -> Pose {
    // Resolve the (anim ref, mirror, reverse) from the motion table first —
    // the mtable borrow ends before the animation is fetched.
    let mut resolved: Option<(AnimRef, bool, bool)> = None;
    if let Some(mtable_id) = a.mtable_id {
        if let Some(mtable) = scene.get_motion_table(mtable_id) {
            if let Some(rc) = resolve_cycle(mtable, a.style, a.motion_cmd) {
                if let Some(ref_) = rc.anims.first() {
                    resolved = Some((*ref_, rc.mirror, rc.reverse));
                }
            }
        }
    }
    if let Some((ref_, mirror, reverse)) = resolved {
        if let Some(anim) = scene.get_animation(ref_.anim_id) {
            let frame = sample_frame(anim, &ref_, reverse, anim_time);
            let (pos_frame, part_frames) = row_frames(anim, frame, mirror);
            return Pose { part_frames, pos_frame };
        }
    }
    // No motion table / no cycle: the setup's default animation, else
    // identity.
    if let Some(setup_id) = a.setup_id {
        if let Some(tail) = scene.get_setup_tail(setup_id) {
            let def_anim = tail[0];
            if def_anim != 0 {
                if let Some(anim) = scene.get_animation(def_anim) {
                    if anim.num_frames > 0 {
                        let (pos_frame, part_frames) = row_frames(anim, 0, false);
                        return Pose { part_frames, pos_frame };
                    }
                }
            }
        }
    }
    // Parts authored in place (identity transform).
    Pose {
        part_frames: Vec::new(),
        pos_frame: IDENTITY,
    }
}

/// Extract one sampled row from an animation: the root (pos) frame and the
/// part frames (mirror applied when `mirror`). Out-of-range rows fall back
/// to identity so a short animation never indexes past its buffers.
fn row_frames(anim: &Animation, frame: usize, mirror: bool) -> (Frame, Vec<Frame>) {
    let pos_frame = match &anim.pos_frames {
        Some(pf) if !pf.is_empty() && frame * 7 + 7 <= pf.len() => {
            let o = frame * 7;
            Frame::new(
                pf[o], pf[o + 1], pf[o + 2], pf[o + 3], pf[o + 4], pf[o + 5], pf[o + 6],
            )
        }
        _ => IDENTITY,
    };
    let mut part_frames = Vec::with_capacity(anim.num_parts);
    for pi in 0..anim.num_parts {
        let o = (frame * anim.num_parts + pi) * 7;
        let raw = if o + 7 <= anim.part_frames.len() {
            Frame::new(
                anim.part_frames[o],
                anim.part_frames[o + 1],
                anim.part_frames[o + 2],
                anim.part_frames[o + 3],
                anim.part_frames[o + 4],
                anim.part_frames[o + 5],
                anim.part_frames[o + 6],
            )
        } else {
            IDENTITY
        };
        part_frames.push(if mirror { acegeom::mirror_frame(raw) } else { raw });
    }
    (pos_frame, part_frames)
}

/// Build the per-frame dynamic triangle soup (12 floats/tri, world/block
/// space) for the given actors, advancing each actor's animation clock.
/// `None` when there is nothing to draw.
fn build_dynamic_soup(
    scene: &mut Scene,
    actors: &[Actor],
    anim: &mut HashMap<u32, AnimClock>,
    now: f64,
) -> Option<Vec<f32>> {
    if actors.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for a in actors {
        let clock = anim.entry(a.guid).or_default();
        let dt = (now - clock.last_now).clamp(0.0, 0.5);
        clock.last_now = now;
        if a.motion_cmd != clock.last_motion {
            clock.anim_time = 0.0;
            clock.last_motion = a.motion_cmd;
        }
        clock.anim_time += dt;

        let setup_id = match a.setup_id {
            Some(id) => id,
            None => continue,
        };
        let parts = match scene.get_setup_parts(setup_id) {
            Some(p) => p.clone(),
            None => continue,
        };
        if parts.is_empty() {
            continue;
        }
        let pose = resolve_pose(scene, a, clock.anim_time);

        // Actor's world frame (x/y wrapped into the current landblock's frame)
        let obj = Frame::new(
            wrap_block_coord(a.x),
            wrap_block_coord(a.y),
            a.z,
            a.quat[0],
            a.quat[1],
            a.quat[2],
            a.quat[3],
        );

        for (pi, &base_model) in parts.iter().enumerate() {
            let model_id = a
                .model_parts
                .as_ref()
                .and_then(|mp| mp.iter().find(|(i, _)| *i == pi).map(|(_, m)| *m))
                .unwrap_or(base_model);
            if model_id == NULL_PART {
                continue;
            }
            let Some(mesh) = scene.get_part_mesh(model_id) else {
                continue;
            };
            if mesh.tris.is_empty() {
                continue;
            }

            let pf = pose.part_frames.get(pi).copied().unwrap_or(IDENTITY);
            let (q, t) = acegeom::compose3(&obj, &pose.pos_frame, &pf);

            let (pos, tris, tri_mat, mats) = (&mesh.pos, &mesh.tris, &mesh.tri_mat, &mesh.mats);
            for ti in (0..tris.len()).step_by(3) {
                let ia = tris[ti] as usize * 3;
                let ib = tris[ti + 1] as usize * 3;
                let ic = tris[ti + 2] as usize * 3;
                let va = acegeom::rot_t(q, pos[ia], pos[ia + 1], pos[ia + 2], t);
                let vb = acegeom::rot_t(q, pos[ib], pos[ib + 1], pos[ib + 2], t);
                let vc = acegeom::rot_t(q, pos[ic], pos[ic + 1], pos[ic + 2], t);
                let [mr, mg, mb] = mat_color(mats[tri_mat[ti / 3] as usize]);
                out.extend_from_slice(&[
                    va[0], va[1], va[2], vb[0], vb[1], vb[2], vc[0], vc[1], vc[2], mr, mg, mb,
                ]);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// the frame
// ---------------------------------------------------------------------------

/// Render the landblock the camera is in into a `cols × rows` RGB frame.
/// `None` when there is nothing to draw (no dats, no terrain and no actors,
/// or too small).
///
/// With `zoom`, the camera keeps its line of sight to the player: the back
/// distance is pulled in past any geometry between the player and the ideal
/// camera position (entering a confined space zooms in) and eased back out
/// when the sight opens (exiting zooms out).
#[must_use]
pub fn render_frame(
    scene: &mut Scene,
    zoom: &mut Zoom,
    anim: &mut HashMap<u32, AnimClock>,
    statics: &[Solid],
    doors: &HashMap<u32, (Aabb, bool)>,
    lb: u32,
    cam: &Camera,
    actors: &[Actor],
    cols: usize,
    rows: usize,
    glow: bool,
    reticle: Option<&ReticleTarget>,
) -> Option<Frame3D> {
    if cols < 4 || rows < 4 {
        return None;
    }
    scene.load_landblock(lb);
    let now = now_secs();
    let dyn_soup = build_dynamic_soup(scene, actors, anim, now);
    let block_soup = scene.block_soup(lb);
    if block_soup.map_or(true, |s| s.is_empty()) && dyn_soup.as_ref().map_or(true, |s| s.is_empty()) {
        return None;
    }

    // Camera basis (the zoomed camera is shared with the reticle/glow). The
    // user's base back distance sets how far the camera wants to sit; the
    // occlusion pass may pull it in past blocking geometry, never farther.
    let target = occlude_camera_back(block_soup, statics, cam, zoom.user_back);
    let back = smooth_camera_zoom(zoom, target, now);
    let basis = camera_basis(cam, cols, rows, back);

    // Sky: a top->bottom blue gradient fills the pane before geometry.
    let total = cols * rows;
    let mut cells = vec![[0u8, 0, 0]; total];
    for (r, row) in cells.chunks_exact_mut(cols).enumerate() {
        let t = r as f32 / (rows - 1) as f32;
        let (sr, sg, sb) = (
            cl8(52.0 + t * 66.0),
            cl8(84.0 + t * 84.0),
            cl8(150.0 + t * 74.0),
        );
        for c in row {
            *c = [sr, sg, sb];
        }
    }
    let mut depth = vec![f32::INFINITY; total];

    // Normalize the light once.
    let ll = (LIGHT.0 * LIGHT.0 + LIGHT.1 * LIGHT.1 + LIGHT.2 * LIGHT.2).sqrt();
    let light = (LIGHT.0 / ll, LIGHT.1 / ll, LIGHT.2 / ll);
    let fog_span = FOG_END - FOG_START;

    // Terrain, then statics, then doors, then the animated actors last.
    if let Some(soup) = block_soup {
        if !soup.is_empty() {
            raster_soup(&mut cells, &mut depth, &basis, cols, rows, soup, light, fog_span);
        }
    }
    if !statics.is_empty() {
        raster_statics(&mut cells, &mut depth, &basis, cols, rows, statics, light, fog_span);
    }
    for &(b, open) in doors.values() {
        raster_door_box(&mut cells, &mut depth, &basis, cols, rows, &b, open, light, fog_span);
    }
    if let Some(d) = &dyn_soup {
        if !d.is_empty() {
            raster_soup(&mut cells, &mut depth, &basis, cols, rows, d, light, fog_span);
        }
    }

    // The per-actor glow halos (depth-tested; the caller's Actor.glow colors).
    if glow {
        draw_glow(&mut cells, &depth, &basis, cols, rows, actors);
    }
    // The target reticle: drawn last so the box stays crisp on top.
    if let Some(rt) = reticle {
        if let Some(sb) = screen_box_for(&basis, cols, rows, rt.x, rt.y, rt.z, rt.r, rt.half_h) {
            let phase = (now % RETICLE_PERIOD) / RETICLE_PERIOD;
            draw_target_reticle(&mut cells, cols, rows, sb, phase);
        }
    }
    Some(Frame3D {
        cols,
        rows,
        cells,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_scene() -> Option<Scene> {
        if crate::dats::find_dat_dir(None).is_none() {
            return None;
        }
        Some(Scene::new(None))
    }

    #[test]
    fn render_frame_smoke() {
        let Some(mut scene) = test_scene() else {
            eprintln!("skipped: retail dats unavailable");
            return;
        };
        let lb = 0x3073u32; // (48, 115): terrain + hundreds of statics
        let mut zoom = Zoom::new();
        let mut anim = HashMap::new();
        let statics = scene.solids_for_block(lb);
        let doors = HashMap::new();
        let cam = Camera {
            x: 96.0,
            y: 96.0,
            z: scene.height_at(lb, 96.0, 96.0).unwrap_or(0.0),
            heading: 0.7,
        };
        // The player (a human setup, walk cycle) + a nearby creature.
        let actors = vec![
            Actor {
                guid: 1,
                x: 96.0,
                y: 96.0,
                z: cam.z,
                quat: [1.0, 0.0, 0.0, 0.0],
                mtable_id: Some(0x09000001),
                setup_id: Some(0x02000001),
                model_parts: None,
                motion_cmd: MCMD_WALK_FORWARD,
                style: 0x8000003d,
                glow: Some(0xFFE08C),
            },
            Actor {
                guid: 2,
                x: 100.0,
                y: 100.0,
                z: cam.z,
                quat: [1.0, 0.0, 0.0, 0.0],
                mtable_id: None,
                setup_id: Some(0x02000002),
                model_parts: None,
                motion_cmd: 0,
                style: 0x3d,
                glow: Some(0xFF965A),
            },
        ];
        let reticle = ReticleTarget {
            x: 100.0,
            y: 100.0,
            z: cam.z,
            r: 0.7,
            half_h: 1.4,
        };
        let frame = render_frame(
            &mut scene,
            &mut zoom,
            &mut anim,
            &statics,
            &doors,
            lb,
            &cam,
            &actors,
            80,
            24,
            true,
            Some(&reticle),
        )
        .expect("frame for a known landblock");
        assert_eq!(frame.cols, 80);
        assert_eq!(frame.rows, 24);
        assert_eq!(frame.cells.len(), 80 * 24);
        // Not all sky: some geometry landed in the pane.
        let sky = frame.cells[0];
        assert!(
            frame.cells.iter().any(|c| *c != sky),
            "frame should contain non-sky cells"
        );
        // The animation clock advanced for the actors.
        assert_eq!(anim.len(), 2);
        assert!(anim[&1].last_now > 0.0);
        // A second frame (zoom ease) also renders.
        let frame2 = render_frame(
            &mut scene,
            &mut zoom,
            &mut anim,
            &statics,
            &doors,
            lb,
            &cam,
            &actors,
            80,
            24,
            false,
            None,
        )
        .expect("second frame");
        assert_eq!(frame2.cells.len(), 80 * 24);
    }

    #[test]
    fn zoom_notches_clamp_and_change_back() {
        // A single notch in / out moves the base distance from the default.
        let mut zin = Zoom::new();
        zin.zoom_in();
        assert!(zin.user_back() < CAM_BACK, "zoom-in lowers the base distance");
        let mut zout = Zoom::new();
        zout.zoom_out();
        assert!(zout.user_back() > CAM_BACK, "zoom-out raises the base distance");

        // Many notches clamp at the limits and stay clamped.
        let mut zin = Zoom::new();
        for _ in 0..64 {
            zin.zoom_in();
        }
        assert!((zin.user_back() - BACK_MIN).abs() < 1e-3, "zoom-in clamps at BACK_MIN");
        let mut zout = Zoom::new();
        for _ in 0..64 {
            zout.zoom_out();
        }
        assert!((zout.user_back() - BACK_MAX).abs() < 1e-3, "zoom-out clamps at BACK_MAX");
    }

    #[test]
    fn zoom_changes_framing() {
        let Some(mut scene) = test_scene() else {
            eprintln!("skipped: retail dats unavailable");
            return;
        };
        let lb = 0x3073u32;
        let statics = scene.solids_for_block(lb);
        let doors = HashMap::new();
        let cam = Camera {
            x: 96.0,
            y: 96.0,
            z: scene.height_at(lb, 96.0, 96.0).unwrap_or(0.0),
            heading: 0.7,
        };
        // Same camera + terrain, only the zoom level differs. Stationary
        // (no actors / animation) so the only variable is the camera dolly.
        let mut render = |zoom: &mut Zoom| {
            let mut anim = HashMap::new();
            render_frame(
                &mut scene,
                zoom,
                &mut anim,
                &statics,
                &doors,
                lb,
                &cam,
                &[],
                80,
                24,
                false,
                None,
            )
            .expect("frame for a known landblock")
            .cells
        };
        let mut z = Zoom::new();
        let default = render(&mut z);
        let mut zin = Zoom::new();
        for _ in 0..4 {
            zin.zoom_in();
        }
        let close = render(&mut zin);
        let mut zout = Zoom::new();
        for _ in 0..4 {
            zout.zoom_out();
        }
        let far = render(&mut zout);
        assert_ne!(close, default, "zoom-in changes the framing");
        assert_ne!(far, default, "zoom-out changes the framing");
        assert_ne!(close, far, "closer and farther differ");
    }

    #[test]
    fn render_degrades_without_dats() {
        let mut scene = Scene::unavailable(Some("/nonexistent-ac-dats".to_string()));
        let mut zoom = Zoom::new();
        let mut anim = HashMap::new();
        let cam = Camera {
            x: 96.0,
            y: 96.0,
            z: 0.0,
            heading: 0.0,
        };
        // No dats, no actor geometry to resolve: nothing to draw.
        let frame = render_frame(
            &mut scene,
            &mut zoom,
            &mut anim,
            &[],
            &HashMap::new(),
            0,
            &cam,
            &[],
            80,
            24,
            true,
            None,
        );
        assert!(frame.is_none());
        // Too small is also nothing.
        let mut scene = Scene::unavailable(Some("/nonexistent-ac-dats".to_string()));
        assert!(
            render_frame(
                &mut scene,
                &mut zoom,
                &mut anim,
                &[],
                &HashMap::new(),
                0,
                &cam,
                &[],
                2,
                2,
                false,
                None,
            )
            .is_none()
        );
    }
}
