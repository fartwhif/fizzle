//! acgeo — Asheron's Call geometry & 3D world.
//!
//! Ported from `acclient-ts` (`world3d.ts`, `acegeom.ts`, `dats.ts`,
//! `collision.ts`, `dereth.ts`): retail DAT container parsing, per-landblock
//! terrain + static meshes, animated actor meshes, a software 3D rasterizer
//! (follow camera, z-buffer, fog, actor glow, target reticle), the
//! terrain/solid/door collision probe for the movement sim, and the Dereth
//! continent landmask for the Map widget.
//!
//! The retail dats (`client_cell_1.dat` + `client_portal.dat`) are resolved
//! from an explicit dir override, else `$AC_DAT_DIR`, else the standard
//! locations. When the dats are absent, everything degrades to "no info" so
//! the client keeps its flat-plane / tactical behavior.

pub mod acegeom;
pub mod collision;
pub mod dats;
pub mod dereth;
pub mod render;
pub mod scene;

/// The humanoid body scale (re-exported from [`acegeom`]); callers that build
/// [`Actor`]s set it for creatures/players and 1.0 for objects.
pub use acegeom::ACTOR_SCALE;

/// A world-space axis-aligned box: `[min_x, min_y, min_z, max_x, max_y, max_z]`.
pub type Aabb = [f32; 6];

/// Transform a model-space [`Aabb`] into world space: rotate its 8 corners
/// by `quat` (a `[w, x, y, z]` quaternion), translate by `origin`, and take
/// the axis-aligned bounds of the result. `setup_aabb` returns the
/// model-space box; this is the world box a door's collision slab is built
/// from, and is shared by the app (ObjectCreate/Motion) and the tests.
#[must_use]
pub fn transform_aabb(aabb: Aabb, origin: [f32; 3], quat: [f32; 4]) -> Aabb {
    let (w, qx, qy, qz) = (quat[0], quat[1], quat[2], quat[3]);
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for cx in [aabb[0], aabb[3]] {
        for cy in [aabb[1], aabb[4]] {
            for cz in [aabb[2], aabb[5]] {
                // t = 2 * cross(qv, p); p' = p + w*t + cross(qv, t)
                let tx = 2.0 * (qy * cz - qz * cy);
                let ty = 2.0 * (qz * cx - qx * cz);
                let tz = 2.0 * (qx * cy - qy * cx);
                let rx = cx + w * tx + (qy * tz - qz * ty) + origin[0];
                let ry = cy + w * ty + (qz * tx - qx * tz) + origin[1];
                let rz = cz + w * tz + (qx * ty - qy * tx) + origin[2];
                mn[0] = mn[0].min(rx);
                mn[1] = mn[1].min(ry);
                mn[2] = mn[2].min(rz);
                mx[0] = mx[0].max(rx);
                mx[1] = mx[1].max(ry);
                mx[2] = mx[2].max(rz);
            }
        }
    }
    [mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]]
}

/// One static solid in a landblock (the same GfxObjs the 3D view draws):
/// world-space block-frame triangles (`9` floats per tri: 3 verts) + bbox.
#[derive(Clone, Debug, Default)]
pub struct Solid {
    pub tris: Vec<f32>,
    pub bbox: Aabb,
    /// True when this solid is a `buildings`-list GfxObj. Its detailed visual
    /// mesh is drawn separately (the building soup), so the flat static box is
    /// skipped in [`raster_statics`](crate::render) — drawing both z-fights.
    /// Collision still uses the solid. False for props and env cells.
    pub building: bool,
}

/// The world-geometry queries the movement sim runs its steps against
/// (terrain height + solid blocking). `None` / `false` = "no info", which
/// keeps the sim's flat-plane behavior.
pub trait WorldProbe {
    /// Terrain height at block-local `(x, y)` (client frame), or `None`.
    /// `cur_z` is the caller's current height (the upper-floor lift is
    /// referenced to it so a first-floor foot does not snap to the second
    /// floor's ceiling).
    fn height_at(&mut self, x: f32, y: f32, cur_z: f32) -> Option<f32>;
    /// True when moving the body from `(x0,y0,z0)` to `(x1,y1,z1)` would
    /// intersect a solid (wall, building, closed door). Endpoint `z`s are the
    /// terrain heights at the endpoints (so slopes stay walkable).
    fn blocked(&mut self, x0: f32, y0: f32, z0: f32, x1: f32, y1: f32, z1: f32) -> bool;
    /// The block-local region AABB of env cell `cell` (num < 0x10000) in
    /// landblock `lb`, or `None` (no such env cell / dats unavailable).
    fn env_cell_region(&mut self, lb: u32, cell: u32) -> Option<Aabb>;
    /// The env cell in landblock `lb` whose region contains block-local
    /// `(x, y)`, as the full cell id (`lb << 16 | num`), or `None`.
    fn env_cell_at(&mut self, lb: u32, x: f32, y: f32) -> Option<u32>;
}

/// A third-person follow camera in block-local client coordinates.
#[derive(Clone, Copy, Debug, Default)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Heading, radians (movement-sim convention: 0 = +Y, + toward +X).
    pub heading: f32,
}

/// One visible body (player / creature / object) to render in the 3D view.
/// The animation clock per `guid` is kept inside [`World3D`], so this is a
/// per-frame snapshot.
#[derive(Clone, Debug)]
pub struct Actor {
    pub guid: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Orientation quaternion `(w, x, y, z)`.
    pub quat: [f32; 4],
    pub mtable_id: Option<u32>,
    pub setup_id: Option<u32>,
    /// Per-part GfxObj model overrides (`part index → model id`).
    pub model_parts: Option<Vec<(usize, u32)>>,
    /// Retail motion command (e.g. `WalkForward`, `Ready`) — drives the cycle.
    pub motion_cmd: u32,
    /// Motion-table style (e.g. `0x8000003d` non-combat).
    /// Motion-table style (e.g. `0x8000003d` non-combat).
    pub style: u32,
    /// Actor glow-halo color (`0xRRGGBB`), or `None` for no glow. The caller
    /// (which knows self / player / creature) sets it: self = gold, other
    /// players = cyan, creatures = orange.
    pub glow: Option<u32>,
    /// Model scale (1.0 for objects/doors; the humanoid body scale for
    /// creatures/players, whose retail meshes are ~0.825 u tall).
    pub scale: f32,
}

/// The current target's body box, for the 2D reticle overlay (feet position).
#[derive(Clone, Copy, Debug)]
pub struct ReticleTarget {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Body half-width (world units).
    pub r: f32,
    /// Body half-height, feet up (world units).
    pub half_h: f32,
}

/// One rendered frame: `cols × rows` cells, each an RGB color to draw a
/// full-block `█` with.
#[derive(Clone, Debug)]
pub struct Frame3D {
    pub cols: usize,
    pub rows: usize,
    /// Row-major RGB per cell.
    pub cells: Vec<[u8; 3]>,
    /// Whether the frame rendered in first-person (head) mode — a cramped
    /// room forced the follow camera to the head. The click-pick projection
    /// must use the matching first-person camera basis to line up with the
    /// on-screen framing.
    pub first_person: bool,
}

/// The Dereth continent landmask (for the Map widget).
#[derive(Clone, Debug)]
pub struct DerethLand {
    pub min_x: u32,
    pub max_x: u32,
    pub min_y: u32,
    pub max_y: u32,
    /// `255 × 255`; index = `y * 255 + x`; `1` = land.
    pub land: Vec<u8>,
}

/// True when block-local `(x, y)` is inside `aabb`'s horizontal extent
/// (± `margin`). The AABB may sit in an unwrapped world frame (an env
/// cell's placement frame can put its structure at a negative offset), so
/// each axis is compared on the 192-unit ring: a region at y = -35..-25
/// contains the wrapped y = 163.52.
#[must_use]
pub fn region_contains_xy(aabb: &Aabb, x: f32, y: f32, margin: f32) -> bool {
    const L: f32 = 192.0;
    let ring = |a: f32, b: f32| {
        let d = (a - b).rem_euclid(L);
        d.min(L - d)
    };
    let dist_to_segment = |p: f32, lo: f32, hi: f32| {
        let l = ring(lo, hi);
        let dlo = ring(p, lo);
        let dhi = ring(p, hi);
        if dlo + dhi <= l + 1e-3 {
            0.0
        } else {
            dlo.min(dhi)
        }
    };
    dist_to_segment(x, aabb[0], aabb[3]) <= margin
        && dist_to_segment(y, aabb[1], aabb[4]) <= margin
}

/// Compute (cached) the Dereth continent landmask from the retail cell dat.
/// `None` when the dats are unavailable. The first call reads every
/// CellLandblock record (~0.5 s); later calls are instant.
#[must_use]
pub fn get_dereth_land(dat_dir: Option<&str>) -> Option<DerethLand> {
    dereth::get_dereth_land(dat_dir)
}

/// Landblock edge length (world units): the block frame is periodic in it.
const BLOCK_LEN: f32 = 192.0;

/// Translate a block-local coordinate from the frame of landblock `from`
/// into the frame of landblock `to`: the origin-offset delta between the
/// two (landblock = `block_x << 8 | block_y`, `BLOCK_LEN` per block).
#[inline]
fn block_frame_delta(from: u32, to: u32) -> (f32, f32) {
    let dx = (((from >> 8) & 0xFF) as i32 - ((to >> 8) & 0xFF) as i32) as f32 * BLOCK_LEN;
    let dy = ((from & 0xFF) as i32 - (to & 0xFF) as i32) as f32 * BLOCK_LEN;
    (dx, dy)
}

/// The 3D world: owns the retail-DAT scene, the terrain/collision probe, the
/// camera-zoom state, and per-actor animation clocks. All access is on the
/// owning thread (the main loop). `available() == false` (no dats) → every
/// probe query is "no info" and [`World3D::render`] returns `None`.
pub struct World3D {
    scene: scene::Scene,
    /// Door collision slabs, block-local to the current landblock
    /// (re-based by `set_landblock` on a crossing).
    doors: std::collections::HashMap<u32, render::Door>,
    cur_lb: u32,
    statics: Vec<Solid>,
    /// The env (indoor/structure) cell physics solids for the current
    /// landblock — a town's walls (its LBI has no stabs). Collision-only
    /// (the renderer draws the env soup); re-based on a landblock change.
    env_statics: Vec<Solid>,
    /// The env-cell room for the current landblock (the building interior —
    /// walls/floors/ceilings) as retail draws it: the CellStruct **drawing**
    /// polygons (the `env_room` source), not the coarser physics skin that
    /// collision uses (`env_statics`). Drawn as the room by the renderer;
    /// re-based on a landblock change.
    env_skin: Vec<Solid>,
    /// The 3×3 landblock geometry grid centered on the current block
    /// (index 4 = current). Fetched in `set_landblock`, drawn in `render`:
    /// buildings/props in a neighbor block stay visible until the fog takes
    /// them instead of popping out when the player crosses a boundary.
    neighbors: render::Neighborhood,
    zoom: render::Zoom,
    anim: std::collections::HashMap<u32, render::AnimClock>,
    /// Whether the last rendered frame used the first-person (head) camera —
    /// a cramped room. The click-pick projection reads this to match the
    /// on-screen camera basis.
    last_first_person: bool,
}

impl World3D {
    /// Open the retail dats (`dat_dir` overrides `$AC_DAT_DIR` / standard paths).
    #[must_use]
    pub fn new(dat_dir: Option<&str>) -> Self {
        let scene = scene::Scene::new(dat_dir);
        Self {
            scene,
            doors: std::collections::HashMap::new(),
            cur_lb: 0,
            statics: Vec::new(),
            env_statics: Vec::new(),
            env_skin: Vec::new(),
            neighbors: std::array::from_fn(|_| render::BlockGeom::default()),
            zoom: render::Zoom::new(),
            anim: std::collections::HashMap::new(),
            last_first_person: false,
        }
    }

    /// Whether the retail dats were found.
    #[must_use]
    pub fn available(&self) -> bool {
        self.scene.available()
    }

    /// The terrain height at block-local `(x, y)` in the given landblock,
    /// loading the block's height grid on demand. `None` when the dats have
    /// no terrain for the block. Unlike [`WorldProbe::height_at`] this takes
    /// an explicit landblock, so it can ground entities in neighbouring
    /// blocks (not just the player's). `cur_z` is the entity's current height
    /// (the upper-floor lift is referenced to it).
    #[must_use]
    pub fn terrain_height(&mut self, lb: u32, x: f32, y: f32, cur_z: f32) -> Option<f32> {
        self.scene.height_at(lb, x, y, cur_z)
    }

    /// Zoom the 3D camera in (closer) by one wheel notch.
    pub fn zoom_in(&mut self) {
        self.zoom.zoom_in();
    }

    /// Zoom the 3D camera out (farther) by one wheel notch.
    pub fn zoom_out(&mut self) {
        self.zoom.zoom_out();
    }

    /// The camera's current (smoothed) back distance, in world units — the
    /// distance the renderer is using this frame. The fizzle click-pick
    /// projection uses this to line up with the on-screen framing after a zoom.
    #[must_use]
    pub fn camera_back(&self) -> f32 {
        self.zoom.back()
    }

    /// Whether the last rendered frame used the first-person (head) camera
    /// (a cramped room pushed the follow camera in). The click-pick
    /// projection uses this to build the same camera basis as the renderer.
    #[must_use]
    pub fn camera_first_person(&self) -> bool {
        self.last_first_person
    }

    /// Whether the camera at `from` has line of sight to the point `to` (both
    /// in the block-local frame the renderer uses): the straight segment
    /// between them is clear of the current landblock's terrain, static
    /// geometry, and closed doors. Gates click-picking so an occluded entity
    /// can't be targeted. `ignore_door` excludes one door's own slab from the
    /// test: a door's pick anchor sits inside its closed slab, so without the
    /// skip a closed door could never see itself.
    #[must_use]
    pub fn line_of_sight(&self, from: [f32; 3], to: [f32; 3], ignore_door: Option<u32>) -> bool {
        !render::segment_blocked(
            self.scene.block_soup(self.cur_lb),
            &self.statics,
            &self.env_statics,
            &self.doors,
            from,
            to,
            ignore_door,
        )
    }

    /// Re-base the static solid geometry on a landblock (no-op when unchanged).
    pub fn set_landblock(&mut self, lb: u32) {
        if lb == self.cur_lb {
            return;
        }
        // The door slabs are stored in the old block frame; shift them into
        // the new frame (the same delta the actors get on the crossing), so
        // a door or sign in the player's old block stays where it is on
        // screen instead of landing a block away.
        let (dx, dy) = block_frame_delta(self.cur_lb, lb);
        if dx != 0.0 || dy != 0.0 {
            for d in self.doors.values_mut() {
                d.aabb[0] += dx;
                d.aabb[1] += dy;
                d.aabb[3] += dx;
                d.aabb[4] += dy;
            }
        }
        self.cur_lb = lb;
        self.statics = self.scene.solids_for_block(lb);
        self.env_statics = self.scene.env_statics(lb);
        self.env_skin = self.scene.env_room(lb);
        // Fetch the 3×3 geometry grid around the player's block (cheap when
        // the blocks are already scene-cached: one LRU lookup each).
        let bx = ((lb >> 8) & 0xFF) as i32;
        let by = (lb & 0xFF) as i32;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let nbl = render::landblock_at(bx + dx, by + dy);
                self.neighbors[((dy + 1) * 3 + (dx + 1)) as usize] = render::BlockGeom {
                    statics: self.scene.solids_for_block(nbl),
                    env: self.scene.env_soup(nbl),
                    props: self.scene.prop_soup(nbl),
                    buildings: self.scene.building_soup(nbl),
                    env_props: self.scene.env_props(nbl),
                };
            }
        }
    }

    /// Add/update a door collision slab (`None` aabb drops it). `aabb` is
    /// block-local to the door's own landblock `block` (the server's entity
    /// cell); it is stored in the current block frame — the frame the
    /// renderer draws in — so a door or sign in a neighbor block stays put
    /// when the player crosses. `open` sets the swing's goal; any in-flight
    /// swing is preserved so the door eases to the new state rather than
    /// snapping.
    pub fn set_door(&mut self, guid: u32, aabb: Option<Aabb>, open: bool, block: u32) {
        match aabb {
            Some(mut b) => {
                let (dx, dy) = block_frame_delta(block, self.cur_lb);
                b[0] += dx;
                b[1] += dy;
                b[3] += dx;
                b[4] += dy;
                let goal = if open { 1.0 } else { 0.0 };
                let open_cur = self.doors.get(&guid).map(|d| d.open).unwrap_or(0.0);
                self.doors.insert(
                    guid,
                    render::Door { aabb: b, open: open_cur, goal, last_now: -1.0 },
                );
            }
            None => { self.doors.remove(&guid); }
        }
    }

    /// Drop a door slab.
    pub fn remove_door(&mut self, guid: u32) {
        self.doors.remove(&guid);
    }

    /// Advance the door swings on the wall clock (frame-rate independent).
    /// The app calls this every tick so a door's collision state never
    /// depends on the 3D pane being drawn (the render pass advances the
    /// same swings; the two are idempotent through `Door.last_now`).
    pub fn tick(&mut self) {
        render::advance_doors(&mut self.doors, render::now_secs());
    }

    /// The composed setup-model AABB for a closed door (the collision slab).
    #[must_use]
    pub fn setup_aabb(&self, setup_id: u32) -> Option<Aabb> {
        self.scene.setup_aabb(setup_id)
    }

    /// Render the landblock `lb` from the camera `cam` into `cols × rows`.
    /// `actors` are the visible bodies; `glow` toggles the actor halos;
    /// `reticle` is the current target box (or `None`); `reticle_animated`
    /// toggles the reticle's breathing pulse; `self_guid` is the player's own
    /// guid (hidden from the render in first-person, where the head camera
    /// would otherwise show the body filling the view). `None` when there is
    /// nothing to draw (no dats, no terrain and no actors, or too small).
    #[must_use]
    pub fn render(
        &mut self,
        lb: u32,
        cam: &Camera,
        actors: &[Actor],
        cols: usize,
        rows: usize,
        glow: bool,
        reticle: Option<&ReticleTarget>,
        reticle_animated: bool,
        self_guid: Option<u32>,
    ) -> Option<Frame3D> {
        let frame = render::render_frame(
            &mut self.scene,
            &mut self.zoom,
            &mut self.anim,
            &self.env_statics,
            &self.env_skin,
            &self.neighbors,
            &mut self.doors,
            lb,
            cam,
            actors,
            cols,
            rows,
            glow,
            reticle,
            reticle_animated,
            self_guid,
        );
        self.last_first_person = frame.as_ref().is_some_and(|f| f.first_person);
        frame
    }
}

impl WorldProbe for World3D {
    fn height_at(&mut self, x: f32, y: f32, cur_z: f32) -> Option<f32> {
        self.scene.height_at(self.cur_lb, x, y, cur_z)
    }

    fn blocked(&mut self, x0: f32, y0: f32, z0: f32, x1: f32, y1: f32, z1: f32) -> bool {
        collision::blocked(
            &self.statics,
            &self.env_statics,
            &self.doors,
            x0, y0, z0,
            x1, y1, z1,
        )
    }

    fn env_cell_region(&mut self, lb: u32, cell: u32) -> Option<Aabb> {
        self.scene
            .env_cells(lb)
            .into_iter()
            .find(|(c, _)| *c == cell)
            .map(|(_, b)| b)
    }

    fn env_cell_at(&mut self, lb: u32, x: f32, y: f32) -> Option<u32> {
        self.scene
            .env_cells(lb)
            .into_iter()
            .find(|(_, b)| region_contains_xy(b, x, y, 0.0))
            .map(|(c, _)| (lb << 16) | c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lifestone's env region sits in an unwrapped frame (y = -35..-25);
    /// the sim's position is block-wrapped (y = 163.52). Both must agree.
    #[test]
    fn region_contains_wrapped_frame() {
        let r: Aabb = [5.0, -35.0, 0.0, 15.0, -25.0, 6.0];
        assert!(region_contains_xy(&r, 12.32, 163.52, 0.0));
        assert!(region_contains_xy(&r, 12.32, 158.0, 0.0));
        // Just past the region's edge (wrapped hi = 167): outside.
        assert!(!region_contains_xy(&r, 12.32, 170.0, 0.0));
        // Past the wrapped lo (157): outside.
        assert!(!region_contains_xy(&r, 12.32, 155.0, 0.0));
    }

    #[test]
    fn region_contains_plain_frame() {
        let r: Aabb = [10.0, 160.0, 0.0, 15.0, 167.0, 3.0];
        assert!(region_contains_xy(&r, 12.0, 163.0, 0.0));
        assert!(!region_contains_xy(&r, 16.0, 163.0, 0.0));
        assert!(!region_contains_xy(&r, 12.0, 168.0, 0.0));
    }

    #[test]
    fn region_contains_crossing_the_block_edge() {
        // x spans 188..192 and 0..4 (the region crosses the 0/192 edge).
        let r: Aabb = [188.0, 190.0, 0.0, 4.0, 2.0, 6.0];
        assert!(region_contains_xy(&r, 0.0, 1.0, 0.0));
        assert!(region_contains_xy(&r, 189.0, 1.0, 0.0));
        assert!(!region_contains_xy(&r, 96.0, 1.0, 0.0));
    }

    #[test]
    fn region_contains_margin() {
        let r: Aabb = [10.0, 10.0, 0.0, 20.0, 20.0, 6.0];
        assert!(region_contains_xy(&r, 9.6, 15.0, 0.5)); // 0.4 outside the edge
        assert!(!region_contains_xy(&r, 9.4, 15.0, 0.5)); // 0.6 outside
        assert!(region_contains_xy(&r, 15.0, 20.3, 0.5));
        assert!(!region_contains_xy(&r, 15.0, 21.0, 0.5));
    }

    /// Regression: town walls exist only in env-cell physics (a town's
    /// LBI has no stabs), in an unwrapped placement frame. The lifestone
    /// room of retail town 0x8502 (cell 0x850201ad, env 0x0d0001c9, region
    /// x 5..15, unwrapped y -35..-25): a player at the wrapped
    /// (12.0, 160.0) — clear of the room's solid props (furniture, fences)
    /// — must be blocked by the west wall (unwrapped x = 5), may walk
    /// inside the room, and the renderer must show walls/ceiling.
    #[test]
    fn town_walls_render_and_collide() {
        let Some(dir) = dats::find_dat_dir(None) else {
            eprintln!("skipped: retail dats unavailable");
            return;
        };
        let mut world = World3D::new(Some(&dir));
        assert!(world.available());
        let lb = 0x8502u32;
        world.set_landblock(lb);

        // The town's LBI has no stabs — its walls come from env cells.
        assert!(world.statics.is_empty());
        assert!(!world.env_statics.is_empty());

        let (px, py) = (12.0f32, 160.0f32);
        let z0 = world.height_at(px, py, 0.0).unwrap_or(0.0);
        // West toward the room's west wall (unwrapped x = 5): blocked.
        assert!(world.blocked(px, py, z0, 4.0, py, z0));
        // North, inside the room: clear.
        assert!(!world.blocked(px, py, z0, px, py + 3.0, z0));

        // The renderer must draw the room: walls/ceiling cover the view.
        let cam = Camera { x: px, y: py, z: z0, heading: 0.0 };
        let Some(frame) = world.render(lb, &cam, &[], 80, 24, false, None, false, None) else {
            panic!("no frame for the town block");
        };
        let sky = frame.cells[0];
        let non_sky = frame.cells.iter().filter(|c| **c != sky).count();
        assert!(non_sky > 100, "town interior rendered almost empty: {non_sky}");
    }

    /// Regression: `set_landblock` must re-base the terrain probe's
    /// landblock. A missing `cur_lb` update made `height_at` always query
    /// landblock 0 — an all-zero record that pinned every grounded player to
    /// z = 0, so teleporting to hilly terrain left the player far below the
    /// ground while the renderer drew the correct block's terrain above.
    /// (Flat-0 town tests cannot catch this: block 0 is also 0.)
    #[test]
    fn probe_tracks_set_landblock() {
        let Some(dir) = dats::find_dat_dir(None) else {
            eprintln!("skipped: retail dats unavailable");
            return;
        };
        let mut world = World3D::new(Some(&dir));
        assert!(world.available());
        let lb = 0x3073u32; // (48, 115): hilly terrain, z 36..44
        world.set_landblock(lb);
        let probe = world.height_at(96.0, 96.0, 0.0).expect("probe terrain height");
        let scene = world.scene.height_at(lb, 96.0, 96.0, 0.0).expect("scene terrain height");
        assert_eq!(probe, scene, "the probe must query the set landblock");
        assert!(probe > 1.0, "hilly block is not flat: {probe}");
    }
}
