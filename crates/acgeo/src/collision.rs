//! Terrain/solid/door collision for the movement sim: two body-height rays
//! (knee + chest) against per-landblock static solids + closed-door slabs.
//! Ported from `acclient-ts/src/collision.ts`.
//!
//! Coordinates: the client frame (the sim's raw x/y, which can sit mid-step
//! outside the `[0, BLOCK_LEN)` block frame after a boundary crossing). The
//! block geometry is made periodic around the segment to match it.

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
/// intersect a solid (wall, building, closed door). `statics` are the
/// LBI/structure solids and `env_statics` the env-cell physics solids
/// (a town's walls); both are periodic in the block frame. The endpoint
/// `z`s are the terrain heights at the endpoints (so slopes stay
/// walkable).
#[must_use]
pub fn blocked(
    statics: &[Solid],
    env_statics: &[Solid],
    doors: &HashMap<u32, crate::render::Door>,
    x0: f32,
    y0: f32,
    z0: f32,
    x1: f32,
    y1: f32,
    z1: f32,
) -> bool {
    // Two horizontal body-height rays (knee + chest): a solid wall ~0.3u
    // or taller stops the player.
    ray_blocked(statics, env_statics, doors, x0, y0, z0 + 0.3, x1, y1, z1 + 0.3)
        || ray_blocked(statics, env_statics, doors, x0, y0, z0 + 1.5, x1, y1, z1 + 1.5)
}

/// One body-height ray against the periodic statics + the closed doors.
fn ray_blocked(
    statics: &[Solid],
    env_statics: &[Solid],
    doors: &HashMap<u32, crate::render::Door>,
    ax: f32,
    ay: f32,
    az: f32,
    bx: f32,
    by: f32,
    bz: f32,
) -> bool {
    // LBI statics live in the [0, BLOCK_LEN) block frame; env-cell statics
    // live in their unwrapped placement frame (a town's room wraps the
    // block toroidally). Either way the world is periodic in BLOCK_LEN, so
    // test every periodic copy whose bbox overlaps the segment's span —
    // one for an in-block step, up to four when the step crosses a block
    // corner. A midpoint-based single shift misses the wall/door on the
    // far side of a boundary the step straddles.
    let (dx, dy, dz) = (bx - ax, by - ay, bz - az);
    let (xlo, xhi) = (ax.min(bx), ax.max(bx));
    let (ylo, yhi) = (ay.min(by), ay.max(by));
    let (zlo, zhi) = (az.min(bz), az.max(bz));
    for s in statics.iter().chain(env_statics.iter()) {
        let b = s.bbox;
        if zhi < b[2] || zlo > b[5] {
            continue;
        }
        // The copy multiples k whose [b0 + 192k, b3 + 192k] span can
        // overlap [xlo, xhi] (the EPS keeps a grazing copy in range).
        let kx0 = ((xlo - b[3]) / BLOCK_LEN - EPS).ceil() as i64;
        let kx1 = ((xhi - b[0]) / BLOCK_LEN + EPS).floor() as i64;
        let ky0 = ((ylo - b[4]) / BLOCK_LEN - EPS).ceil() as i64;
        let ky1 = ((yhi - b[1]) / BLOCK_LEN + EPS).floor() as i64;
        let tris = &s.tris;
        let n = tris.len() / 9;
        for kx in kx0..=kx1 {
            let ox = (kx as f32) * BLOCK_LEN;
            for ky in ky0..=ky1 {
                let oy = (ky as f32) * BLOCK_LEN;
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
        }
    }
    // Closed doors (same client frame as the player; the slabs tile with
    // the block like the statics). A door stops movement until it has
    // mostly swung clear.
    for d in doors.values() {
        if d.open >= 0.9 {
            continue;
        }
        let b = d.aabb;
        if zhi < b[2] || zlo > b[5] {
            continue;
        }
        let kx0 = ((xlo - b[3]) / BLOCK_LEN - EPS).ceil() as i64;
        let kx1 = ((xhi - b[0]) / BLOCK_LEN + EPS).floor() as i64;
        let ky0 = ((ylo - b[4]) / BLOCK_LEN - EPS).ceil() as i64;
        let ky1 = ((yhi - b[1]) / BLOCK_LEN + EPS).floor() as i64;
        for kx in kx0..=kx1 {
            let ox = (kx as f32) * BLOCK_LEN;
            for ky in ky0..=ky1 {
                let oy = (ky as f32) * BLOCK_LEN;
                if seg_box(
                    ax, ay, az, dx, dy, dz,
                    &[b[0] + ox, b[1] + oy, b[2], b[3] + ox, b[4] + oy, b[5]],
                ) {
                    return true;
                }
            }
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

    /// A vertical wall triangle at x = `x`, spanning y 0..20, z 0..5.
    fn wall_at_x(x: f32) -> Solid {
        Solid {
            tris: vec![x, 0.0, 0.0, x, 20.0, 0.0, x, 20.0, 5.0],
            bbox: [x, 0.0, 0.0, x, 20.0, 5.0],
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
        doors.insert(1, crate::render::Door { aabb: [9.0, 18.0, 0.0, 11.0, 20.0, 4.0], open: 0.0, goal: 0.0, last_now: -1.0 });
        // Crossing the wall (the knee ray at z+0.3 hits it).
        assert!(blocked(&[wall.clone()], &[], &doors, 5.0, 10.0, 0.0, 15.0, 10.0, 0.0));
        // Crossing the closed door.
        assert!(blocked(&[], &[], &doors, 5.0, 19.0, 0.0, 15.0, 19.0, 0.0));
        // The same path with the door open does not block.
        doors.insert(1, crate::render::Door { aabb: [9.0, 18.0, 0.0, 11.0, 20.0, 4.0], open: 1.0, goal: 1.0, last_now: -1.0 });
        assert!(!blocked(&[], &[], &doors, 5.0, 19.0, 0.0, 15.0, 19.0, 0.0));
        // Mid-swing the door still blocks (the player can't clip through a
        // leaf that is only half open); past the 0.9 threshold it doesn't.
        doors.insert(1, crate::render::Door { aabb: [9.0, 18.0, 0.0, 11.0, 20.0, 4.0], open: 0.5, goal: 1.0, last_now: -1.0 });
        assert!(blocked(&[], &[], &doors, 5.0, 19.0, 0.0, 15.0, 19.0, 0.0));
        doors.insert(1, crate::render::Door { aabb: [9.0, 18.0, 0.0, 11.0, 20.0, 4.0], open: 0.95, goal: 1.0, last_now: -1.0 });
        assert!(!blocked(&[], &[], &doors, 5.0, 19.0, 0.0, 15.0, 19.0, 0.0));
        // A path that avoids everything: it crosses the wall plane at
        // y = 0.5, below the triangle's rising hypotenuse (z < y/4).
        assert!(!blocked(&[wall], &[], &doors, 5.0, 0.5, 0.0, 15.0, 0.5, 0.0));
    }

    #[test]
    fn blocked_respects_periodicity() {
        let wall = wall_solid();
        // The wall is at block-x 10; the client frame at x = 192+5 is the
        // same x, so the geometry must be shifted by +192.
        assert!(blocked(&[wall], &[], &HashMap::new(), 197.0, 10.0, 0.0, 207.0, 10.0, 0.0));
    }

    #[test]
    fn blocked_by_unwrapped_env_wall_across_seam() {
        // Regression: a town's walls exist only in env-cell physics, in an
        // unwrapped placement frame (the town room wraps the block
        // toroidally — retail town 0x8502's LBI has no stabs). A wall at
        // unwrapped x = 193 (= wrapped x = 1), y -35..-25 (= wrapped
        // 157..167), z 0..6 must block wrapped-frame steps on both sides of
        // the 0/192 seam, and pass a step clear of the wall's end.
        let wall = Solid {
            // Vertical quad (two triangles) at x = 193, y -35..-25, z 0..6.
            tris: vec![
                193.0, -35.0, 0.0, 193.0, -25.0, 0.0, 193.0, -25.0, 6.0,
                193.0, -35.0, 0.0, 193.0, -25.0, 6.0, 193.0, -35.0, 6.0,
            ],
            bbox: [193.0, -35.0, 0.0, 193.0, -25.0, 6.0],
        };
        // Player at wrapped x = 191 walking +x across the seam: the k = 0
        // copy (x = 193) is crossed.
        assert!(blocked(
            &[],
            &[wall.clone()],
            &HashMap::new(),
            191.0, 161.0, 0.0, 194.0, 161.0, 0.0,
        ));
        // The same step 1u clear of the wall's north end (wrapped y = 168
        // = unwrapped y = -24, past the wall's y = -25 edge).
        assert!(!blocked(
            &[],
            &[wall.clone()],
            &HashMap::new(),
            191.0, 168.0, 0.0, 194.0, 168.0, 0.0,
        ));
        // Player at wrapped x = 0.5 walking +x: the same wall's k = -1 copy
        // (x = 1) is crossed.
        assert!(blocked(
            &[],
            &[wall],
            &HashMap::new(),
            0.5, 161.0, 0.0, 2.0, 161.0, 0.0,
        ));
    }

    #[test]
    fn blocked_far_edge_wall_mid_step_past_block() {
        // Regression: RETAIL.md §3 — the sim keeps its own block's frame,
        // so a position can sit mid-step just past 192. A wall at the
        // block's far edge (local x 191.9) is crossed by the step
        // 191.85 -> 192.20 at its own (k=0) copy; the single periodic
        // shift chosen from the segment midpoint sits a full block away
        // and let the player through the wall.
        let wall = wall_at_x(191.9);
        assert!(blocked(&[wall], &[], &HashMap::new(), 191.85, 10.0, 0.0, 192.2, 10.0, 0.0));
    }

    #[test]
    fn blocked_near_edge_wall_mid_step_before_block() {
        // Mirror of the far-edge case: a wall at the block's near edge
        // (local x 0.1) crossed by the step -0.25 -> 0.2 (a position just
        // past the 0 boundary).
        let wall = wall_at_x(0.1);
        assert!(blocked(&[wall], &[], &HashMap::new(), -0.25, 10.0, 0.0, 0.2, 10.0, 0.0));
    }

    #[test]
    fn blocked_diagonal_step_across_block_corner() {
        // A step crossing both the x and y block boundaries (a block
        // corner) still meets a wall just inside the far-far corner of the
        // block (the plane y = 191.75, x 191.5..192.5).
        let wall = Solid {
            tris: vec![191.5, 191.75, 0.0, 192.5, 191.75, 0.0, 192.5, 191.75, 5.0],
            bbox: [191.5, 191.75, 0.0, 192.5, 191.75, 5.0],
        };
        assert!(blocked(&[wall], &[], &HashMap::new(), 191.7, 191.7, 0.0, 192.3, 192.3, 0.0));
    }

    #[test]
    fn blocked_door_slab_tiles_across_block_boundaries() {
        // Door slabs tile with the block like the statics: a near-edge
        // slab blocks a step just past the far boundary (192.x), and a
        // far-edge slab blocks a step just past the near boundary (-0.x).
        let mut doors = HashMap::new();
        doors.insert(1, crate::render::Door { aabb: [0.0, 9.0, 0.0, 2.0, 11.0, 4.0], open: 0.0, goal: 0.0, last_now: -1.0 });
        assert!(blocked(&[], &[], &doors, 191.95, 10.0, 0.0, 192.3, 10.0, 0.0));
        let mut doors = HashMap::new();
        doors.insert(1, crate::render::Door { aabb: [190.0, 9.0, 0.0, 192.0, 11.0, 4.0], open: 0.0, goal: 0.0, last_now: -1.0 });
        assert!(blocked(&[], &[], &doors, -0.3, 10.0, 0.0, -0.05, 10.0, 0.0));
    }
}
