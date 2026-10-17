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
/// Landblock soup cache cap (the renderer only draws one landblock).
const MAX_BLOCKS: usize = 4;
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
            if let Some(buf) = portal.borrow_mut().read(t) {
                if let Some(avg) = acegeom::parse_texture_average(&buf, 512, 4096) {
                    return Some(avg);
                }
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
        Some((z00 * (1.0 - tx) + z10 * tx) * (1.0 - ty) + (z01 * (1.0 - tx) + z11 * tx) * ty)
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
                    let mut ids: Vec<(u32, Frame)> = lbi.stabs.iter().map(|s| (s.id, s.frame)).collect();
                    ids.extend(lbi.buildings.iter().copied());
                    for (id, frame) in ids {
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
}
