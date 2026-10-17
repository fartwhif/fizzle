//! Terrain/solid/door collision for the movement sim: two body-height rays
//! (knee + chest) against per-landblock static solids + closed-door slabs.
//! Ported from `acclient-ts/src/collision.ts`.
//!
//! Coordinates: the client frame (the sim's raw x/y; the block geometry is
//! `[0, BLOCK_LEN)` and is made periodic around the query point to match).

use std::collections::HashMap;

use crate::{Aabb, Solid};

const BLOCK_LEN: f32 = 192.0;
const EPS: f32 = 1e-9;

/// Double-sided segment vs triangle (Möller–Trumbore over the segment).
/// `(o, d)` is the segment start/direction (unit over the segment); the
/// triangle is `(a, b, c)`.
#[must_use]
pub fn seg_tri(
    ox: f32,
    oy: f32,
    oz: f32,
    dx: f32,
    dy: f32,
    dz: f32,
    ax: f32,
    ay: f32,
    az: f32,
    bx: f32,
    by: f32,
    bz: f32,
    cx: f32,
    cy: f32,
    cz: f32,
) -> bool {
    let (e1x, e1y, e1z) = (bx - ax, by - ay, bz - az);
    let (e2x, e2y, e2z) = (cx - ax, cy - ay, cz - az);
    let (hx, hy, hz) = (dy * e2z - dz * e2y, dz * e2x - dx * e2z, dx * e2y - dy * e2x);
    let det = e1x * hx + e1y * hy + e1z * hz;
    if det > -EPS && det < EPS {
        return false;
    }
    let inv = 1.0 / det;
    let (sx, sy, sz) = (ox - ax, oy - ay, oz - az);
    let u = (sx * hx + sy * hy + sz * hz) * inv;
    if u < -EPS || u > 1.0 + EPS {
        return false;
    }
    let (qx, qy, qz) = (sy * e1z - sz * e1y, sz * e1x - sx * e1z, sx * e1y - sy * e1x);
    let v = (dx * qx + dy * qy + dz * qz) * inv;
    if v < -EPS || u + v > 1.0 + EPS {
        return false;
    }
    let t = (e2x * qx + e2y * qy + e2z * qz) * inv;
    t > EPS && t < 1.0 + EPS
}

/// One slab axis of the segment-vs-AABB test; updates `tmin`/`tmax`.
fn axis(o: f32, d: f32, lo: f32, hi: f32, tmin: &mut f32, tmax: &mut f32) -> bool {
    if d.abs() < EPS {
        return o >= lo && o <= hi;
    }
    let mut t1 = (lo - o) / d;
    let mut t2 = (hi - o) / d;
    if t1 > t2 {
        std::mem::swap(&mut t1, &mut t2);
    }
    if t1 > *tmin {
        *tmin = t1;
    }
    if t2 < *tmax {
        *tmax = t2;
    }
    *tmin <= *tmax
}

/// Segment vs AABB (slab method).
#[must_use]
pub fn seg_box(
    ox: f32,
    oy: f32,
    oz: f32,
    dx: f32,
    dy: f32,
    dz: f32,
    b: &Aabb,
) -> bool {
    let (mut tmin, mut tmax) = (0.0f32, 1.0f32);
    axis(ox, dx, b[0], b[3], &mut tmin, &mut tmax)
        && axis(oy, dy, b[1], b[4], &mut tmin, &mut tmax)
        && axis(oz, dz, b[2], b[5], &mut tmin, &mut tmax)
}

/// True when moving a body from `(x0, y0, z0)` to `(x1, y1, z1)` would
/// intersect a solid (wall, building, closed door). The endpoint `z`s are
/// the terrain heights at the endpoints (so slopes stay walkable).
#[must_use]
pub fn blocked(
    statics: &[Solid],
    doors: &HashMap<u32, (Aabb, bool)>,
    x0: f32,
    y0: f32,
    z0: f32,
    x1: f32,
    y1: f32,
    z1: f32,
) -> bool {
    // Two horizontal body-height rays (knee + chest): a solid wall ~0.3u
    // or taller stops the player.
    ray_blocked(statics, doors, x0, y0, z0 + 0.3, x1, y1, z1 + 0.3)
        || ray_blocked(statics, doors, x0, y0, z0 + 1.5, x1, y1, z1 + 1.5)
}

/// One body-height ray against the periodic statics + the closed doors.
fn ray_blocked(
    statics: &[Solid],
    doors: &HashMap<u32, (Aabb, bool)>,
    ax: f32,
    ay: f32,
    az: f32,
    bx: f32,
    by: f32,
    bz: f32,
) -> bool {
    // Statics live in the [0, BLOCK_LEN) block frame; the client frame is
    // periodic in it, so shift the geometry by the block multiple nearest
    // the segment midpoint.
    let mx = (ax + bx) / 2.0;
    let my = (ay + by) / 2.0;
    let ox = BLOCK_LEN * ((mx - BLOCK_LEN / 2.0) / BLOCK_LEN).round();
    let oy = BLOCK_LEN * ((my - BLOCK_LEN / 2.0) / BLOCK_LEN).round();
    let (dx, dy, dz) = (bx - ax, by - ay, bz - az);
    for s in statics {
        let b = s.bbox;
        if ax.max(bx) < b[0] + ox || ax.min(bx) > b[3] + ox {
            continue;
        }
        if ay.max(by) < b[1] + oy || ay.min(by) > b[4] + oy {
            continue;
        }
        let zlo = az.min(bz);
        let zhi = az.max(bz);
        if zhi < b[2] || zlo > b[5] {
            continue;
        }
        let tris = &s.tris;
        let n = tris.len() / 9;
        for i in 0..n {
            let o = i * 9;
            if seg_tri(
                ax, ay, az, dx, dy, dz,
                tris[o] + ox, tris[o + 1] + oy, tris[o + 2],
                tris[o + 3] + ox, tris[o + 4] + oy, tris[o + 5],
                tris[o + 6] + ox, tris[o + 7] + oy, tris[o + 8],
            ) {
                return true;
            }
        }
    }
    // Closed doors (same client frame as the player — no periodicity).
    for &(b, open) in doors.values() {
        if open {
            continue;
        }
        if ax.max(bx) < b[0] || ax.min(bx) > b[3] {
            continue;
        }
        if ay.max(by) < b[1] || ay.min(by) > b[4] {
            continue;
        }
        let zlo = az.min(bz);
        let zhi = az.max(bz);
        if zhi < b[2] || zlo > b[5] {
            continue;
        }
        if seg_box(ax, ay, az, dx, dy, dz, &b) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vertical wall triangle at x = 10, spanning y 0..20, z 0..5.
    fn wall_solid() -> Solid {
        let tris = vec![
            10.0, 0.0, 0.0, 10.0, 20.0, 0.0, 10.0, 20.0, 5.0, //
        ];
        Solid {
            tris,
            bbox: [10.0, 0.0, 0.0, 10.0, 20.0, 5.0],
        }
    }

    #[test]
    fn seg_tri_hits_and_misses() {
        // Crosses the wall triangle (at y=10 the tri covers z 0..2.5).
        assert!(seg_tri(5.0, 10.0, 1.0, 10.0, 0.0, 0.0,
            10.0, 0.0, 0.0, 10.0, 20.0, 0.0, 10.0, 20.0, 5.0));
        // Same line but above the triangle.
        assert!(!seg_tri(5.0, 10.0, 4.0, 10.0, 0.0, 0.0,
            10.0, 0.0, 0.0, 10.0, 20.0, 0.0, 10.0, 20.0, 5.0));
        // Parallel (in the wall plane): no intersection.
        assert!(!seg_tri(9.0, 10.0, 1.0, 0.0, 5.0, 0.0,
            10.0, 0.0, 0.0, 10.0, 20.0, 0.0, 10.0, 20.0, 5.0));
        // Double-sided: from the other side too.
        assert!(seg_tri(15.0, 10.0, 1.0, -10.0, 0.0, 0.0,
            10.0, 0.0, 0.0, 10.0, 20.0, 0.0, 10.0, 20.0, 5.0));
    }

    #[test]
    fn seg_box_hits_and_misses() {
        let b: Aabb = [9.0, 9.0, 0.0, 11.0, 11.0, 4.0];
        assert!(seg_box(5.0, 10.0, 2.0, 10.0, 0.0, 0.0, &b));
        assert!(!seg_box(5.0, 5.0, 2.0, 10.0, 0.0, 0.0, &b));
        // Parallel to the slab, outside it: no hit.
        assert!(!seg_box(12.0, 10.0, 2.0, 0.0, 5.0, 0.0, &b));
        // Inside the box at the start: counts as a hit (tmin=0).
        assert!(seg_box(10.0, 10.0, 2.0, 0.0, 1.0, 0.0, &b));
    }

    #[test]
    fn blocked_by_wall_and_closed_door() {
        let wall = wall_solid();
        let mut doors = HashMap::new();
        // A closed door slab the path must cross.
        doors.insert(1, ([9.0, 18.0, 0.0, 11.0, 20.0, 4.0], false));
        // Crossing the wall (the knee ray at z+0.3 hits it).
        assert!(blocked(&[wall.clone()], &doors, 5.0, 10.0, 0.0, 15.0, 10.0, 0.0));
        // Crossing the closed door.
        assert!(blocked(&[], &doors, 5.0, 19.0, 0.0, 15.0, 19.0, 0.0));
        // The same path with the door open does not block.
        doors.insert(1, ([9.0, 18.0, 0.0, 11.0, 20.0, 4.0], true));
        assert!(!blocked(&[], &doors, 5.0, 19.0, 0.0, 15.0, 19.0, 0.0));
        // A path that avoids everything: it crosses the wall plane at
        // y = 0.5, below the triangle's rising hypotenuse (z < y/4).
        assert!(!blocked(&[wall], &doors, 5.0, 0.5, 0.0, 15.0, 0.5, 0.0));
    }

    #[test]
    fn blocked_respects_periodicity() {
        let wall = wall_solid();
        // The wall is at block-x 10; the client frame at x = 192+5 is the
        // same x, so the geometry must be shifted by +192.
        assert!(blocked(&[wall], &HashMap::new(), 197.0, 10.0, 0.0, 207.0, 10.0, 0.0));
    }
}
