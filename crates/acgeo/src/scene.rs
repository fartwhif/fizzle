//! Retail-DAT scene: per-landblock terrain soup, static solid geometry,
//! terrain heights, setup-model AABBs, and the per-model caches the
//! animated-actor path uses. Ported from the `WorldScene` half of
//! `acclient-ts/src/world3d.ts`.
//!
//! Everything is in landblock-local coordinates (x, y in `[0, 192)`; the
//! wire positions are local to the landblock, so no global landblock origin
//! is needed). +X east, +Y north, +Z up.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::acegeom;
use crate::acegeom::{Animation, Frame, Mesh, MotionTable, IDENTITY};
use crate::dats::{self, DatDatabase};
use crate::{Aabb, Solid};

/// The region height table id (0x13…); a region's landblocks share it.
const REGION_DESC: u32 = 0x13000000;
/// The "null part" sentinel GfxObj — never rendered.
pub(crate) const NULL_PART: u32 = 0x010001ec;
/// Landblock soup cache cap. The renderer draws the 3×3 landblock grid around
/// the player (9 blocks), so this must hold all nine plus headroom for the
/// stragglers as the player crosses a block boundary.
const MAX_BLOCKS: usize = 16;
/// Part-mesh cache cap (dynamic actors reference a bounded set of models).
const MAX_PART_MESHES: usize = 512;
/// A landblock's edge length in units (an 8x8 grid of 24u subcells).
pub(crate) const BLOCK_LEN: f32 = 192.0;

/// Base terrain color by (terrain type, road bit). Holtburg uses types 1, 3,
/// 8, 13 (grassland / lush / rock / patchy); the rest map to a default.
#[must_use]
pub(crate) fn terrain_color(type_: u16, road: u16) -> [f32; 3] {
    if road != 0 {
        return [122.0, 120.0, 116.0];
    }
    match type_ & 0x1f {
        1 => [96.0, 132.0, 56.0],
        3 => [70.0, 120.0, 46.0],
        8 => [120.0, 112.0, 92.0],
        13 => [112.0, 116.0, 58.0],
        _ => [90.0, 126.0, 56.0],
    }
}

/// A material (0x00RRGGBB solid or 0xffRRGGBB texture average) -> `[r, g, b]`.
/// Only the exact MAT_UNKNOWN sentinel (0xff000000, a palette-only texture
/// with no average) falls back to neutral gray — real texture colors carry
/// 0xff in the high byte, so the high byte alone cannot mean "unknown".
#[must_use]
pub(crate) fn mat_color(m: u32) -> [f32; 3] {
    if m == 0xff000000 {
        return [96.0, 96.0, 96.0];
    }
    [((m >> 16) & 0xff) as f32, ((m >> 8) & 0xff) as f32, (m & 0xff) as f32]
}

/// The retail-DAT scene for one resolved dat directory. Owns the two open
/// dat databases and every parsed/derived geometry cache; all access is on
/// the owning thread (the main loop).
pub struct Scene {
    dir: Option<String>,
    /// `RefCell`: [`Scene::setup_aabb`] (a `&self` contract from the
    /// session) reads dat records while the scene lives behind a shared ref.
    cell: Option<RefCell<DatDatabase>>,
    portal: Option<RefCell<DatDatabase>>,
    height_table: Option<[f32; 256]>,
    /// Terrain triangle soups per landblock (12 floats/tri), LRU-capped.
    blocks: HashMap<u32, Vec<f32>>,
    order: Vec<u32>,
    /// Surface (0x08…) id -> 0x00RRGGBB / 0xffRRGGBB / None (cached).
    surf_cache: HashMap<u32, Option<u32>>,
    /// Terrain height grid (81 floats) per landblock; None = no record.
    height_grids: HashMap<u32, Option<Vec<f32>>>,
    /// Static solid geometry per landblock (the collision + render solids).
    solid_objs: HashMap<u32, Vec<Solid>>,
    /// Env (indoor/structure) cells present per landblock:
    /// `(cellnum < 0x10000, block-local region AABB)`, cached.
    env_regions: HashMap<u32, Vec<(u32, Aabb)>>,
    /// Env (indoor/structure) cell triangle soups per landblock, one entry
    /// per cell: `(cell_id, drawn-vertex region AABB, soup)`. The soup is
    /// 12 floats/tri (3 verts + rgb), the walls/ceiling the renderer draws;
    /// the region lets the renderer skip the cell the camera is inside so a
    /// building shell doesn't wall in its interior. Cached.
    env_soups: HashMap<u32, Vec<(u32, Aabb, Vec<f32>)>>,
    /// Placed GfxObj (0x01) prop visual meshes per landblock, one entry per
    /// prop: `(model_id, drawn-vertex region AABB, soup)`. The soup is 12
    /// floats/tri (3 world-space verts + rgb) from the GfxObj render mesh —
    /// the furniture/barrels/lamps the renderer draws. Pass-through props
    /// (no physics leaf) still get their visual mesh; only the `stabs` list
    /// (placed props), never the `buildings` list (whose visual mesh z-fights
    /// the env-cell shell). Cached.
    prop_soups: HashMap<u32, Vec<(u32, Aabb, Vec<f32>)>>,
    /// Building GfxObj visual meshes per landblock (the `buildings` list):
    /// 12 floats/tri (3 world-space verts + rgb). These are the detailed
    /// multi-material models a town's shops are made of (walls + the interior
    /// props — paintings, barrels, lamps). The renderer draws one only when
    /// the camera is inside its region: from inside, the env-cell shell that
    /// duplicates the building's outer walls is already skipped, so the
    /// interior shows with no z-fight; from outside the env cells represent it.
    building_soups: HashMap<u32, Vec<(u32, Aabb, Vec<f32>)>>,
    /// Near-flat, upward-facing floor planes derived from the building
    /// GfxObj meshes per landblock: one `[ztop, xmin, ymin, xmax, ymax]`
    /// per triangle. These are the shop's interior floors (e.g. the second
    /// floor) that the terrain height grid has no record of; [`Scene::height_at`]
    /// lifts the foot onto the highest one that contains it so the character
    /// stands on an upper floor instead of falling through to the ground.
    /// Cached.
    building_floors: HashMap<u32, Vec<[f32; 5]>>,
    /// GfxObj (0x01) statics placed inside the env (indoor) cells per
    /// landblock — the shop's interior props (stairs, candlesticks,
    /// paintings). 12 floats/tri (3 world-space verts + rgb). The env-cell
    /// statics' frames are block-local (world) space, so they are drawn
    /// directly, like the LBI `stabs`. Cached.
    env_prop_soups: HashMap<u32, Vec<(u32, Aabb, Vec<f32>)>>,
    /// Env (indoor/structure) cell physics solids per landblock (the walls
    /// that block movement; a town's walls exist only here — its LBI has no
    /// stabs). Triangles/bbox in the env cell's unwrapped placement frame;
    /// the collision/occlusion paths test periodic copies. Cached.
    env_solid_objs: HashMap<u32, Vec<Solid>>,
    // Per-id caches for the dynamic (animated) actors.
    setup_parts_cache: HashMap<u32, Option<Vec<u32>>>,
    mtable_cache: HashMap<u32, Option<MotionTable>>,
    anim_cache: HashMap<u32, Option<Animation>>,
    part_mesh_cache: HashMap<u32, Option<Mesh>>,
}

impl Scene {
    /// Open the retail dats (`dat_dir` overrides `$AC_DAT_DIR` / standard
    /// paths). When either database is missing, `available() == false` and
    /// every query degrades to "no info".
    #[must_use]
    pub fn new(dat_dir: Option<&str>) -> Self {
        let mut s = Self::unavailable(None);
        if let Some(dir) = dats::find_dat_dir(dat_dir) {
            let cell = DatDatabase::open(format!("{dir}/client_cell_1.dat"));
            let portal = DatDatabase::open(format!("{dir}/client_portal.dat"));
            if let (Ok(cell), Ok(mut portal)) = (cell, portal) {
                let region = portal.find(REGION_DESC).and_then(|_| portal.read(REGION_DESC));
                s.dir = Some(dir);
                s.cell = Some(RefCell::new(cell));
                s.portal = Some(RefCell::new(portal));
                s.height_table =
                    region.as_deref().and_then(acegeom::parse_region_desc_height_table);
            }
        }
        s
    }

    /// A scene with no dat databases: `available() == false` and every
    /// query degrades to "no info" (used by tests; production paths go
    /// through [`Scene::new`]).
    pub(crate) fn unavailable(dir: Option<String>) -> Self {
        Self {
            dir,
            cell: None,
            portal: None,
            height_table: None,
            blocks: HashMap::new(),
            order: Vec::new(),
            surf_cache: HashMap::new(),
            height_grids: HashMap::new(),
            solid_objs: HashMap::new(),
            env_regions: HashMap::new(),
            env_soups: HashMap::new(),
            prop_soups: HashMap::new(),
            building_soups: HashMap::new(),
            building_floors: HashMap::new(),
            env_prop_soups: HashMap::new(),
            env_solid_objs: HashMap::new(),
            setup_parts_cache: HashMap::new(),
            mtable_cache: HashMap::new(),
            anim_cache: HashMap::new(),
            part_mesh_cache: HashMap::new(),
        }
    }

    /// True when the dats were located and both databases opened.
    #[must_use]
    pub fn available(&self) -> bool {
        self.cell.is_some() && self.portal.is_some()
    }

    /// The resolved dat directory (for diagnostics), or `None`.
    #[must_use]
    pub fn dir_found(&self) -> Option<&str> {
        self.dir.as_deref()
    }

    /// Read a portal-dat record, or `None`.
    fn portal_read(&self, id: u32) -> Option<Vec<u8>> {
        self.portal.as_ref()?.borrow_mut().read(id)
    }

    /// A surface (0x08…) data id resolved to a color, uncached (the `&self`
    /// path — [`Scene::setup_aabb`]).
    fn surface_color_fresh(&self, id: u32) -> Option<u32> {
        let Some(portal) = self.portal.as_ref() else {
            return None;
        };
        let Some(buf) = portal.borrow_mut().read(id) else {
            return None;
        };
        let Some(surf) = acegeom::parse_surface(&buf) else {
            return None;
        };
        // Glass (translucency >= 0.5) is not rendered: returning `None`
        // makes the geometry builder skip the polygon, so window orifices
        // stay see-through instead of painting opaque blue.
        if surf.translucency >= 0.5 {
            return None;
        }
        if let Some(c) = surf.color {
            return Some(c);
        }
        let Some(tid) = surf.texture_id else {
            return None;
        };
        let Some(st) = portal.borrow_mut().read(tid) else {
            return None;
        };
        for t in acegeom::parse_surface_texture_ids(&st).into_iter().flatten() {
            if portal.borrow_mut().find(t).is_none() {
                continue;
            }
            let Some(buf) = portal.borrow_mut().read(t) else {
                continue;
            };
            // Paletted (format 101): each 2-byte pixel is a u16-LE index into
            // the palette named by the record's trailing id. Resolve via that
            // palette — the raw bytes are indices, not RGB, so averaging them
            // directly yields the wrong (all-blue) tint.
            if let Some(pal_id) = acegeom::parse_texture_palette_id(&buf) {
                if let Some(pal) = portal.borrow_mut().read(pal_id) {
                    if buf.len() >= 24 {
                        let len =
                            i32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]) as usize;
                        if buf.len() >= 24 + len {
                            if let Some(avg) =
                                acegeom::average_paletted(&buf[24..24 + len], &pal, 4096)
                            {
                                return Some(avg);
                            }
                        }
                    }
                }
            }
            if let Some(avg) = acegeom::parse_texture_average(&buf, 512, 4096) {
                return Some(avg);
            }
        }
        None
    }

    /// A surface (0x08…) data id resolved to a color (cached).
    fn surface_color(&mut self, id: u32) -> Option<u32> {
        if let Some(&hit) = self.surf_cache.get(&id) {
            return hit;
        }
        let out = self.surface_color_fresh(id);
        self.surf_cache.insert(id, out);
        out
    }

    /// Build (or reuse) the terrain triangle soup for a landblock.
    /// 12 floats per directed triangle: `[ax,ay,az, bx,by,bz, cx,cy,cz, r,g,b]`.
    pub fn load_landblock(&mut self, lb: u32) -> bool {
        if self.blocks.contains_key(&lb) {
            if let Some(i) = self.order.iter().position(|&b| b == lb) {
                self.order.remove(i);
            }
            self.order.push(lb);
            return true;
        }
        let (Some(cell), Some(htable)) = (self.cell.as_ref(), self.height_table) else {
            return false;
        };
        let Some(rec) = cell.borrow_mut().read(lb * 0x10000 + 0xffff) else {
            return false;
        };
        let Some(data) = acegeom::parse_cell_landblock(&rec) else {
            return false;
        };

        // Terrain: a 9x9 vertex grid, 8x8 quads, 2 tris each.
        let mut z = [0f32; 81];
        for i in 0..81 {
            z[i] = htable[data.height[i] as usize];
        }
        let mut soup = Vec::with_capacity(8 * 8 * 2 * 12);
        for i in 0..8 {
            for j in 0..8 {
                let (x0, x1) = (i as f32 * 24.0, (i + 1) as f32 * 24.0);
                let (y0, y1) = (j as f32 * 24.0, (j + 1) as f32 * 24.0);
                let z00 = z[i * 9 + j];
                let z10 = z[(i + 1) * 9 + j];
                let z01 = z[i * 9 + j + 1];
                let z11 = z[(i + 1) * 9 + j + 1];
                let t00 = data.terrain[i * 9 + j];
                let t10 = data.terrain[(i + 1) * 9 + j];
                let t01 = data.terrain[i * 9 + j + 1];
                let t11 = data.terrain[(i + 1) * 9 + j + 1];
                let road = acegeom::terrain_road(t00)
                    | acegeom::terrain_road(t10)
                    | acegeom::terrain_road(t01)
                    | acegeom::terrain_road(t11);
                let [r, g, b] = terrain_color(acegeom::terrain_type(t00), road);

                // (00,10,11) and (00,11,01)
                soup.extend_from_slice(&[
                    x0, y0, z00, x1, y0, z10, x1, y1, z11, r, g, b,
                ]);
                soup.extend_from_slice(&[x0, y0, z00, x1, y1, z11, x0, y1, z01, r, g, b]);
            }
        }

        self.blocks.insert(lb, soup);
        self.order.push(lb);
        if self.order.len() > MAX_BLOCKS {
            let evict = self.order.remove(0);
            if evict != lb {
                self.blocks.remove(&evict);
            }
        }
        true
    }

    /// The terrain soup for a loaded landblock, or `None`.
    #[must_use]
    pub fn block_soup(&self, lb: u32) -> Option<&[f32]> {
        self.blocks.get(&lb).map(Vec::as_slice)
    }

    /// Terrain height (region height table) at block-local `(x, y)`, or
    /// `None` when the dats are unavailable or the landblock has no terrain
    /// record. Bilinear over the 9x9 vertex grid — the same surface the
    /// renderer rasterizes. x/y are wrapped into the block frame.
    #[must_use]
    pub fn height_at(&mut self, lb: u32, x: f32, y: f32) -> Option<f32> {
        if !self.height_grids.contains_key(&lb) {
            let mut grid = None;
            if let (Some(cell), Some(htable)) = (self.cell.as_ref(), self.height_table) {
                if let Some(rec) = cell.borrow_mut().read(lb * 0x10000 + 0xffff) {
                    if let Some(data) = acegeom::parse_cell_landblock(&rec) {
                        grid = Some(
                            (0..81).map(|i| htable[data.height[i] as usize]).collect(),
                        );
                    }
                }
            }
            self.height_grids.insert(lb, grid);
        }
        let grid = match self.height_grids.get(&lb) {
            Some(Some(g)) => g,
            _ => return None,
        };
        let gx = x.rem_euclid(BLOCK_LEN);
        let gy = y.rem_euclid(BLOCK_LEN);
        let fx = gx / 24.0;
        let fy = gy / 24.0;
        let i0 = fx as usize;
        let j0 = fy as usize;
        let tx = fx - i0 as f32;
        let ty = fy - j0 as f32;
        let z00 = grid[i0 * 9 + j0];
        let z10 = grid[(i0 + 1) * 9 + j0];
        let z01 = grid[i0 * 9 + j0 + 1];
        let z11 = grid[(i0 + 1) * 9 + j0 + 1];
        let mut z = (z00 * (1.0 - tx) + z10 * tx) * (1.0 - ty) + (z01 * (1.0 - tx) + z11 * tx) * ty;
        // A stair is a stack of flat step boxes (env-cell statics). The
        // terrain grid has no stair, so the knee ray sits under the step and
        // the stair blocks like a wall. When the foot is over an upward-
        // facing, near-flat step top that rises a stair's height (0.2..4.0u)
        // above the terrain, lift to that top so the character climbs.
        // [`stair_top_at`] only considers stair model ids, so tall furniture
        // (a 3u candlestick) in the same band never snaps the foot.
        if let Some(top) = self.stair_top_at(lb, gx, gy, z) {
            if (0.2..4.0).contains(&(top - z)) {
                z = top;
            }
        }
        // A shop's upper floor is a near-flat upward-facing plane in the
        // building GfxObj mesh (the terrain grid only records the ground).
        // Lift the foot onto it so the character stands on the second floor
        // instead of falling through to the ground.
        if let Some(top) = self.floor_top_at(lb, gx, gy, z) {
            if (0.2..8.0).contains(&(top - z)) {
                z = top;
            }
        }
        Some(z)
    }

    /// The z of the highest upward-facing, near-flat top of a *stair* prop
    /// that contains block-local `(gx, gy)` and rises above `terrain_z` — a
    /// step the character can stand on. `None` when the foot is over no such
    /// step. Only the stair model ids (the `0x0100081a` step box and the
    /// `0x02` multi-part staircase setups) are considered, so candlesticks,
    /// barrels, and other furniture never snap the foot even when their tops
    /// sit in the same height band as a step. A tri counts as a step top when
    /// it is near-flat (own z-range < 0.3) and faces up; the stair's vertical
    /// side walls never do.
    fn stair_top_at(&mut self, lb: u32, gx: f32, gy: f32, terrain_z: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for (id, _region, soup) in self.env_props(lb) {
            let tag = id & 0xff000000;
            let is_stair = id == 0x0100081a || tag == 0x02000000;
            if !is_stair {
                continue;
            }
            for o in (0..soup.len()).step_by(12) {
                let ax = soup[o];
                let ay = soup[o + 1];
                let az = soup[o + 2];
                let bx = soup[o + 3];
                let by = soup[o + 4];
                let bz = soup[o + 5];
                let cx = soup[o + 6];
                let cy = soup[o + 7];
                let cz = soup[o + 8];
                let zmin = az.min(bz).min(cz);
                let zmax = az.max(bz).max(cz);
                if zmax - zmin >= 0.3 || zmax <= terrain_z + 0.1 {
                    continue;
                }
                let xmin = ax.min(bx).min(cx);
                let xmax = ax.max(bx).max(cx);
                let ymin = ay.min(by).min(cy);
                let ymax = ay.max(by).max(cy);
                // Test the tri and its copies across the block seam, like the
                // periodic `ray_blocked`.
                for dx in [0.0f32, BLOCK_LEN, -BLOCK_LEN] {
                    for dy in [0.0f32, BLOCK_LEN, -BLOCK_LEN] {
                        if xmin + dx <= gx
                            && gx <= xmax + dx
                            && ymin + dy <= gy
                            && gy <= ymax + dy
                        {
                            best = Some(best.map_or(zmax, |b: f32| b.max(zmax)));
                        }
                    }
                }
            }
        }
        best
    }

    /// The z of the highest upward-facing, near-flat *floor* plane of the
    /// landblock's building GfxObj meshes that contains block-local
    /// `(gx, gy)` and rises above `terrain_z` — an upper floor the
    /// character can stand on. `None` when the foot is over no such floor.
    /// The terrain grid only records the ground, so without this a shop's
    /// second floor (a building-soup plane) drops the foot to the ground and
    /// the character falls through it. The planes are precomputed once per
    /// landblock ([`Scene::building_floors`]): each building-soup triangle
    /// that is near-flat (own z-range < 0.3) and faces up (positive z
    /// normal) is a floor candidate; vertical walls and ceilings never are.
    fn floor_top_at(&mut self, lb: u32, gx: f32, gy: f32, terrain_z: f32) -> Option<f32> {
        if !self.building_floors.contains_key(&lb) {
            let mut floors: Vec<[f32; 5]> = Vec::new();
            for (_id, _region, soup) in self.building_soup(lb) {
                for o in (0..soup.len()).step_by(12) {
                    let ax = soup[o];
                    let ay = soup[o + 1];
                    let az = soup[o + 2];
                    let bx = soup[o + 3];
                    let by = soup[o + 4];
                    let bz = soup[o + 5];
                    let cx = soup[o + 6];
                    let cy = soup[o + 7];
                    let cz = soup[o + 8];
                    let zmin = az.min(bz).min(cz);
                    let zmax = az.max(bz).max(cz);
                    if zmax - zmin >= 0.3 {
                        continue;
                    }
                    // Normal z-component of (B-A)x(C-A); >0 faces up.
                    let nz = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
                    if nz <= 0.0 {
                        continue;
                    }
                    floors.push([
                        zmax,
                        ax.min(bx).min(cx),
                        ay.min(by).min(cy),
                        ax.max(bx).max(cx),
                        ay.max(by).max(cy),
                    ]);
                }
            }
            self.building_floors.insert(lb, floors);
        }
        let mut best: Option<f32> = None;
        if let Some(floors) = self.building_floors.get(&lb) {
            for f in floors {
                let (ztop, xmin, ymin, xmax, ymax) = (f[0], f[1], f[2], f[3], f[4]);
                if ztop <= terrain_z + 0.1 {
                    continue;
                }
                // Test the floor and its copies across the block seam, like
                // the periodic `ray_blocked`.
                for dx in [0.0f32, BLOCK_LEN, -BLOCK_LEN] {
                    for dy in [0.0f32, BLOCK_LEN, -BLOCK_LEN] {
                        if xmin + dx <= gx
                            && gx <= xmax + dx
                            && ymin + dy <= gy
                            && gy <= ymax + dy
                        {
                            best = Some(best.map_or(ztop, |b| b.max(ztop)));
                        }
                    }
                }
            }
        }
        best
    }




    /// The static solid geometry of a landblock: every stab/building GfxObj
    /// (0x01 model) with a solid physics leaf — world-space block-frame
    /// triangles + bbox. The same objects the renderer draws, so collision
    /// matches the picture. Cached per landblock.
    #[must_use]
    pub fn solids_for_block(&mut self, lb: u32) -> Vec<Solid> {
        if let Some(v) = self.solid_objs.get(&lb) {
            return v.clone();
        }
        let mut out = Vec::new();
        if let (Some(cell), Some(portal)) = (self.cell.as_ref(), self.portal.as_ref()) {
            if let Some(rec) = cell.borrow_mut().read(lb * 0x10000 + 0xfffe) {
                if let Some(lbi) = acegeom::parse_landblock_info(&rec) {
                    // (model id, placement frame, is_a_building). Buildings
                    // carry the same solid physics as props but are drawn as a
                    // detailed visual mesh (the building soup), so their flat
                    // static box must be skipped — tag them with `building`.
                    let mut ids: Vec<(u32, Frame, bool)> =
                        lbi.stabs.iter().map(|s| (s.id, s.frame, false)).collect();
                    for (id, frame) in &lbi.buildings {
                        ids.push((*id, *frame, true));
                    }
                    for (id, frame, building) in ids {
                        if id & 0xff000000 != 0x01000000 {
                            continue;
                        }
                        let Some(buf) = portal.borrow_mut().read(id) else {
                            continue;
                        };
                        if let Some(phys) = acegeom::parse_gfx_obj_physics(&buf, Some(&frame)) {
                            if phys.solid {
                                out.push(Solid {
                                    tris: phys.tris,
                                    bbox: phys.bbox,
                                    building,
                                });
                            }
                        }
                    }
                }
            }
        }
        self.solid_objs.insert(lb, out.clone());
        out
    }

    /// The placed GfxObj (0x01) prop visual meshes for landblock `lb`, one
    /// entry per prop: `(model_id, drawn-vertex region AABB, soup)`. The soup
    /// is 12 floats/tri (3 world-space verts + rgb) from the GfxObj render
    /// mesh — the furniture/barrels/lamps the renderer draws. Pass-through
    /// props (no physics leaf) still get their visual mesh; a prop with a
    /// solid physics leaf also blocks movement via [`Scene::solids_for_block`].
    /// Only the LBI `stabs` list (placed props) is used, never `buildings`
    /// (whose visual mesh z-fights the env-cell shell). Cached per landblock;
    /// empty when the dats are unavailable or the block has no props.
    #[must_use]
    pub fn prop_soup(&mut self, lb: u32) -> Vec<(u32, Aabb, Vec<f32>)> {
        if let Some(v) = self.prop_soups.get(&lb) {
            return v.clone();
        }
        // Collect the raw GfxObj buffers first; the dat borrows must be
        // released before the surface-color resolution below (which
        // re-borrows the portal).
        let mut raw: Vec<(u32, Frame, Vec<u8>)> = Vec::new();
        if let (Some(cell), Some(portal)) = (self.cell.as_ref(), self.portal.as_ref()) {
            if let Some(rec) = cell.borrow_mut().read(lb * 0x10000 + 0xfffe) {
                if let Some(lbi) = acegeom::parse_landblock_info(&rec) {
                    for s in &lbi.stabs {
                        if s.id & 0xff000000 != 0x01000000 {
                            continue;
                        }
                        let Some(buf) = portal.borrow_mut().read(s.id) else {
                            continue;
                        };
                        raw.push((s.id, s.frame, buf));
                    }
                }
            }
        }
        let mut soups: Vec<(u32, Aabb, Vec<f32>)> = Vec::new();
        // Parse each mesh (world-space, resolved colors) and build the soup.
        for (id, frame, buf) in &raw {
            let Some(mesh) =
                acegeom::parse_gfx_obj(buf, |cid| self.surface_color(cid), Some(frame))
            else {
                continue;
            };
            let n = mesh.pos.len() / 3;
            if n == 0 || mesh.tris.is_empty() {
                continue;
            }
            let mut soup: Vec<f32> = Vec::with_capacity(mesh.tri_mat.len() * 12);
            for (j, &tm) in mesh.tri_mat.iter().enumerate() {
                let (ia, ib, ic) = match (
                    mesh.tris.get(3 * j).copied(),
                    mesh.tris.get(3 * j + 1).copied(),
                    mesh.tris.get(3 * j + 2).copied(),
                ) {
                    (Some(a), Some(b), Some(c)) => (a, b, c),
                    _ => continue,
                };
                if ia as usize >= n || ib as usize >= n || ic as usize >= n {
                    continue;
                }
                let color = mesh
                    .mats
                    .get(tm as usize)
                    .copied()
                    .unwrap_or(0xff808080);
                let (cr, cg, cb) = (
                    ((color >> 16) & 0xff) as f32,
                    ((color >> 8) & 0xff) as f32,
                    (color & 0xff) as f32,
                );
                let a = 3 * ia as usize;
                let b = 3 * ib as usize;
                let c = 3 * ic as usize;
                soup.extend_from_slice(&[
                    mesh.pos[a], mesh.pos[a + 1], mesh.pos[a + 2],
                    mesh.pos[b], mesh.pos[b + 1], mesh.pos[b + 2],
                    mesh.pos[c], mesh.pos[c + 1], mesh.pos[c + 2],
                    cr, cg, cb,
                ]);
            }
            if soup.is_empty() {
                continue;
            }
            let region = {
                let mut bb = [
                    f32::INFINITY, f32::INFINITY, f32::INFINITY,
                    f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY,
                ];
                for v in (0..soup.len()).step_by(12) {
                    for k in 0..9 {
                        let val = soup[v + k];
                        bb[k % 3] = bb[k % 3].min(val);
                        bb[3 + (k % 3)] = bb[3 + (k % 3)].max(val);
                    }
                }
                bb
            };
            soups.push((*id, region, soup));
        }
        self.prop_soups.insert(lb, soups.clone());
        soups
    }

    /// The building GfxObj visual meshes for landblock `lb` (the LBI
    /// `buildings` list): each parsed world-space with resolved material
    /// colors into the 12-float/tri soup, with its world-space region AABB.
    /// These are the detailed multi-material models a town's shops are made
    /// of. Mirrors [`Scene::prop_soup`] but reads `buildings` instead of
    /// `stabs`. Cached per landblock.
    #[must_use]
    pub fn building_soup(&mut self, lb: u32) -> Vec<(u32, Aabb, Vec<f32>)> {
        if let Some(v) = self.building_soups.get(&lb) {
            return v.clone();
        }
        // Collect the raw GfxObj buffers first; the dat borrows must be
        // released before the surface-color resolution below (which
        // re-borrows the portal).
        let mut raw: Vec<(u32, Frame, Vec<u8>)> = Vec::new();
        if let (Some(cell), Some(portal)) = (self.cell.as_ref(), self.portal.as_ref()) {
            if let Some(rec) = cell.borrow_mut().read(lb * 0x10000 + 0xfffe) {
                if let Some(lbi) = acegeom::parse_landblock_info(&rec) {
                    for (id, frame) in &lbi.buildings {
                        let Some(buf) = portal.borrow_mut().read(*id) else {
                            continue;
                        };
                        raw.push((*id, *frame, buf));
                    }
                }
            }
        }
        let mut soups: Vec<(u32, Aabb, Vec<f32>)> = Vec::new();
        // Parse each mesh (world-space, resolved colors) and build the soup.
        for (id, frame, buf) in &raw {
            let Some(mesh) =
                acegeom::parse_gfx_obj(buf, |cid| self.surface_color(cid), Some(frame))
            else {
                continue;
            };
            let n = mesh.pos.len() / 3;
            if n == 0 || mesh.tris.is_empty() {
                continue;
            }
            let mut soup: Vec<f32> = Vec::with_capacity(mesh.tri_mat.len() * 12);
            for (j, &tm) in mesh.tri_mat.iter().enumerate() {
                let (ia, ib, ic) = match (
                    mesh.tris.get(3 * j).copied(),
                    mesh.tris.get(3 * j + 1).copied(),
                    mesh.tris.get(3 * j + 2).copied(),
                ) {
                    (Some(a), Some(b), Some(c)) => (a, b, c),
                    _ => continue,
                };
                if ia as usize >= n || ib as usize >= n || ic as usize >= n {
                    continue;
                }
                let color = mesh
                    .mats
                    .get(tm as usize)
                    .copied()
                    .unwrap_or(0xff808080);
                let (cr, cg, cb) = (
                    ((color >> 16) & 0xff) as f32,
                    ((color >> 8) & 0xff) as f32,
                    (color & 0xff) as f32,
                );
                let a = 3 * ia as usize;
                let b = 3 * ib as usize;
                let c = 3 * ic as usize;
                soup.extend_from_slice(&[
                    mesh.pos[a], mesh.pos[a + 1], mesh.pos[a + 2],
                    mesh.pos[b], mesh.pos[b + 1], mesh.pos[b + 2],
                    mesh.pos[c], mesh.pos[c + 1], mesh.pos[c + 2],
                    cr, cg, cb,
                ]);
            }
            if soup.is_empty() {
                continue;
            }
            let region = {
                let mut bb = [
                    f32::INFINITY, f32::INFINITY, f32::INFINITY,
                    f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY,
                ];
                for v in (0..soup.len()).step_by(12) {
                    for k in 0..9 {
                        let val = soup[v + k];
                        bb[k % 3] = bb[k % 3].min(val);
                        bb[3 + (k % 3)] = bb[3 + (k % 3)].max(val);
                    }
                }
                bb
            };
            soups.push((*id, region, soup));
        }
        self.building_soups.insert(lb, soups.clone());
        soups
    }

    /// The GfxObj (0x01) statics placed inside the landblock's env (indoor)
    /// cells: the shop's interior props — the staircase, candlesticks,
    /// paintings, barrels. Each env cell carries a `statics` list (its
    /// `0x2` HasStaticObjs payload); the GfxObj entries are rendered here,
    /// 12 floats/tri (3 world-space verts + rgb) with their region AABB. The
    /// static frames are block-local (world) space — not the cell's local
    /// frame — so they are drawn directly (like the LBI `stabs`). Mirrors
    /// [`Scene::prop_soup`] but reads the env cells instead of the LBI.
    /// SetupModel (0x02) entries are the room's characters, not props, and
    /// are skipped. Cached per landblock; empty when the dats are
    /// unavailable or the block's env cells carry no GfxObj statics.
    #[must_use]
    pub fn env_props(&mut self, lb: u32) -> Vec<(u32, Aabb, Vec<f32>)> {
        if let Some(v) = self.env_prop_soups.get(&lb) {
            return v.clone();
        }
        // Collect the raw GfxObj buffers first; the dat borrows must be
        // released before the surface-color resolution below (which
        // re-borrows the portal).
        let mut raw: Vec<(u32, Frame, Vec<u8>)> = Vec::new();
        if let (Some(cell), Some(portal)) = (self.cell.as_ref(), self.portal.as_ref()) {
            let lo = lb * 0x10000 + 0x100;
            let hi = lb * 0x10000 + 0xFFFD;
            let mut ids: Vec<u32> = Vec::new();
            {
                let dir = cell.borrow();
                let keys = dir.ids();
                let start = keys.partition_point(|&k| k < lo);
                for &id in &keys[start..] {
                    if id > hi {
                        break;
                    }
                    ids.push(id);
                }
            }
            for id in ids {
                let Some(rec) = cell.borrow_mut().read(id) else {
                    continue;
                };
                let Some(ce) = acegeom::parse_env_cell(&rec) else {
                    continue;
                };
                for s in &ce.statics {
                    let tag = s.id & 0xff000000;
                    if tag == 0x01000000 {
                        // GfxObj static (fences, etc.): the model itself.
                        let Some(buf) = portal.borrow_mut().read(s.id) else {
                            continue;
                        };
                        raw.push((s.id, s.frame, buf));
                    } else if tag == 0x02000000 {
                        // SetupModel static. A shop's furniture (candlestick,
                        // paintings, barrels) is a 1-part setup — its single
                        // part is a GfxObj placed at the static's frame. A
                        // multi-part setup is a static staircase when it
                        // repeats the step box (0x0100081a); otherwise it is a
                        // character (live NPCs come from the wire, so an
                        // env-cell multi-part setup is static) and is skipped.
                        let Some(setup) = portal.borrow_mut().read(s.id) else {
                            continue;
                        };
                        let Some(parts) = acegeom::parse_setup_parts(&setup) else {
                            continue;
                        };
                        let is_staircase =
                            parts.iter().filter(|&&p| p == 0x0100081a).count() >= 3;
                        if !(parts.len() == 1 || is_staircase) {
                            continue;
                        }
                        // Resolve the pose: a 1-part setup sits at the static
                        // frame with an identity pose; a staircase takes its
                        // row-0 part frames from its DefaultAnimation, each
                        // composed with the static frame. has_anim is false
                        // when no DefaultAnimation exists (the common case for
                        // a static staircase) - then the parts take their pose
                        // from the setup's embedded per-part frames (the
                        // stringers, the rising treads, the top landing), or
                        // identity when the record has none.
                        let (pos_frame, part_frames, has_anim) = if is_staircase {
                            match acegeom::parse_setup_tail(&setup).and_then(|t| {
                                if t[0] == 0 {
                                    None
                                } else {
                                    portal
                                        .borrow_mut()
                                        .read(t[0])
                                        .as_deref()
                                        .and_then(acegeom::parse_animation)
                                }
                            }) {
                                Some(a) => {
                                    let pos = match &a.pos_frames {
                                        Some(pf) if pf.len() >= 7 => Frame::new(
                                            pf[0], pf[1], pf[2], pf[3], pf[4], pf[5], pf[6],
                                        ),
                                        _ => IDENTITY,
                                    };
                                    let pfs: Vec<Frame> = (0..a.num_parts)
                                        .map(|pi| {
                                            let o = pi * 7;
                                            if o + 7 <= a.part_frames.len() {
                                                Frame::new(
                                                    a.part_frames[o],
                                                    a.part_frames[o + 1],
                                                    a.part_frames[o + 2],
                                                    a.part_frames[o + 3],
                                                    a.part_frames[o + 4],
                                                    a.part_frames[o + 5],
                                                    a.part_frames[o + 6],
                                                )
                                            } else {
                                                IDENTITY
                                            }
                                        })
                                        .collect();
                                    (pos, pfs, true)
                                }
                                None => {
                                    // No DefaultAnimation: a static staircase
                                    // embeds its per-part model-space pose
                                    // (the stringers, the treads rising per
                                    // step, the top landing). Use it when
                                    // present; otherwise fall back to identity.
                                    let pfs =
                                        acegeom::parse_setup_part_frames(&setup)
                                            .unwrap_or_else(|| {
                                                (0..parts.len()).map(|_| IDENTITY).collect()
                                            });
                                    (IDENTITY, pfs, false)
                                }
                            }
                        } else {
                            (IDENTITY, vec![IDENTITY], false)
                        };
                        let n_parts = if is_staircase { parts.len() } else { 1 };
                        // A static staircase's per-part pose (parsed above, or
                        // identity) places the parts in the setup's model space,
                        // which is not always grounded at z=0. Lift the frame so
                        // the base (min model-space z over all parts, each
                        // rotated by its own frame) sits at s.frame.oz, matching
                        // the 1-part step-box convention where a flat box at
                        // z~0 sits exactly at s.frame.oz.
                        let mut frame = s.frame;
                        if is_staircase && !has_anim {
                            let mut min_z = f32::INFINITY;
                            for (pi, &part_id) in parts.iter().enumerate().take(n_parts) {
                                let pf = part_frames.get(pi).copied().unwrap_or(IDENTITY);
                                if let Some(buf) = portal.borrow_mut().read(part_id) {
                                    if let Some(mesh) =
                                        acegeom::parse_gfx_obj(&buf, |_| None, None)
                                    {
                                        let q = [pf.qw, pf.qx, pf.qy, pf.qz];
                                        for i in (0..mesh.pos.len()).step_by(3) {
                                            let rz = acegeom::q_rotate(
                                                q,
                                                mesh.pos[i],
                                                mesh.pos[i + 1],
                                                mesh.pos[i + 2],
                                            )[2];
                                            min_z = min_z.min(rz + pf.oz);
                                        }
                                    }
                                }
                            }
                            if min_z.is_finite() && min_z < 0.0 {
                                frame.oz -= min_z;
                            }
                        }
                        for (pi, &part_id) in parts.iter().enumerate().take(n_parts) {
                            let Some(buf) = portal.borrow_mut().read(part_id) else {
                                continue;
                            };
                            let pf = part_frames.get(pi).copied().unwrap_or(IDENTITY);
                            let (q, t) = acegeom::compose3(&frame, &pos_frame, &pf);
                            let world =
                                Frame::new(t[0], t[1], t[2], q[0], q[1], q[2], q[3]);
                            raw.push((s.id, world, buf));
                        }
                    }
                }
            }
        }
        let mut soups: Vec<(u32, Aabb, Vec<f32>)> = Vec::new();
        // Parse each mesh (world-space, resolved colors) and build the soup.
        for (id, frame, buf) in &raw {
            let Some(mesh) =
                acegeom::parse_gfx_obj(buf, |cid| self.surface_color(cid), Some(frame))
            else {
                continue;
            };
            let n = mesh.pos.len() / 3;
            if n == 0 || mesh.tris.is_empty() {
                continue;
            }
            let mut soup: Vec<f32> = Vec::with_capacity(mesh.tri_mat.len() * 12);
            for (j, &tm) in mesh.tri_mat.iter().enumerate() {
                let (ia, ib, ic) = match (
                    mesh.tris.get(3 * j).copied(),
                    mesh.tris.get(3 * j + 1).copied(),
                    mesh.tris.get(3 * j + 2).copied(),
                ) {
                    (Some(a), Some(b), Some(c)) => (a, b, c),
                    _ => continue,
                };
                if ia as usize >= n || ib as usize >= n || ic as usize >= n {
                    continue;
                }
                let color = mesh
                    .mats
                    .get(tm as usize)
                    .copied()
                    .unwrap_or(0xff808080);
                let (cr, cg, cb) = (
                    ((color >> 16) & 0xff) as f32,
                    ((color >> 8) & 0xff) as f32,
                    (color & 0xff) as f32,
                );
                let a = 3 * ia as usize;
                let b = 3 * ib as usize;
                let c = 3 * ic as usize;
                soup.extend_from_slice(&[
                    mesh.pos[a], mesh.pos[a + 1], mesh.pos[a + 2],
                    mesh.pos[b], mesh.pos[b + 1], mesh.pos[b + 2],
                    mesh.pos[c], mesh.pos[c + 1], mesh.pos[c + 2],
                    cr, cg, cb,
                ]);
            }
            if soup.is_empty() {
                continue;
            }
            let region = {
                let mut bb = [
                    f32::INFINITY, f32::INFINITY, f32::INFINITY,
                    f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY,
                ];
                for v in (0..soup.len()).step_by(12) {
                    for k in 0..9 {
                        let val = soup[v + k];
                        bb[k % 3] = bb[k % 3].min(val);
                        bb[3 + (k % 3)] = bb[3 + (k % 3)].max(val);
                    }
                }
                bb
            };
            soups.push((*id, region, soup));
        }
        self.env_prop_soups.insert(lb, soups.clone());
        soups
    }


    /// The env (indoor/structure) cells present in landblock `lb`, each with
    /// its block-local region AABB (the environment's drawing geometry
    /// transformed by the EnvCell position frame). Cached per landblock;
    /// empty when the dats are unavailable or the block has no env cells.
    /// The dat directory is sorted by id, so the block's env-cell range
    /// (0x100..=0xFFFD) is a slice, not a scan.
    #[must_use]
    pub fn env_cells(&mut self, lb: u32) -> Vec<(u32, Aabb)> {
        if let Some(v) = self.env_regions.get(&lb) {
            return v.clone();
        }
        let mut out: Vec<(u32, Aabb)> = Vec::new();
        if let (Some(cell), Some(portal)) = (self.cell.as_ref(), self.portal.as_ref()) {
            let lo = lb * 0x10000 + 0x100;
            let hi = lb * 0x10000 + 0xFFFD;
            let mut ids: Vec<u32> = Vec::new();
            {
                let dir = cell.borrow();
                let keys = dir.ids();
                let start = keys.partition_point(|&k| k < lo);
                for &id in &keys[start..] {
                    if id > hi {
                        break;
                    }
                    ids.push(id);
                }
            }
            for id in ids {
                let Some(rec) = cell.borrow_mut().read(id) else {
                    continue;
                };
                let Some(ce) = acegeom::parse_env_cell(&rec) else {
                    continue;
                };
                let Some(env) = portal.borrow_mut().read(ce.environment_id) else {
                    continue;
                };
                let Some(data) = acegeom::parse_environment(&env, ce.cell_structure as u32)
                else {
                    continue;
                };
                let (mut mn, mut mx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
                for i in (0..data.pos.len()).step_by(3) {
                    let v =
                        acegeom::apply_frame(&ce.position, data.pos[i], data.pos[i + 1], data.pos[i + 2]);
                    for a in 0..3 {
                        mn[a] = mn[a].min(v[a]);
                        mx[a] = mx[a].max(v[a]);
                    }
                }
                if !mn.iter().all(|v| v.is_finite()) {
                    continue;
                }
                out.push((id & 0xFFFF, [mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]]));
            }
        }
        self.env_regions.insert(lb, out.clone());
        out
    }

    /// The env (indoor/structure) cell triangle soups for landblock `lb`,
    /// one entry per cell: `(cell_id, region, soup)`. The soup is 12
    /// floats/tri (3 verts + rgb) — the walls/ceiling/floor the renderer
    /// draws. Each CellStruct's drawing polygons are fanned, transformed by
    /// the EnvCell position frame, and colored from the polygon's positive
    /// surface. The region is the cell's drawn-vertex bbox (block-local); the
    /// renderer skips the cell whose region contains the camera so a building
    /// shell doesn't wall in its interior. Cached per landblock; empty when
    /// the dats are unavailable or the block has no env cells.
    #[must_use]
    pub fn env_soup(&mut self, lb: u32) -> Vec<(u32, Aabb, Vec<f32>)> {
        if let Some(v) = self.env_soups.get(&lb) {
            return v.clone();
        }
        // Collect the env-cell geometry first; the dat borrows must be
        // released before the surface-color resolution below (which re-borrows
        // the portal).
        struct EnvGeom {
            cell_id: u32,
            pos: Vec<f32>,
            keys: Vec<u16>,
            polys: Vec<acegeom::Poly>,
            surface_ids: Vec<u32>,
            position: Frame,
        }
        let mut envs: Vec<EnvGeom> = Vec::new();
        if let (Some(cell), Some(portal)) = (self.cell.as_ref(), self.portal.as_ref()) {
            let lo = lb * 0x10000 + 0x100;
            let hi = lb * 0x10000 + 0xFFFD;
            let mut ids: Vec<u32> = Vec::new();
            {
                let dir = cell.borrow();
                let keys = dir.ids();
                let start = keys.partition_point(|&k| k < lo);
                for &id in &keys[start..] {
                    if id > hi {
                        break;
                    }
                    ids.push(id);
                }
            }
            for id in ids {
                let Some(rec) = cell.borrow_mut().read(id) else {
                    continue;
                };
                let Some(ce) = acegeom::parse_env_cell(&rec) else {
                    continue;
                };
                let Some(env) = portal.borrow_mut().read(ce.environment_id) else {
                    continue;
                };
                let Some(data) =
                    acegeom::parse_environment(&env, ce.cell_structure as u32)
                else {
                    continue;
                };
                envs.push(EnvGeom {
                    cell_id: id,
                    pos: data.pos,
                    keys: data.keys,
                    polys: data.polys,
                    surface_ids: ce.surface_ids,
                    position: ce.position,
                });
            }
        }
        let mut soups: Vec<(u32, Aabb, Vec<f32>)> = Vec::new();
        for eg in &envs {
            let key_map = acegeom::key_to_slot_map(&eg.keys);
            let mut soup: Vec<f32> = Vec::new();
            for poly in &eg.polys {
                let surf_id = eg
                    .surface_ids
                    .get(poly.pos_surface as usize)
                    .copied()
                    .unwrap_or(0);
                // Skip AC's invisible surfaces, keyed on surface structure:
                //  - glass (translucency >= 0.5): `surface_color_fresh`
                //    returns `None` for it, and the geometry builders skip a
                //    polygon whose front surface resolves `None`, so window /
                //    doorway orifices stay see-through instead of painting a
                //    flat grey slab (the env-cell's orifice surface carries a
                //    0xff solid color, so the no-color/no-texture and 0xff
                //    rules below would otherwise miss it), and
                //  - a solid/texture-avg color carrying the 0xff top byte
                //    (case 1: solid 0xff filler; case 3: texture with a
                //    resolved average — in 0x8502 these floor/ceiling planes
                //    blanket the frame, so HEAD's blanket 0xff skip is
                //    required for the town test), and
                //  - a surface with neither a color nor a texture (the flat
                //    plane that otherwise covers a doorway orifice as a grey
                //    slab).
                // A texture-present surface whose average is unresolved (case 4)
                // is real geometry (town walls) and draws with the fallback.
                let surf = if surf_id != 0 {
                    self.portal.as_ref().and_then(|p| {
                        p.borrow_mut()
                            .read(surf_id)
                            .and_then(|b| acegeom::parse_surface(&b))
                    })
                } else {
                    None
                };
                let surf_color = self.surface_color(surf_id);
                if surf.is_some_and(|s| s.translucency >= 0.5) {
                    continue;
                }
                if surf.is_some_and(|s| s.color.is_none() && s.texture_id.is_none()) {
                    continue;
                }
                if surf_color.is_some_and(|c| (c >> 24) == 0xff) {
                    continue;
                }
                let color = surf_color.map(mat_color).unwrap_or([96.0, 96.0, 96.0]);
                let verts: Vec<[f32; 3]> = poly
                    .vertex_ids
                    .iter()
                    .filter_map(|&k| {
                        let slot = if let Some(m) = &key_map {
                            m[k as usize]
                        } else {
                            k as i32
                        };
                        if slot < 0 || slot as usize * 3 + 2 >= eg.pos.len() {
                            return None;
                        }
                        let i = slot as usize * 3;
                        Some(acegeom::apply_frame(
                            &eg.position,
                            eg.pos[i],
                            eg.pos[i + 1],
                            eg.pos[i + 2],
                        ))
                    })
                    .collect();
                for t in 1..verts.len().saturating_sub(1) {
                    let [a, b, c] = [verts[0], verts[t], verts[t + 1]];
                    soup.extend_from_slice(&[
                        a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2], color[0],
                        color[1], color[2],
                    ]);
                }
            }
            // The cell's drawn-vertex bbox (block-local), for the renderer's
            // "camera inside this building" test.
            let region = if soup.is_empty() {
                [0.0; 6]
            } else {
                let mut bb = [
                    f32::INFINITY, f32::INFINITY, f32::INFINITY,
                    f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY,
                ];
                for v in (0..soup.len()).step_by(12) {
                    for k in 0..9 {
                        let val = soup[v + k];
                        bb[k % 3] = bb[k % 3].min(val);
                        bb[3 + (k % 3)] = bb[3 + (k % 3)].max(val);
                    }
                }
                bb
            };
            soups.push((eg.cell_id, region, soup));
        }
        self.env_soups.insert(lb, soups.clone());
        soups
    }

    /// The env (indoor/structure) cell physics solids for landblock `lb`:
    /// the walls that block movement. A town's walls exist only here (its
    /// LBI has no stabs). Each env cell's physics polygons are fanned and
    /// transformed by the EnvCell position frame (unwrapped — the frame can
    /// sit outside the `[0, 192)` block frame). Cached per landblock; empty
    /// when the dats are unavailable or no env cell carries physics.
    #[must_use]
    pub fn env_statics(&mut self, lb: u32) -> Vec<Solid> {
        if let Some(v) = self.env_solid_objs.get(&lb) {
            return v.clone();
        }
        let mut out: Vec<Solid> = Vec::new();
        if let (Some(cell), Some(portal)) = (self.cell.as_ref(), self.portal.as_ref()) {
            let lo = lb * 0x10000 + 0x100;
            let hi = lb * 0x10000 + 0xFFFD;
            let mut ids: Vec<u32> = Vec::new();
            {
                let dir = cell.borrow();
                let keys = dir.ids();
                let start = keys.partition_point(|&k| k < lo);
                for &id in &keys[start..] {
                    if id > hi {
                        break;
                    }
                    ids.push(id);
                }
            }
            for id in ids {
                let Some(rec) = cell.borrow_mut().read(id) else {
                    continue;
                };
                let Some(ce) = acegeom::parse_env_cell(&rec) else {
                    continue;
                };
                let Some(env) = portal.borrow_mut().read(ce.environment_id) else {
                    continue;
                };
                let Some(data) =
                    acegeom::parse_environment(&env, ce.cell_structure as u32)
                else {
                    continue;
                };
                if data.phys_tris.is_empty() {
                    continue;
                }
                // The drawing polys carry the special surfaces (0xff top
                // byte) — AC's invisible portal/trigger planes that fill
                // doorways and windows. The physics below would otherwise
                // keep a solid slab across the doorway (the "grey barricade"),
                // so carve the physics out at every orifice.
                let key_map = acegeom::key_to_slot_map(&data.keys);
                let mut orifices: Vec<[f32; 6]> = Vec::new();
                for poly in &data.polys {
                    let surf_id = ce
                        .surface_ids
                        .get(poly.pos_surface as usize)
                        .copied()
                        .unwrap_or(0);
                    if surf_id == 0 {
                        continue;
                    }
                    let Some(sbuf) = portal.borrow_mut().read(surf_id) else {
                        continue;
                    };
                    let Some(surf) = acegeom::parse_surface(&sbuf) else {
                        continue;
                    };
                    if !surf.color.is_some_and(|c| (c >> 24) == 0xff) {
                        continue;
                    }
                    let mut mn = [f32::INFINITY; 3];
                    let mut mx = [f32::NEG_INFINITY; 3];
                    let mut any = false;
                    for &k in &poly.vertex_ids {
                        let slot = key_map.as_ref().map(|m| m[k as usize]).unwrap_or(k as i32);
                        if slot < 0 || slot as usize * 3 + 2 >= data.pos.len() {
                            continue;
                        }
                        let i = slot as usize * 3;
                        let v = acegeom::apply_frame(
                            &ce.position,
                            data.pos[i],
                            data.pos[i + 1],
                            data.pos[i + 2],
                        );
                        for a in 0..3 {
                            mn[a] = mn[a].min(v[a]);
                            mx[a] = mx[a].max(v[a]);
                        }
                        any = true;
                    }
                    if any {
                        // Expand the orifice sheet so the physics slab that
                        // straddles the doorway is fully removed.
                        const E: f32 = 1.5;
                        orifices.push([
                            mn[0] - E,
                            mn[1] - E,
                            mn[2] - E,
                            mx[0] + E,
                            mx[1] + E,
                            mx[2] + E,
                        ]);
                    }
                }
                let mut bbox = [
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                ];
                let mut tris: Vec<f32> = Vec::with_capacity(data.phys_tris.len());
                for o in (0..data.phys_tris.len()).step_by(9) {
                    let mut c = [0.0f32; 3];
                    let mut verts = [[0.0f32; 3]; 3];
                    for a in 0..3 {
                        let v = acegeom::apply_frame(
                            &ce.position,
                            data.phys_tris[o + a * 3],
                            data.phys_tris[o + a * 3 + 1],
                            data.phys_tris[o + a * 3 + 2],
                        );
                        verts[a] = v;
                        for a2 in 0..3 {
                            c[a2] += v[a2] / 3.0;
                        }
                    }
                    // Drop a physics tri whose centroid sits in a doorway
                    // orifice (the portal plane that fills the opening).
                    if orifices.iter().any(|r| {
                        c[0] >= r[0] && c[0] <= r[3]
                            && c[1] >= r[1] && c[1] <= r[4]
                            && c[2] >= r[2] && c[2] <= r[5]
                    }) {
                        continue;
                    }
                    for v in &verts {
                        for a2 in 0..3 {
                            tris.push(v[a2]);
                            bbox[a2] = bbox[a2].min(v[a2]);
                            bbox[a2 + 3] = bbox[a2 + 3].max(v[a2]);
                        }
                    }
                }
                if tris.is_empty() || !bbox.iter().all(|v| v.is_finite()) {
                    continue;
                }
                out.push(Solid { tris, bbox, building: false });
            }
        }
        // The env-cell statics (the shop's interior props — fences, stairs,
        // candlesticks, paintings, barrels) also block movement. Reuse the
        // rendered prop soup: each tri's first 9 floats are the world-space
        // verts (the last 3 are the colour, dropped here).
        for (_id, region, soup) in self.env_props(lb) {
            if soup.is_empty() {
                continue;
            }
            let mut tris = Vec::with_capacity(soup.len());
            for i in (0..soup.len()).step_by(12) {
                tris.extend_from_slice(&soup[i..i + 9]);
            }
            if !tris.is_empty() {
                out.push(Solid { tris, bbox: region, building: false });
            }
        }
        self.env_solid_objs.insert(lb, out.clone());
        out
    }

    /// The composed setup-model AABB at the model-space (identity) object
    /// frame: the render path's `obj ∘ posFrame ∘ partFrame` composition with
    /// `obj` the identity and `posFrame`/`partFrame` from the setup's default
    /// animation row 0. A closed door's collision slab (the session queries
    /// it per door; the door's own transform is applied by the caller).
    /// Parsed on demand (the `&self` contract cannot touch the per-model
    /// caches); called per door, never per frame.
    #[must_use]
    pub fn setup_aabb(&self, setup_id: u32) -> Option<Aabb> {
        let Some(buf) = self.portal_read(setup_id) else {
            return None;
        };
        let parts = acegeom::parse_setup_parts(&buf)?;
        if parts.is_empty() {
            return None;
        }
        let tail = acegeom::parse_setup_tail(&buf);
        let def_anim = tail.map(|t| t[0]).unwrap_or(0);
        let mut pos_frame = IDENTITY;
        let mut part_frames: Option<Vec<f32>> = None;
        let mut num_parts = 0usize;
        if def_anim != 0 {
            if let Some(abuf) = self.portal_read(def_anim) {
                if let Some(anim) = acegeom::parse_animation(&abuf) {
                    if let Some(pf) = &anim.pos_frames {
                        if anim.num_frames > 0 {
                            pos_frame = Frame::new(
                                pf[0], pf[1], pf[2], pf[3], pf[4], pf[5], pf[6],
                            );
                        }
                    }
                    part_frames = Some(anim.part_frames.clone());
                    num_parts = anim.num_parts;
                }
            }
        }
        let (mut mn, mut mx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        let mut any = false;
        for (pi, &model_id) in parts.iter().enumerate() {
            if model_id == NULL_PART {
                continue;
            }
            let Some(mbuf) = self.portal_read(model_id) else {
                continue;
            };
            let Some(mesh) = acegeom::parse_gfx_obj(&mbuf, |id| self.surface_color_fresh(id), None)
            else {
                continue;
            };
            if mesh.pos.is_empty() {
                continue;
            }
            let pf = match part_frames.as_ref() {
                Some(pf) if pi < num_parts => {
                    let o = pi * 7;
                    Frame::new(pf[o], pf[o + 1], pf[o + 2], pf[o + 3], pf[o + 4], pf[o + 5], pf[o + 6])
                }
                _ => IDENTITY,
            };
            let (q, t) = acegeom::compose3(&IDENTITY, &pos_frame, &pf);
            for i in (0..mesh.pos.len()).step_by(3) {
                let v = acegeom::rot_t(q, mesh.pos[i], mesh.pos[i + 1], mesh.pos[i + 2], t);
                mn[0] = mn[0].min(v[0]);
                mn[1] = mn[1].min(v[1]);
                mn[2] = mn[2].min(v[2]);
                mx[0] = mx[0].max(v[0]);
                mx[1] = mx[1].max(v[1]);
                mx[2] = mx[2].max(v[2]);
            }
            any = true;
        }
        if any {
            Some([mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]])
        } else {
            None
        }
    }

    // ------------------------------------------------------------------
    // per-model caches for the dynamic (animated) actor path
    // ------------------------------------------------------------------

    /// The base body part list of a SetupModel (0x02…), or `None` when the
    /// id is absent or unparsable (cached).
    pub(crate) fn get_setup_parts(&mut self, setup_id: u32) -> Option<&Vec<u32>> {
        if !self.setup_parts_cache.contains_key(&setup_id) {
            let parts = self.portal_read(setup_id).and_then(|b| acegeom::parse_setup_parts(&b));
            self.setup_parts_cache.insert(setup_id, parts);
        }
        match self.setup_parts_cache.get(&setup_id) {
            Some(Some(v)) => Some(v),
            _ => None,
        }
    }

    /// The 5 trailing SetupModel data ids `[defAnim, defScript, defMTable,
    /// defSTable, defPSTable]`, or `None`.
    pub(crate) fn get_setup_tail(&mut self, setup_id: u32) -> Option<[u32; 5]> {
        let buf = self.portal_read(setup_id)?;
        acegeom::parse_setup_tail(&buf)
    }

    /// A MotionTable (0x09…), or `None` (cached).
    pub(crate) fn get_motion_table(&mut self, mtable_id: u32) -> Option<&MotionTable> {
        if !self.mtable_cache.contains_key(&mtable_id) {
            let mtable = self.portal_read(mtable_id).and_then(|b| acegeom::parse_motion_table(&b));
            self.mtable_cache.insert(mtable_id, mtable);
        }
        match self.mtable_cache.get(&mtable_id) {
            Some(Some(v)) => Some(v),
            _ => None,
        }
    }

    /// An Animation (0x03…), or `None` (cached).
    pub(crate) fn get_animation(&mut self, anim_id: u32) -> Option<&Animation> {
        if !self.anim_cache.contains_key(&anim_id) {
            let anim = self.portal_read(anim_id).and_then(|b| acegeom::parse_animation(&b));
            self.anim_cache.insert(anim_id, anim);
        }
        match self.anim_cache.get(&anim_id) {
            Some(Some(v)) => Some(v),
            _ => None,
        }
    }

    /// A part GfxObj mesh (0x01…) in model space (cached, capped).
    pub(crate) fn get_part_mesh(&mut self, model_id: u32) -> Option<&Mesh> {
        if !self.part_mesh_cache.contains_key(&model_id) {
            if self.part_mesh_cache.len() >= MAX_PART_MESHES {
                self.part_mesh_cache.clear();
            }
            let mesh = self
                .portal_read(model_id)
                .and_then(|buf| acegeom::parse_gfx_obj(&buf, |id| self.surface_color(id), None));
            self.part_mesh_cache.insert(model_id, mesh);
        }
        match self.part_mesh_cache.get(&model_id) {
            Some(Some(v)) => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_scene() -> Option<Scene> {
        // Heavy: needs the retail dats (gated by AC_DAT_DIR / standard dirs).
        if dats::find_dat_dir(None).is_none() {
            return None;
        }
        Some(Scene::new(None))
    }

    /// A busy landblock (48, 115 -> lb 0x00003073) in the retail cell dat:
    /// it carries hundreds of placed GfxObj models, so terrain + statics.
    #[test]
    fn smoke_height_solids_aabb() {
        let Some(mut scene) = test_scene() else {
            eprintln!("skipped: retail dats unavailable");
            return;
        };
        assert!(scene.available());
        let lb = 0x3073u32; // (48, 115)
        // Terrain height: finite, sane range for Dereth.
        let h = scene.height_at(lb, 96.0, 96.0).expect("terrain height");
        assert!(h.is_finite());
        assert!((0.0..500.0).contains(&h));
        // A second sample is the same surface (bilinear, deterministic).
        let h2 = scene.height_at(lb, 96.0, 96.0).expect("terrain height again");
        assert!((h - h2).abs() < 1e-4);
        // The terrain soup is 8x8 quads x 2 tris x 12 floats.
        assert!(scene.load_landblock(lb));
        let soup = scene.block_soup(lb).expect("soup");
        assert_eq!(soup.len(), 8 * 8 * 2 * 12);
        // The landblock has static solids (it carries placed GfxObj
        // buildings/props), each with 9-float tris and a sane bbox.
        let solids = scene.solids_for_block(lb);
        assert!(!solids.is_empty(), "landblock should have solid statics");
        for s in &solids {
            assert_eq!(s.tris.len() % 9, 0);
            for (lo, hi) in (0..3).zip(3..6) {
                assert!(s.bbox[lo] <= s.bbox[hi]);
            }
        }
        // A human setup model composes to a roughly human-sized slab.
        let aabb = scene
            .setup_aabb(0x02000001)
            .expect("human setup AABB");
        let w = aabb[3] - aabb[0];
        let hgt = aabb[5] - aabb[2];
        assert!((0.2..3.0).contains(&w), "setup width {w}");
        assert!((0.5..3.0).contains(&hgt), "setup height {hgt}");
    }

    /// A town landblock (0x8502) in the retail cell dat: it carries env
    /// (indoor/structure) cells, so the env soup (the walls/ceiling/floor the
    /// renderer draws) is non-empty. This guards the town-interior render
    /// path — a regression here means rooms render as empty space.
    #[test]
    fn smoke_env_soup() {
        let Some(mut scene) = test_scene() else {
            eprintln!("skipped: retail dats unavailable");
            return;
        };
        assert!(scene.available());
        let lb = 0x8502u32;
        let soups = scene.env_soup(lb);
        assert!(!soups.is_empty(), "town landblock should have env soup");
        // Each cell: (cell_id, region, soup); 12 floats per directed triangle.
        for (cell_id, region, soup) in &soups {
            assert!(*cell_id != 0, "env cell id should be nonzero");
            assert_eq!(soup.len() % 12, 0, "cell soup should be whole triangles");
            // Region finite, and min <= max on every axis.
            for ax in 0..3 {
                assert!(region[ax].is_finite() && region[ax + 3].is_finite());
                assert!(region[ax] <= region[ax + 3], "region min <= max");
            }
            // Every vertex is finite (no NaN/Inf from a bad frame transform).
            for i in (0..soup.len()).step_by(12) {
                for v in soup[i..i + 9].iter() {
                    assert!(v.is_finite());
                }
            }
        }
    }

    /// A town landblock (0xc688, Rithwic) carries placed GfxObj (0x01) prop
    /// stabs — furniture/barrels/lamps, some solid (block movement) and some
    /// pass-through (no physics leaf). The prop soup is the renderer's source
    /// for those visuals; a regression here means props are invisible. Guards:
    /// non-empty, only 0x01 ids, whole 12-float tris, finite vertices, sane
    /// region. (The pass-through props still appear — the soup is not gated on
    /// the solid flag.)
    #[test]
    fn smoke_prop_soup() {
        let Some(mut scene) = test_scene() else {
            eprintln!("skipped: retail dats unavailable");
            return;
        };
        assert!(scene.available());
        let lb = 0xc688u32;
        let soups = scene.prop_soup(lb);
        assert!(!soups.is_empty(), "town landblock should have a prop soup");
        for (id, region, soup) in &soups {
            assert!(*id & 0xff000000 == 0x01000000, "prop should be a 0x01 GfxObj");
            assert_eq!(soup.len() % 12, 0, "prop soup should be whole triangles");
            assert!(!soup.is_empty(), "prop soup should be non-empty");
            for ax in 0..3 {
                assert!(region[ax].is_finite() && region[ax + 3].is_finite());
                assert!(region[ax] <= region[ax + 3], "region min <= max");
            }
            for i in (0..soup.len()).step_by(12) {
                for v in soup[i..i + 9].iter() {
                    assert!(v.is_finite(), "prop vertex finite");
                }
            }
        }
    }
    /// The Rithwic shop landblock (0xc98c) carries building GfxObj meshes —
    /// the detailed multi-material models the shops are made of, holding the
    /// interior props (paintings, barrels, lamps). This guards the
    /// building-soup path: well-formed world-space tris, sane bboxes, and
    /// that a building's region actually contains a known interior point
    /// (the first floor the player stands in).
    #[test]
    fn smoke_building_soup() {
        let Some(mut scene) = test_scene() else {
            eprintln!("skipped: retail dats unavailable");
            return;
        };
        assert!(scene.available());
        let lb = 0xc98cu32; // Rithwic: the shop block the player stands in
        let soups = scene.building_soup(lb);
        assert!(!soups.is_empty(), "shop landblock should have a building soup");
        for (id, region, soup) in &soups {
            assert!(*id & 0xff000000 == 0x01000000, "building should be a 0x01 GfxObj");
            assert_eq!(soup.len() % 12, 0, "building soup should be whole triangles");
            assert!(!soup.is_empty(), "building soup should be non-empty");
            for ax in 0..3 {
                assert!(region[ax].is_finite() && region[ax + 3].is_finite());
                assert!(region[ax] <= region[ax + 3], "region min <= max");
            }
            for i in (0..soup.len()).step_by(12) {
                for v in soup[i..i + 9].iter() {
                    assert!(v.is_finite(), "building vertex finite");
                }
            }
        }
        // A building must contain the known first-floor interior point, so the
        // camera-inside pass draws the shop's interior (not just its shell).
        let (px, py, pz) = (112.87f32, 175.40f32, 22.0f32);
        assert!(
            soups.iter().any(|(_, r, _)| {
                px >= r[0] && px <= r[3] && py >= r[1] && py <= r[4] && pz >= r[2] && pz <= r[5]
            }),
            "a building region should contain the shop's interior point"
        );
    }
    /// A town landblock (0x17b2, Redspire) carries env cells whose physics
    /// would otherwise keep a solid slab across every doorway orifice. This
    /// guards the env-physics path: well-formed 9-float tris, sane bboxes,
    /// and a non-empty result (the walls still block movement).
    #[test]
    fn env_statics_well_formed() {
        let Some(mut scene) = test_scene() else {
            eprintln!("skipped: retail dats unavailable");
            return;
        };
        assert!(scene.available());
        let lb = 0x17b2u32; // Redspire
        let solids = scene.env_statics(lb);
        assert!(!solids.is_empty(), "town landblock should have env physics");
        for s in &solids {
            assert_eq!(s.tris.len() % 9, 0, "env physics should be whole triangles");
            for (lo, hi) in (0..3).zip(3..6) {
                assert!(s.bbox[lo] <= s.bbox[hi], "bbox min <= max on every axis");
            }
            for v in &s.tris {
                assert!(v.is_finite(), "env physics vertex finite");
            }
        }
    }


    #[test]
    fn degraded_without_dats() {
        // No dats at all -> available false, all queries "no info".
        // (Constructed directly: find_dat_dir falls back to well-known
        // locations, so a bogus explicit dir is not guaranteed to fail.)
        let scene = Scene::unavailable(Some("/nonexistent-ac-dats".to_string()));
        assert!(!scene.available());
        let mut scene = scene;
        assert!(!scene.load_landblock(0));
        assert_eq!(scene.block_soup(0), None);
        assert_eq!(scene.height_at(0, 10.0, 10.0), None);
        assert!(scene.solids_for_block(0).is_empty());
        assert_eq!(scene.setup_aabb(0x02000001), None);
    }

    /// Crossing the C98C->C98D boundary must not change what is visible.
    /// F2 (grid centered on C98D, player just inside its south edge) and
    /// F1b (grid centered on C98C, the SAME world camera) are one and the
    /// same world view; both historical crossing bugs showed up as
    /// F2-vs-F1b differences here:
    ///  - the town crier actor, whose old `%192` wrap placed it 172u behind
    ///    the camera in the crossed frame (it drew nothing);
    ///  - the fully-fogged horizon band, whose old fixed-color rim jumped
    ///    192u when the 3x3 grid re-centered (a visible haze band popping).
    /// Gated on the retail dats (the C98C town must actually load).
    #[test]
    fn render_block_crossing_invariant() {
        let Some(dir) = dats::find_dat_dir(None) else {
            eprintln!("skipped: retail dats unavailable");
            return;
        };
        let mut w = crate::World3D::new(Some(&dir));
        assert!(w.available(), "retail dats present but scene unavailable");
        let h = std::f32::consts::PI; // facing -Y (south), toward the C98C town
        let crier = |x: f32, y: f32| crate::Actor {
            guid: 1,
            x,
            y,
            z: 22.0,
            quat: [1.0, 0.0, 0.0, 0.0],
            mtable_id: Some(0x09000001),
            setup_id: Some(0x02000001),
            model_parts: None,
            motion_cmd: 0,
            style: 0,
            glow: None,
            scale: 1.0,
        };
        // F2: just after crossing into C98D (player frame y = 0.002). The
        // crier NPC stands in the C98C town 6.2u across the boundary:
        // world (108.8, 185.8), i.e. C98D-frame (108.8, -6.2).
        let cam2 = crate::Camera { x: 103.11, y: 0.002, z: 22.0, heading: h };
        w.set_landblock(0xC98D);
        let f2 = w
            .render(0xC98D, &cam2, std::slice::from_ref(&crier(108.8, -6.2)), 80, 24, false, None, false, None)
            .expect("F2 frame");
        // F1b: the identical world view (camera world y = 192.002) with
        // C98C still the center block — isolates any center-vs-neighbor
        // asymmetry from the crossing itself.
        let cam1b = crate::Camera { x: 103.11, y: 192.002, z: 22.0, heading: h };
        w.set_landblock(0xC98C);
        let f1b = w
            .render(0xC98C, &cam1b, std::slice::from_ref(&crier(108.8, 185.8)), 80, 24, false, None, false, None)
            .expect("F1b frame");

        // The town is actually present (guards against a vacuous pass if
        // the scene failed to load): the 01000827 shop box in the C98C
        // town renders at this cell in the crossed frame.
        assert_eq!(f2.cells[18 * 80 + 33], [131, 129, 124], "town static missing from F2");

        // The crier draws in the crossed frame: the same camera rendered
        // with and without the actor must differ.
        w.set_landblock(0xC98D);
        let f2_no = w
            .render(0xC98D, &cam2, &[], 80, 24, false, None, false, None)
            .expect("F2 no-crier frame");
        let crier_cells = f2_no.cells.iter().zip(f2.cells.iter()).filter(|(a, b)| a != b).count();
        assert!(crier_cells > 0, "crier actor drew nothing in the crossed frame");

        // The grid center must not change the view: the old fixed-color fog
        // band showed up here as a ~63 u8 horizon rim; the residual
        // tolerance is the per-triangle flat shading of the fully-fogged
        // band against the sky gradient (a few u8).
        let max_d = f2
            .cells
            .iter()
            .zip(f1b.cells.iter())
            .map(|(a, b)| a[0].abs_diff(b[0]).max(a[1].abs_diff(b[1])).max(a[2].abs_diff(b[2])) as i32)
            .max()
            .unwrap_or(0);
        assert!(max_d <= 16, "horizon pop on block crossing: F2 vs F1b max channel diff {max_d}");
    }
}
