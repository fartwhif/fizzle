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

/// One static solid in a landblock (the same GfxObjs the 3D view draws):
/// world-space block-frame triangles (`9` floats per tri: 3 verts) + bbox.
#[derive(Clone, Debug, Default)]
pub struct Solid {
    pub tris: Vec<f32>,
    pub bbox: Aabb,
}

/// The world-geometry queries the movement sim runs its steps against
/// (terrain height + solid blocking). `None` / `false` = "no info", which
/// keeps the sim's flat-plane behavior.
pub trait WorldProbe {
    /// Terrain height at block-local `(x, y)` (client frame), or `None`.
    fn height_at(&mut self, x: f32, y: f32) -> Option<f32>;
    /// True when moving the body from `(x0,y0,z0)` to `(x1,y1,z1)` would
    /// intersect a solid (wall, building, closed door). Endpoint `z`s are the
    /// terrain heights at the endpoints (so slopes stay walkable).
    fn blocked(&mut self, x0: f32, y0: f32, z0: f32, x1: f32, y1: f32, z1: f32) -> bool;
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

/// Wrap a client-frame coordinate into the `[0, 192)` block frame.
#[must_use]
pub fn wrap_block_coord(v: f32) -> f32 {
    const BLOCK_LEN: f32 = 192.0;
    let m = v % BLOCK_LEN;
    if m < 0.0 { m + BLOCK_LEN } else { m }
}

/// Compute (cached) the Dereth continent landmask from the retail cell dat.
/// `None` when the dats are unavailable. The first call reads every
/// CellLandblock record (~0.5 s); later calls are instant.
#[must_use]
pub fn get_dereth_land(dat_dir: Option<&str>) -> Option<DerethLand> {
    dereth::get_dereth_land(dat_dir)
}

/// The 3D world: owns the retail-DAT scene, the terrain/collision probe, the
/// camera-zoom state, and per-actor animation clocks. All access is on the
/// owning thread (the main loop). `available() == false` (no dats) → every
/// probe query is "no info" and [`World3D::render`] returns `None`.
pub struct World3D {
    scene: scene::Scene,
    doors: std::collections::HashMap<u32, (Aabb, bool)>,
    cur_lb: u32,
    statics: Vec<Solid>,
    zoom: render::Zoom,
    anim: std::collections::HashMap<u32, render::AnimClock>,
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
            zoom: render::Zoom::new(),
            anim: std::collections::HashMap::new(),
        }
    }

    /// Whether the retail dats were found.
    #[must_use]
    pub fn available(&self) -> bool {
        self.scene.available()
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

    /// Re-base the static solid geometry on a landblock (no-op when unchanged).
    pub fn set_landblock(&mut self, lb: u32) {
        if lb == self.cur_lb {
            return;
        }
        self.cur_lb = lb;
        self.statics = self.scene.solids_for_block(lb);
    }

    /// Add/update a closed-door collision slab (`None` aabb drops it).
    pub fn set_door(&mut self, guid: u32, aabb: Option<Aabb>, open: bool) {
        match aabb {
            Some(b) => { self.doors.insert(guid, (b, open)); }
            None => { self.doors.remove(&guid); }
        }
    }

    /// Drop a door slab.
    pub fn remove_door(&mut self, guid: u32) {
        self.doors.remove(&guid);
    }

    /// The composed setup-model AABB for a closed door (the collision slab).
    #[must_use]
    pub fn setup_aabb(&self, setup_id: u32) -> Option<Aabb> {
        self.scene.setup_aabb(setup_id)
    }

    /// Render the landblock `lb` from the camera `cam` into `cols × rows`.
    /// `actors` are the visible bodies; `glow` toggles the actor halos;
    /// `reticle` is the current target box (or `None`); `reticle_animated`
    /// toggles the reticle's breathing pulse. `None` when there is nothing to
    /// draw (no dats, no terrain and no actors, or too small).
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
    ) -> Option<Frame3D> {
        render::render_frame(
            &mut self.scene,
            &mut self.zoom,
            &mut self.anim,
            &self.statics,
            &self.doors,
            lb,
            cam,
            actors,
            cols,
            rows,
            glow,
            reticle,
            reticle_animated,
        )
    }
}

impl WorldProbe for World3D {
    fn height_at(&mut self, x: f32, y: f32) -> Option<f32> {
        self.scene.height_at(self.cur_lb, x, y)
    }

    fn blocked(&mut self, x0: f32, y0: f32, z0: f32, x1: f32, y1: f32, z1: f32) -> bool {
        collision::blocked(
            &self.statics,
            &self.doors,
            x0, y0, z0,
            x1, y1, z1,
        )
    }
}
