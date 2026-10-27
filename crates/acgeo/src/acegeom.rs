//! Byte-exact ACE record parsers (little-endian, compressed-u32 cursor):
//! RegionDesc height table, CellLandblock, LandblockInfo, Surface/texture
//! averages, GfxObj mesh + physics, SetupModel parts/tail, MotionTable,
//! Animation.
//!
//! Layouts mirror the C# loader (ACE.DatLoader FileTypes/Entity) and the
//! retail DungeonViewer unpackers; verified against the real dats. All
//! reads are little-endian.
//!
//! Coordinate system: +X east, +Y north, +Z up; a landblock is 192x192 u,
//! split 8x8 into 24x24 u tiles, described by a 9x9 vertex grid. Positions
//! on the wire are landblock-local (x,y in [0,192), z from the region
//! height table).
//!
//! Unlike the TypeScript source (which throws on truncation and lets
//! callers catch), every parser returns `None` when the record is
//! truncated or malformed — the callers' catch clauses map 1:1.

use std::collections::HashMap;

use crate::Aabb;

// ---------------------------------------------------------------------------
// reader
// ---------------------------------------------------------------------------

/// Little-endian cursor over a byte slice. Every read returns `None` when
/// the record would be truncated (the TS reader throws; callers catch).
pub struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }

    pub fn pos(&self) -> usize {
        self.p
    }

    pub fn u8(&mut self) -> Option<u8> {
        let v = self.b.get(self.p)?;
        self.p += 1;
        Some(*v)
    }

    pub fn u16(&mut self) -> Option<u16> {
        let b = self.b.get(self.p..self.p + 2)?;
        self.p += 2;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Option<u32> {
        let b = self.b.get(self.p..self.p + 4)?;
        self.p += 4;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i16(&mut self) -> Option<i16> {
        let b = self.b.get(self.p..self.p + 2)?;
        self.p += 2;
        Some(i16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i32(&mut self) -> Option<i32> {
        let b = self.b.get(self.p..self.p + 4)?;
        self.p += 4;
        Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32(&mut self) -> Option<f32> {
        let b = self.b.get(self.p..self.p + 4)?;
        self.p += 4;
        Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn vec3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }

    pub fn bytes(&mut self, n: usize) -> Option<()> {
        self.b.get(self.p..self.p + n)?;
        self.p += n;
        Some(())
    }

    /// Compressed u32 (mirrors C# ReadCompressedUInt32): width is decided
    /// by bits of the first byte, not the second.
    /// - top bit clear: 1 byte
    /// - top bit set, 0x40 clear: 2 bytes -> ((b0 & 0x7F) << 8) | b1
    /// - top bit and 0x40 set: 4 bytes -> ((((b0 & 0x3F) << 8) | b1) << 16) | u16
    pub fn c_u32(&mut self) -> Option<u32> {
        let b0 = self.b.get(self.p)?;
        if b0 & 0x80 == 0 {
            self.p += 1;
            return Some(*b0 as u32);
        }
        let b1 = self.b.get(self.p + 1)?;
        if b0 & 0x40 == 0 {
            self.p += 2;
            return Some(((b0 & 0x7f) as u32) << 8 | *b1 as u32);
        }
        let s = u16::from_le_bytes([self.b[self.p + 2], self.b[self.p + 3]]);
        self.p += 4;
        Some(((b0 & 0x3f) as u32) << 24 | (*b1 as u32) << 16 | s as u32)
    }

    /// PString: u16 length + bytes (lossy UTF-8, like `toString("utf8")`).
    pub fn pstring(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        let b = self.b.get(self.p..self.p + n)?;
        self.p += n;
        Some(String::from_utf8_lossy(b).into_owned())
    }

    /// Align the absolute buffer offset to the next 4-byte boundary.
    pub fn align4(&mut self) {
        let d = self.p % 4;
        if d != 0 {
            self.p += 4 - d;
        }
    }
}

// ---------------------------------------------------------------------------
// frames
// ---------------------------------------------------------------------------

/// Position + orientation (the wire Position's frame half).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub ox: f32,
    pub oy: f32,
    pub oz: f32,
    pub qw: f32,
    pub qx: f32,
    pub qy: f32,
    pub qz: f32,
}

/// The identity frame (no rotation, origin).
pub const IDENTITY: Frame = Frame {
    ox: 0.0,
    oy: 0.0,
    oz: 0.0,
    qw: 1.0,
    qx: 0.0,
    qy: 0.0,
    qz: 0.0,
};

impl Frame {
    pub fn new(ox: f32, oy: f32, oz: f32, qw: f32, qx: f32, qy: f32, qz: f32) -> Self {
        Self {
            ox,
            oy,
            oz,
            qw,
            qx,
            qy,
            qz,
        }
    }
}

pub fn read_frame(r: &mut R) -> Option<Frame> {
    let [ox, oy, oz] = r.vec3()?;
    let qw = r.f32()?;
    let qx = r.f32()?;
    let qy = r.f32()?;
    let qz = r.f32()?;
    Some(Frame {
        ox,
        oy,
        oz,
        qw,
        qx,
        qy,
        qz,
    })
}

/// Rotate a point by the frame's quaternion (unit, assumed normalized) and
/// translate.
pub fn apply_frame(f: &Frame, x: f32, y: f32, z: f32) -> [f32; 3] {
    let [qw, qx, qy, qz] = [f.qw, f.qx, f.qy, f.qz];
    // two quaternion-vector products
    let tx = 2.0 * (qy * z - qz * y);
    let ty = 2.0 * (qz * x - qx * z);
    let tz = 2.0 * (qx * y - qy * x);
    let rx = x + qw * tx + (qy * tz - qz * ty);
    let ry = y + qw * ty + (qz * tx - qx * tz);
    let rz = z + qw * tz + (qx * ty - qy * tx);
    [rx + f.ox, ry + f.oy, rz + f.oz]
}

/// Quaternion product a x b (composes rotations R_a then R_b).
#[inline]
pub fn q_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [aw, ax, ay, az] = a;
    let [bw, bx, by, bz] = b;
    [
        aw * bw - ax * bx - ay * by - az * bz,
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
    ]
}

/// Rotate a vector by a (unit) quaternion.
#[inline]
pub fn q_rotate(q: [f32; 4], x: f32, y: f32, z: f32) -> [f32; 3] {
    let [w, qx, qy, qz] = q;
    let tx = 2.0 * (qy * z - qz * y);
    let ty = 2.0 * (qz * x - qx * z);
    let tz = 2.0 * (qx * y - qy * x);
    [
        x + w * tx + (qy * tz - qz * ty),
        y + w * ty + (qz * tx - qx * tz),
        z + w * tz + (qx * ty - qy * tx),
    ]
}

/// Mirror a part frame across the character's sagittal plane (x -> -x): the
/// retail client's derivation of the left/back variants from the
/// right/forward cycles.
#[inline]
pub fn mirror_frame(f: Frame) -> Frame {
    Frame {
        ox: -f.ox,
        oy: f.oy,
        oz: f.oz,
        qw: f.qw,
        qx: -f.qx,
        qy: f.qy,
        qz: -f.qz,
    }
}

/// Compose objFrame o posFrame o partFrame into a single (quaternion,
/// translation) so each vertex takes one rotation + one add.
pub fn compose3(o: &Frame, p: &Frame, f: &Frame) -> ([f32; 4], [f32; 3]) {
    let oq = [o.qw, o.qx, o.qy, o.qz];
    let pq = [p.qw, p.qx, p.qy, p.qz];
    let fq = [f.qw, f.qx, f.qy, f.qz];
    let q = q_mul(oq, q_mul(pq, fq));
    let rp = q_rotate(pq, f.ox, f.oy, f.oz); // R_p . t_f
    let rx = rp[0] + p.ox;
    let ry = rp[1] + p.oy;
    let rz = rp[2] + p.oz; // + t_p
    let ro = q_rotate(oq, rx, ry, rz); // R_o . (R_p.t_f + t_p)
    let t = [ro[0] + o.ox, ro[1] + o.oy, ro[2] + o.oz]; // + t_o
    (q, t)
}

/// The model scale factor applied to actor meshes. The retail GfxObj body
/// meshes are ~2.9 world-units tall at unit scale, and the renderer's glow,
/// reticle, and click-pick anchor assume a ~2.8-unit humanoid, so the actor
/// is scaled slightly down. The scale is applied uniformly: `rot_st` scales
/// the vertices and `build_dynamic_soup` scales the part's model-space
/// anchor position by the same factor (anchored at the actor's world
/// position), so the parts keep their natural proportions instead of each
/// inflating about its own center.
pub const ACTOR_SCALE: f32 = 0.96;
/// The model's rest pose sits ~0.036 u below its origin (feet just under the
/// actor's z). Lift the body so its feet sit exactly at the actor's z (the
/// ground), matching the glow/reticle/pick convention.
pub const ACTOR_GROUND_LIFT: f32 = 0.036;


/// Rotate by q, scale by s, and add t — the per-vertex transform for a
/// composed part with a model scale factor (see `ACTOR_SCALE`).
#[inline]
pub fn rot_st(q: [f32; 4], x: f32, y: f32, z: f32, s: f32, t: [f32; 3]) -> [f32; 3] {
    rot_t(q, x * s, y * s, z * s, t)
}

/// Rotate by q and add t (the per-vertex transform for a composed part).
#[inline]
pub fn rot_t(q: [f32; 4], x: f32, y: f32, z: f32, t: [f32; 3]) -> [f32; 3] {
    let [w, qx, qy, qz] = q;
    let tx = 2.0 * (qy * z - qz * y);
    let ty = 2.0 * (qz * x - qx * z);
    let tz = 2.0 * (qx * y - qy * x);
    [
        x + w * tx + (qy * tz - qz * ty) + t[0],
        y + w * ty + (qz * tx - qx * tz) + t[1],
        z + w * tz + (qx * ty - qy * tx) + t[2],
    ]
}

// ---------------------------------------------------------------------------
// region desc (portal 0x13000000) — the 256-entry land height table
// ---------------------------------------------------------------------------

/// Parse RegionDesc through its LandDefs; returns the height table
/// (heightmap byte -> world z). Parsing stops there; the rest of the record
/// (game time, sky, scene, ...) is not needed for rendering.
pub fn parse_region_desc_height_table(buf: &[u8]) -> Option<[f32; 256]> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    r.u32()?; // RegionNumber
    r.u32()?; // Version
    r.pstring()?; // RegionName ("Dereth")
    r.align4();
    // LandDefs
    r.i32()?; // NumBlockLength
    r.i32()?; // NumBlockWidth
    r.f32()?; // SquareLength
    r.i32()?; // LBlockLength
    r.i32()?; // VertexPerCell
    r.f32()?; // MaxObjHeight
    r.f32()?; // SkyHeight
    r.f32()?; // RoadWidth
    let mut table = [0.0f32; 256];
    for i in 0..256 {
        table[i] = r.f32()?;
    }
    Some(table)
}

// ---------------------------------------------------------------------------
// cell dat: landblock
// ---------------------------------------------------------------------------

/// CellLandblock (cell dat, id = landblock|0xFFFF): the outdoor terrain.
/// 81 terrain codes + 81 height bytes, index = x*9+y, x west->east,
/// y south->north; vertex (x,y) at (x*24, y*24, table[height]).
#[derive(Clone, Copy, Debug)]
pub struct CellLandblock {
    pub has_objects: bool,
    /// 81 terrain codes.
    pub terrain: [u16; 81],
    /// 81 height bytes.
    pub height: [u8; 81],
}

pub fn parse_cell_landblock(buf: &[u8]) -> Option<CellLandblock> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    let has_objects = r.u32()? == 1;
    let mut terrain = [0u16; 81];
    let mut height = [0u8; 81];
    for i in 0..81 {
        terrain[i] = r.u16()?;
    }
    for i in 0..81 {
        height[i] = r.u8()?;
    }
    Some(CellLandblock {
        has_objects,
        terrain,
        height,
    })
}

/// Terrain code bits (CellLandblock masks).
pub const T_ERRRROAD: u16 = 0x3; // bits 0-1: road
pub const T_TYPE_MASK: u16 = 0x7c; // bits 2-6: terrain type
const T_TYPE_SHIFT: u32 = 2;

#[must_use]
pub fn terrain_road(t: u16) -> u16 {
    t & T_ERRRROAD
}

#[must_use]
pub fn terrain_type(t: u16) -> u16 {
    (t & T_TYPE_MASK) >> T_TYPE_SHIFT
}

// ---------------------------------------------------------------------------
// cell dat: landblock info (buildings / static objects)
// ---------------------------------------------------------------------------

/// A placed static object: a model data id + frame.
#[derive(Clone, Copy, Debug)]
pub struct Stab {
    pub id: u32,
    pub frame: Frame,
}

/// LandblockInfo (cell dat, id = landblock|0xFFFE). `buildings` is
/// `(modelId, frame)`.
#[derive(Clone, Debug)]
pub struct LandblockInfo {
    pub stabs: Vec<Stab>,
    pub buildings: Vec<(u32, Frame)>,
}

fn read_stabs(r: &mut R, n: u32) -> Option<Vec<Stab>> {
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let id = r.u32()?;
        let frame = read_frame(r)?;
        out.push(Stab { id, frame });
    }
    Some(out)
}

fn read_buildings(r: &mut R, n: u32) -> Option<Vec<(u32, Frame)>> {
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let model_id = r.u32()?;
        let frame = read_frame(r)?;
        r.u32()?; // NumLeaves
        let num_portals = r.u32()?;
        for _ in 0..num_portals {
            r.u16()?; // Flags
            r.u16()?; // OtherCellId
            r.u16()?; // OtherPortalId
            let num_stabs = r.u16()?;
            for _ in 0..num_stabs {
                r.u16()?;
            }
            r.align4();
        }
        out.push((model_id, frame));
    }
    Some(out)
}

pub fn parse_landblock_info(buf: &[u8]) -> Option<LandblockInfo> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    r.u32()?; // NumCells
    let n_stabs = r.u32()?;
    let stabs = read_stabs(&mut r, n_stabs)?;
    let num_buildings = r.u16()?;
    let pack_mask = r.u16()?;
    let buildings = read_buildings(&mut r, num_buildings as u32)?;
    if pack_mask & 1 != 0 {
        let n = r.u16()?;
        r.u16()?; // bucket size
        for _ in 0..n {
            r.u32()?;
            r.u32()?;
        }
    }
    Some(LandblockInfo { stabs, buildings })
}

// ---------------------------------------------------------------------------
// portal dat: surfaces (materials) and textures
// ---------------------------------------------------------------------------

/// Surface (portal 0x08xxxxxx): a material. Either a solid color or a
/// texture reference.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    /// 0x00RRGGBB when solid.
    pub color: Option<u32>,
    pub texture_id: Option<u32>,
    /// 0.0 = opaque, 1.0 = fully transparent (glass).
    pub translucency: f32,
}

pub fn parse_surface(buf: &[u8]) -> Option<Surface> {
    let mut r = R::new(buf);
    let ty = r.u32()?;
    let mut color = None;
    let mut texture_id = None;
    if ty & 0x6 != 0 {
        // Base1Image (0x2) / Base1ClipMap (0x4)
        texture_id = r.u32();
        r.u32()?; // OrigPaletteId
    } else {
        color = r.u32();
    }
    let translucency = r.f32()?; // Translucency
    r.f32()?; // Luminosity
    r.f32()?; // Diffuse
    Some(Surface {
        color,
        texture_id,
        translucency,
    })
}

/// SurfaceTexture (portal 0x05xxxxxx): an ordered list of texture data
/// ids; the first is the base map.
pub fn parse_surface_texture_ids(buf: &[u8]) -> Option<Vec<u32>> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    r.i32()?; // Unknown
    r.u8()?; // UnknownByte
    let n = r.i32()? as usize;
    let mut ids = Vec::with_capacity(n);
    for _ in 0..n {
        ids.push(r.u32()?);
    }
    Some(ids)
}

#[inline]
fn rgb565(v: u16) -> (u32, u32, u32) {
    (
        ((v >> 11) & 0x1f) as u32 * (255 / 31) as u32,
        ((v >> 5) & 0x3f) as u32 * (255 / 63) as u32,
        (v & 0x1f) as u32 * (255 / 31) as u32,
    )
}

/// Average color of a Texture record (portal 0x06xxxxxx), or `None` when
/// the format needs a palette or is unknown. Samples at most `blocks` DXT
/// blocks or `max_pixels` raw pixels — enough for a material tint.
pub fn parse_texture_average(buf: &[u8], blocks: usize, max_pixels: usize) -> Option<u32> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    r.i32()?; // Unknown
    let _w = r.i32()?;
    let _h = r.i32()?;
    let format = r.u32()?;
    let len = r.i32()? as usize;
    let _ = r.bytes(len)?;
    let data = &buf[r.pos() - len..r.pos()];
    let mut rs: u64 = 0;
    let mut gs: u64 = 0;
    let mut bs: u64 = 0;
    let mut n: u64 = 0;

    macro_rules! add {
        ($cr:expr, $cg:expr, $cb:expr, $count:expr) => {{
            rs += $cr as u64 * $count as u64;
            gs += $cg as u64 * $count as u64;
            bs += $cb as u64 * $count as u64;
            n += $count as u64;
        }};
    }

    match format {
        20 => {
            // PFID_R8G8B8
            let mut i = 0;
            while i + 2 < data.len() && n < max_pixels as u64 {
                add!(data[i], data[i + 1], data[i + 2], 1);
                i += 3;
            }
        }
        240 | 21 => {
            // PFID_CUSTOM_R8G8B8A8 / PFID_A8R8G8B8
            let mut i = 0;
            while i + 3 < data.len() && n < max_pixels as u64 {
                add!(data[i + 1], data[i + 2], data[i + 3], 1);
                i += 4;
            }
        }
        241 | 32 => {
            // PFID_CUSTOM_A8B8G8R8 / PFID_A8B8G8R8
            let mut i = 0;
            while i + 3 < data.len() && n < max_pixels as u64 {
                add!(data[i + 3], data[i + 2], data[i + 1], 1);
                i += 4;
            }
        }
        242 | 243 | 33 => {
            // PFID_CUSTOM_B8G8R8 / PFID_CUSTOM_LSCAPE_R8G8B8 (B,G,R on disk) /
            // PFID_X8B8G8R8
            let mut i = 0;
            while i + 2 < data.len() && n < max_pixels as u64 {
                add!(data[i + 2], data[i + 1], data[i], 1);
                i += 4;
            }
        }
        22 => {
            // PFID_X8R8G8B8
            let mut i = 0;
            while i + 2 < data.len() && n < max_pixels as u64 {
                add!(data[i + 1], data[i + 2], data[i + 3], 1);
                i += 4;
            }
        }
        23 | 101 => {
            // PFID_R5G6B5. Format 101 is the same 16-bit RGB used by
            // character/prop textures (e.g. the shop NPCs); without it those
            // surfaces resolve to no color and the mesh is dropped.
            let mut i = 0;
            while i + 1 < data.len() && n < max_pixels as u64 {
                let v = u16::from_le_bytes([data[i], data[i + 1]]);
                let (cr, cg, cb) = rgb565(v);
                add!(cr, cg, cb, 1);
                i += 2;
            }
        }
        26 => {
            // PFID_A4R4G4B4
            let mut i = 0;
            while i + 1 < data.len() && n < max_pixels as u64 {
                let v = u16::from_le_bytes([data[i], data[i + 1]]);
                add!(((v >> 12) & 0xf) * 17, ((v >> 8) & 0xf) * 17, ((v >> 4) & 0xf) * 17, 1);
                i += 2;
            }
        }
        30 => {
            // PFID_X4R4G4B4
            let mut i = 0;
            while i + 1 < data.len() && n < max_pixels as u64 {
                let v = u16::from_le_bytes([data[i], data[i + 1]]);
                add!(((v >> 8) & 0xf) * 17, ((v >> 4) & 0xf) * 17, (v & 0xf) * 17, 1);
                i += 2;
            }
        }
        50 => {
            // PFID_L8
            let mut i = 0;
            while i < data.len() && n < max_pixels as u64 {
                add!(data[i], data[i], data[i], 1);
                i += 1;
            }
        }
        827611204 | 861165636 | 894720068 => {
            // PFID_DXT1 / DXT3 / DXT5
            let block_bytes = if format == 827611204 { 8 } else { 16 };
            let nb = blocks.min(data.len() / block_bytes);
            for blk in 0..nb {
                let o = blk * block_bytes;
                let c0 = u16::from_le_bytes([data[o], data[o + 1]]);
                let c1 = u16::from_le_bytes([data[o + 2], data[o + 3]]);
                let p0 = rgb565(c0);
                let p1 = rgb565(c1);
                let (p2, p3) = if c0 > c1 {
                    (
                        (
                            (2 * p0.0 + p1.0) / 3,
                            (2 * p0.1 + p1.1) / 3,
                            (2 * p0.2 + p1.2) / 3,
                        ),
                        (
                            (p0.0 + 2 * p1.0) / 3,
                            (p0.1 + 2 * p1.1) / 3,
                            (p0.2 + 2 * p1.2) / 3,
                        ),
                    )
                } else {
                    (
                        ((p0.0 + p1.0) / 2, (p0.1 + p1.1) / 2, (p0.2 + p1.2) / 2),
                        (0, 0, 0),
                    )
                };
                let pal = [p0, p1, p2, p3];
                for k in 0..16u32 {
                    // DXT1 packs four 2-bit indices per byte (16 total across
                    // the 4 index bytes at o+4..o+8); `k/4` keeps the read
                    // inside the block (the old `k/2` read 8 index bytes and
                    // ran past the end on the last block).
                    let idx = (data[o + 4 + (k / 4) as usize] >> ((k % 4) * 2)) & 0x3;
                    let c = pal[idx as usize];
                    add!(c.0, c.1, c.2, 1);
                }
            }
        }
        _ => return None, // paletted/unknown: no color without the palette chain
    }
    if n == 0 {
        return None;
    }
    Some(
        0xff000000
            | (255.min((rs / n) as u32) << 16)
            | (255.min((gs / n) as u32) << 8)
            | 255.min((bs / n) as u32),
    )
}

/// The trailing palette id of a paletted texture record (format 101: 2 bytes
/// per pixel, each a u16-LE index into a 2048-entry palette), or `None` when
/// the record is not that format. The palette id is the final 4-byte field
/// after the pixel payload.
pub fn parse_texture_palette_id(buf: &[u8]) -> Option<u32> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    r.i32()?; // flags
    r.i32()?; // width
    r.i32()?; // height
    if r.u32()? != 101 {
        return None;
    }
    if buf.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([
        buf[buf.len() - 4],
        buf[buf.len() - 3],
        buf[buf.len() - 2],
        buf[buf.len() - 1],
    ]))
}

/// Average color of a paletted texture payload (format 101). `data` is the
/// pixel bytes (2/pixel, u16-LE palette index); `palette` is the palette
/// record (8-byte id+count header then `count` RGBA entries). Returns
/// `None` on a corrupt index or short buffer.
pub fn average_paletted(data: &[u8], palette: &[u8], max_pixels: usize) -> Option<u32> {
    if palette.len() < 8 {
        return None;
    }
    let count = u32::from_le_bytes([palette[4], palette[5], palette[6], palette[7]]) as usize;
    if 8 + count * 4 > palette.len() {
        return None;
    }
    let mut rs: u64 = 0;
    let mut gs: u64 = 0;
    let mut bs: u64 = 0;
    let mut n: u64 = 0;
    let mut i = 0;
    while i + 1 < data.len() && n < max_pixels as u64 {
        let idx = u16::from_le_bytes([data[i], data[i + 1]]) as usize;
        if idx >= count {
            return None;
        }
        let o = 8 + idx * 4;
        rs += palette[o] as u64;
        gs += palette[o + 1] as u64;
        bs += palette[o + 2] as u64;
        n += 1;
        i += 2;
    }
    if n == 0 {
        return None;
    }
    Some(
        0xff000000
            | (((rs / n) as u32) << 16)
            | (((gs / n) as u32) << 8)
            | (bs / n) as u32,
    )
}

// ---------------------------------------------------------------------------
// polygons and vertex arrays (mesh geometry)
// ---------------------------------------------------------------------------

/// One polygon (planar N-gon) from a GfxObj/CellStruct.
#[derive(Clone, Debug)]
pub struct Poly {
    pub num_pts: usize,
    /// CullMode: 1 = None (two identical sides), 2 = Clockwise.
    pub sides_type: i32,
    pub pos_surface: i16,
    pub neg_surface: i16,
    pub vertex_ids: Vec<i16>,
}

const CULL_NONE: i32 = 1;
const CULL_CLOCKWISE: i32 = 2;
const STIP_NOPOS: u8 = 0x4;
const STIP_NONEG: u8 = 0x8;

pub fn read_polygon(r: &mut R) -> Option<Poly> {
    let num_pts = r.u8()? as usize;
    let stippling = r.u8()?;
    let sides_type = r.i32()?;
    let pos_surface = r.i16()?;
    let mut neg_surface = r.i16()?;
    let mut vertex_ids = Vec::with_capacity(num_pts);
    for _ in 0..num_pts {
        vertex_ids.push(r.i16()?);
    }
    if stippling & STIP_NOPOS == 0 {
        for _ in 0..num_pts {
            r.u8()?;
        }
    }
    if sides_type == CULL_CLOCKWISE && stippling & STIP_NONEG == 0 {
        for _ in 0..num_pts {
            r.u8()?;
        }
    }
    if sides_type == CULL_NONE {
        neg_surface = pos_surface;
    }
    Some(Poly {
        num_pts,
        sides_type,
        pos_surface,
        neg_surface,
        vertex_ids,
    })
}

/// CVertexArray (VertexType 1): a dictionary of SWVertices keyed by u16
/// (each entry: u16 key, u16 numUVs, vec3 Origin, vec3 Normal,
/// numUVs x (2 f32 UV)). Polygons reference vertices by key, so both the
/// read-order positions and the per-slot keys are returned.
#[derive(Clone, Debug)]
pub struct VertexData {
    /// 3 floats per vertex, read order.
    pub pos: Vec<f32>,
    /// Key of each read-order slot.
    pub keys: Vec<u16>,
}

pub fn read_vertex_positions(r: &mut R) -> Option<VertexData> {
    let vertex_type = r.i32()?;
    let count = r.u32()?;
    if vertex_type != 1 || count > 1_000_000 {
        return None;
    }
    let count = count as usize;
    let mut pos = Vec::with_capacity(count * 3);
    let mut keys = Vec::with_capacity(count);
    for _ in 0..count {
        keys.push(r.u16()?);
        let num_uvs = r.u16()?;
        for _ in 0..3 {
            pos.push(r.f32()?);
        }
        for _ in 0..3 {
            r.f32()?; // normal
        }
        for _ in 0..num_uvs {
            r.f32()?;
            r.f32()?; // UV
        }
    }
    Some(VertexData { pos, keys })
}

/// Build a key -> slot reverse map for a vertex array; returns `None` when
/// keys are already 0..count-1 in order (identity).
pub fn key_to_slot_map(keys: &[u16]) -> Option<Vec<i32>> {
    for (i, &k) in keys.iter().enumerate() {
        if k as usize != i {
            let mut m = vec![-1i32; 0x10000];
            for (j, &k2) in keys.iter().enumerate() {
                m[k2 as usize] = j as i32;
            }
            return Some(m);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// BSP skipping (the drawing/physics/cell BSPs carry no geometry we need)
// ---------------------------------------------------------------------------

const T_PORT: u32 = 0x504f5254;
const T_LEAF: u32 = 0x4c454146;
// Interior node markers: 4-byte ASCII tags; two spellings exist for each
// child combination (the retail parser accepts both). The comment carries
// the exact retail marker.
const T_BPNN: u32 = 0x42506e6e;   // "BPnn" — positive child only
const T_BP_IN: u32 = 0x4250496e;  // "BPIn" — positive child only
const T_BPIN: u32 = 0x4270494e;   // "BpIN" — negative child only
const T_BPN_N: u32 = 0x42706e4e;  // "BpnN" — negative child only
const T_BP_IN2: u32 = 0x4250494e; // "BPIN" — both children
const T_BPN_N2: u32 = 0x42506e4e; // "BPNn" — both children

/// Which BSP layout is being skipped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BspKind {
    Cell,
    Physics,
    Drawing,
}

/// Skip a BSP tree; `None` on truncation or a corrupt (too deep) tree.
pub fn skip_bsp(r: &mut R, kind: BspKind) -> Option<()> {
    skip_bsp_node(r, kind, 0)
}

fn skip_bsp_node(r: &mut R, kind: BspKind, depth: usize) -> Option<()> {
    if depth > 4096 {
        return None; // "BSP tree too deep (corrupt?)"
    }
    let ty = r.u32()?;
    if ty == T_LEAF {
        r.i32()?; // LeafIndex
        if kind == BspKind::Physics {
            r.i32()?; // Solid
            r.bytes(16)?; // Sphere
            let n = r.u32()?;
            for _ in 0..n {
                r.u16()?;
            }
        }
        return Some(());
    }
    if ty == T_PORT {
        r.bytes(16)?; // SplittingPlane
        skip_bsp_node(r, kind, depth + 1)?;
        skip_bsp_node(r, kind, depth + 1)?;
        if kind == BspKind::Drawing {
            r.bytes(16)?; // Sphere
            let n_poly = r.u32()?;
            for _ in 0..n_poly {
                r.u16()?;
            }
            let n_portal = r.u32()?;
            for _ in 0..n_portal {
                r.bytes(4)?; // PortalPoly
            }
        }
        return Some(());
    }
    // interior node
    r.bytes(16)?; // SplittingPlane
    let (pos, neg) = match ty {
        T_BPNN | T_BP_IN => (true, false),
        T_BPIN | T_BPN_N => (false, true),
        T_BP_IN2 | T_BPN_N2 => (true, true),
        // unknown node marker: C# reads no children for unlisted types
        _ => (false, false),
    };
    if pos {
        skip_bsp_node(r, kind, depth + 1)?;
    }
    if neg {
        skip_bsp_node(r, kind, depth + 1)?;
    }
    if kind == BspKind::Cell {
        return Some(());
    }
    r.bytes(16)?; // Sphere
    if kind == BspKind::Physics {
        return Some(());
    }
    let n = r.u32()?;
    for _ in 0..n {
        r.u16()?;
    }
    Some(())
}

// ---------------------------------------------------------------------------
// GfxObj (portal 0x01xxxxxx): a mesh
// ---------------------------------------------------------------------------

/// A triangle mesh. `tris` holds directed triangles (both windings of each
/// source triangle, so a flat-shaded z-buffer pass can backface-cull per
/// screen winding); `tri_mat[i]` indexes `mats` — the material 0xRRGGBB
/// (or 0xffRRGGBB texture average) for that winding.
#[derive(Clone, Debug)]
pub struct Mesh {
    /// Vertex positions (3 floats each).
    pub pos: Vec<f32>,
    /// Directed triangle vertex indices (3 per tri).
    pub tris: Vec<u32>,
    /// Per directed triangle; indexes `mats`.
    pub tri_mat: Vec<u16>,
    /// Unique material colors 0xRRGGBB / 0xffRRGGBB.
    pub mats: Vec<u32>,
    /// [min_x, min_y, min_z, max_x, max_y, max_z].
    pub bbox: Aabb,
    pub vertex_count: usize,
    /// Directed triangles.
    pub triangle_count: usize,
}

/// Build a Mesh from a GfxObj buffer. `surface_color` resolves a Surface
/// data id (0x08xxxxxx) to a 0x00RRGGBB or `None`; `frame` optionally
/// transforms the (model-space) geometry into world space.
pub fn parse_gfx_obj(
    buf: &[u8],
    mut surface_color: impl FnMut(u32) -> Option<u32>,
    frame: Option<&Frame>,
) -> Option<Mesh> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    let flags = r.u32()?;
    let n_surfaces = r.c_u32()? as usize;
    let mut surface_ids = Vec::with_capacity(n_surfaces);
    for _ in 0..n_surfaces {
        surface_ids.push(r.u32()?);
    }

    let vdata = read_vertex_positions(&mut r)?;
    let vcount = vdata.pos.len() / 3;
    let key_to_slot = key_to_slot_map(&vdata.keys);
    let slot_of = |key: u32| -> i32 {
        // A corrupt record may reference a key with no vertex; the TS
        // client's `Vertices[(ushort)id]` throw is caught by its callers,
        // which map to this parser's "drop the polygon" behavior.
        let slot = match &key_to_slot {
            None => key as i32,
            Some(m) => m[key as usize],
        };
        if slot >= vcount as i32 {
            -1
        } else {
            slot
        }
    };

    if flags & 0x1 != 0 {
        // physics polygons + physics BSP
        let n_poly = r.c_u32()?;
        for _ in 0..n_poly {
            r.u16()?; // key
            read_polygon(&mut r)?;
        }
        skip_bsp(&mut r, BspKind::Physics)?;
    }
    r.bytes(12)?; // SortCenter

    let mut polys: Vec<Poly> = Vec::new();
    let mut has_drawing = false;
    if flags & 0x2 != 0 {
        has_drawing = true;
        let n_poly = r.c_u32()?;
        for _ in 0..n_poly {
            r.u16()?; // key
            polys.push(read_polygon(&mut r)?);
        }
        // The drawing BSP (next) is a visibility acceleration structure the
        // server never parses; its retail layout isn't needed for rendering.
        // The Polygons list above is the complete geometry. Stop here.
    }
    if !has_drawing && polys.is_empty() {
        return None;
    }

    // resolve materials (cached per surface index). `None` = skip this
    // winding: the surface has no resolvable color (a palette-only texture
    // with no average) or it is glass — the Scene resolver returns `None`
    // for translucency >= 0.5 so window orifices stay see-through.
    let mut mat_of: Vec<Option<Option<u32>>> = vec![None; n_surfaces];
    let mut mat_ids: Vec<u32> = Vec::new();
    let mut resolve = |surf_idx: i16| -> Option<u16> {
        if !((0..n_surfaces).contains(&((surf_idx as i64) as usize)) && surf_idx >= 0) {
            return None;
        }
        let m = mat_of[surf_idx as usize].get_or_insert_with(|| {
            surface_color(surface_ids[surf_idx as usize])
        });
        m.as_ref().map(|&c| {
            if !mat_ids.contains(&c) {
                mat_ids.push(c);
            }
            mat_ids.iter().position(|x| *x == c).unwrap() as u16
        })
    };

    // transform vertices to world space
    let mut pos = vec![0.0f32; vdata.pos.len()];
    if let Some(frame) = frame {
        for i in (0..vdata.pos.len()).step_by(3) {
            let v = apply_frame(frame, vdata.pos[i], vdata.pos[i + 1], vdata.pos[i + 2]);
            pos[i] = v[0];
            pos[i + 1] = v[1];
            pos[i + 2] = v[2];
        }
    } else {
        pos.copy_from_slice(&vdata.pos);
    }

    // fan-triangulate each polygon; emit the front winding and, when a
    // back surface is declared, the back winding. A polygon whose front
    // surface is unresolvable or glass is skipped entirely.
    let mut tris: Vec<u32> = Vec::new();
    let mut tri_mat: Vec<u16> = Vec::new();
    for poly in &polys {
        if poly.num_pts < 3 {
            continue;
        }
        let Some(m_pos) = resolve(poly.pos_surface) else {
            continue;
        };
        // Two-sided only when a back surface is declared: CULL_NONE reuses
        // the front surface, CULL_CLOCKWISE names a distinct one. Any other
        // neg_surface (e.g. -1) means the polygon is single-sided — emit
        // only the front winding.
        let back = if poly.sides_type == CULL_NONE {
            Some(m_pos)
        } else if poly.neg_surface >= 0 && (poly.neg_surface as usize) < n_surfaces {
            resolve(poly.neg_surface)
        } else {
            None
        };
        for i in 1..poly.num_pts - 1 {
            let a = slot_of(poly.vertex_ids[0] as u32 & 0xffff);
            let b = slot_of(poly.vertex_ids[i] as u32 & 0xffff);
            let c = slot_of(poly.vertex_ids[i + 1] as u32 & 0xffff);
            if a < 0 || b < 0 || c < 0 {
                continue;
            }
            tris.extend_from_slice(&[a as u32, b as u32, c as u32]);
            tri_mat.extend_from_slice(&[m_pos]);
            if let Some(m_neg) = back {
                tris.extend_from_slice(&[a as u32, c as u32, b as u32]);
                tri_mat.extend_from_slice(&[m_neg]);
            }
        }
    }

    // bbox (over all vertex positions)
    let mut bbox = [
        f32::INFINITY,
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for i in (0..pos.len()).step_by(3) {
        if pos[i] < bbox[0] {
            bbox[0] = pos[i];
        }
        if pos[i + 1] < bbox[1] {
            bbox[1] = pos[i + 1];
        }
        if pos[i + 2] < bbox[2] {
            bbox[2] = pos[i + 2];
        }
        if pos[i] > bbox[3] {
            bbox[3] = pos[i];
        }
        if pos[i + 1] > bbox[4] {
            bbox[4] = pos[i + 1];
        }
        if pos[i + 2] > bbox[5] {
            bbox[5] = pos[i + 2];
        }
    }

    Some(Mesh {
        vertex_count: pos.len() / 3,
        triangle_count: tris.len() / 3,
        pos,
        tris,
        tri_mat,
        mats: mat_ids,
        bbox,
    })
}

// ---------------------------------------------------------------------------
// GfxObj physics (the collision hull; the drawing BSP is skipped above)
// ---------------------------------------------------------------------------

/// The GfxObj physics hull: the fan-triangulated physics polygons (one
/// winding — collision tests are double-sided) + the physics-BSP solid
/// flag. `tris`/`bbox` are in `frame` space (model space when `frame` is
/// `None`).
#[derive(Clone, Debug)]
pub struct GfxPhysics {
    /// 9 floats per triangle.
    pub tris: Vec<f32>,
    /// True when any physics-BSP leaf is marked solid (Solid != 0): the
    /// object blocks movement.
    pub solid: bool,
    /// [min_x, min_y, min_z, max_x, max_y, max_z].
    pub bbox: Aabb,
}

/// Walk a physics BSP (skip_bsp's "physics" layout) and report whether any
/// leaf is solid.
fn physics_bsp_solid(r: &mut R) -> Option<bool> {
    physics_bsp_node(r, false, 0)
}

fn physics_bsp_node(r: &mut R, solid: bool, depth: usize) -> Option<bool> {
    if depth > 4096 {
        return None;
    }
    let ty = r.u32()?;
    if ty == T_LEAF {
        r.i32()?; // LeafIndex
        let solid = r.i32()? != 0 || solid; // Solid
        r.bytes(16)?; // Sphere
        let n = r.u32()?;
        for _ in 0..n {
            r.u16()?;
        }
        return Some(solid);
    }
    if ty == T_PORT {
        r.bytes(16)?; // SplittingPlane
        let s = physics_bsp_node(r, solid, depth + 1)?;
        return physics_bsp_node(r, s, depth + 1);
    }
    r.bytes(16)?; // SplittingPlane
    let (pos, neg) = match ty {
        T_BPNN | T_BP_IN => (true, false),
        T_BPIN | T_BPN_N => (false, true),
        T_BP_IN2 | T_BPN_N2 => (true, true),
        _ => (false, false),
    };
    let mut s = solid;
    if pos {
        s = physics_bsp_node(r, s, depth + 1)?;
    }
    if neg {
        s = physics_bsp_node(r, s, depth + 1)?;
    }
    r.bytes(16)?; // Sphere
    Some(s)
}

/// Parse a GfxObj's physics section (flag 0x1): the physics polygons
/// fan-triangulated into triangles in `frame` space + the solid flag.
/// `None` when the record has no physics or no usable vertex data.
pub fn parse_gfx_obj_physics(buf: &[u8], frame: Option<&Frame>) -> Option<GfxPhysics> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    let flags = r.u32()?;
    let n_surfaces = r.c_u32()? as usize;
    for _ in 0..n_surfaces {
        r.u32()?;
    }
    let vdata = read_vertex_positions(&mut r)?;
    if flags & 0x1 == 0 {
        return None;
    }
    let vcount = vdata.pos.len() / 3;
    let key_to_slot = key_to_slot_map(&vdata.keys);
    let slot_of = |key: u32| -> i32 {
        // Same corrupt-key guard as `parse_gfx_obj`: a key with no vertex
        // drops the referencing polygon instead of indexing past `pos`.
        let slot = match &key_to_slot {
            None => key as i32,
            Some(m) => m[key as usize],
        };
        if slot >= vcount as i32 {
            -1
        } else {
            slot
        }
    };

    let n_poly = r.c_u32()?;
    let mut polys: Vec<Poly> = Vec::new();
    for _ in 0..n_poly {
        r.u16()?; // key
        polys.push(read_polygon(&mut r)?);
    }
    let solid = physics_bsp_solid(&mut r)?;

    let mut idx: Vec<usize> = Vec::new();
    for poly in &polys {
        if poly.num_pts < 3 {
            continue;
        }
        let a = slot_of(poly.vertex_ids[0] as u32 & 0xffff);
        if a < 0 {
            continue;
        }
        let a = a as usize;
        for i in 1..poly.num_pts - 1 {
            let b = slot_of(poly.vertex_ids[i] as u32 & 0xffff);
            let c = slot_of(poly.vertex_ids[i + 1] as u32 & 0xffff);
            if b < 0 || c < 0 {
                continue;
            }
            idx.extend_from_slice(&[a, b as usize, c as usize]);
        }
    }

    let mut tris = Vec::with_capacity(idx.len() * 3);
    let mut bbox = [
        f32::INFINITY,
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for &s in &idx {
        let mut v = [vdata.pos[s * 3], vdata.pos[s * 3 + 1], vdata.pos[s * 3 + 2]];
        if let Some(frame) = frame {
            v = apply_frame(frame, v[0], v[1], v[2]);
        }
        tris.extend_from_slice(&v);
        if v[0] < bbox[0] {
            bbox[0] = v[0];
        }
        if v[1] < bbox[1] {
            bbox[1] = v[1];
        }
        if v[2] < bbox[2] {
            bbox[2] = v[2];
        }
        if v[0] > bbox[3] {
            bbox[3] = v[0];
        }
        if v[1] > bbox[4] {
            bbox[4] = v[1];
        }
        if v[2] > bbox[5] {
            bbox[5] = v[2];
        }
    }
    Some(GfxPhysics { tris, solid, bbox })
}

// ---------------------------------------------------------------------------
// cell dat: indoor cells (env cells) and their pre-fab environments
// ---------------------------------------------------------------------------

/// EnvCell (cell dat, id = landblock|cellnum, cellnum in 0x100..0xFFFD).
#[derive(Clone, Debug)]
pub struct EnvCell {
    pub flags: u32,
    /// 0x08xxxxxx.
    pub surface_ids: Vec<u32>,
    /// 0x0Dxxxxxx.
    pub environment_id: u32,
    /// Index into the Environment's Cells.
    pub cell_structure: u16,
    pub position: Frame,
    /// Statics placed inside the cell (props: stairs/candlesticks/paintings,
    /// plus SetupModel characters). Frames are block-local (world) space —
    /// not the cell's local frame — so they are drawn directly. Present only
    /// when the `0x2` (HasStaticObjs) flag is set.
    pub statics: Vec<Stab>,
}

pub fn parse_env_cell(buf: &[u8]) -> Option<EnvCell> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    let flags = r.u32()?;
    r.u32()?; // Id again
    let num_surfaces = r.u8()?;
    let num_portals = r.u8()?;
    let num_visible = r.u16()?;
    let mut surface_ids = Vec::with_capacity(num_surfaces as usize);
    for _ in 0..num_surfaces {
        surface_ids.push(0x08000000 | r.u16()? as u32);
    }
    let environment_id = 0x0d000000 | r.u16()? as u32;
    let cell_structure = r.u16()?;
    let position = read_frame(&mut r)?;
    for _ in 0..num_portals {
        r.bytes(8)?; // CellPortal
    }
    for _ in 0..num_visible {
        r.u16()?; // VisibleCells
    }
    let mut statics = Vec::new();
    if flags & 0x2 != 0 {
        let n = r.u32()?;
        statics = read_stabs(&mut r, n)?;
    }
    // `0x4` (dynamic spawns) is server-only and absent from the client cell
    // dat; `0x8` is a restriction instance id.
    if flags & 0x8 != 0 {
        r.u32()?;
    }
    Some(EnvCell {
        flags,
        surface_ids,
        environment_id,
        cell_structure,
        position,
        statics,
    })
}

/// One CellStruct's drawing geometry.
#[derive(Clone, Debug)]
pub struct EnvData {
    pub pos: Vec<f32>,
    pub keys: Vec<u16>,
    pub polys: Vec<Poly>,
    /// The physics polygons fan-triangulated (9 floats/tri) in model space
    /// — the CellStruct's placement frame is applied by the caller. These
    /// are the cell's collision surfaces (a town's walls exist only here —
    /// its LBI has no stabs); empty when the record has no usable physics
    /// geometry. The physics BSP's Solid flag is never set in env cells
    /// (0 of 734,976 retail cells) — it is a GfxObj-only construct.
    pub phys_tris: Vec<f32>,
}

/// Fan-triangulate `polys` against `vdata`'s vertex array into model-space
/// triangles (9 floats/tri). Same corrupt-key guard as
/// `parse_gfx_obj_physics`: a key with no vertex drops the referencing
/// polygon instead of indexing past `pos`.
fn fan_polygons(vdata: &VertexData, polys: &[Poly]) -> Vec<f32> {
    let vcount = vdata.pos.len() / 3;
    let key_to_slot = key_to_slot_map(&vdata.keys);
    let mut tris = Vec::new();
    for poly in polys {
        if poly.num_pts < 3 {
            continue;
        }
        let slot_of = |key: i16| -> Option<usize> {
            let slot = match &key_to_slot {
                None => key as u32 & 0xffff,
                Some(m) => m[(key as u32 & 0xffff) as usize] as u32,
            };
            (slot < vcount as u32).then_some(slot as usize)
        };
        let Some(a) = slot_of(poly.vertex_ids[0]) else {
            continue;
        };
        for i in 1..poly.num_pts - 1 {
            let Some(b) = slot_of(poly.vertex_ids[i]) else {
                continue;
            };
            let Some(c) = slot_of(poly.vertex_ids[i + 1]) else {
                continue;
            };
            for &s in [a, b, c].iter() {
                tris.extend_from_slice(&vdata.pos[s * 3..s * 3 + 3]);
            }
        }
    }
    tris
}

/// Environment (portal 0x0Dxxxxxx): pre-fab dungeon block. Returns the
/// CellStruct's drawing polygons + vertex positions and its physics
/// polygons fan-triangulated (`phys_tris`, the collision surfaces) for the
/// given cell structure key, or `None`.
pub fn parse_environment(buf: &[u8], cell_structure: u32) -> Option<EnvData> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    let n_cells = r.u32()?;
    for _ in 0..n_cells {
        let key = r.u32()?;
        let n_polygons = r.u32()?;
        let n_physics_polygons = r.u32()?;
        let n_portals = r.u32()?;
        let vdata = read_vertex_positions(&mut r)?;
        let mut polys: Vec<Poly> = Vec::new();
        for _ in 0..n_polygons {
            r.u16()?; // key
            polys.push(read_polygon(&mut r)?);
        }
        for _ in 0..n_portals {
            r.u16()?;
        }
        r.align4();
        skip_bsp(&mut r, BspKind::Cell)?;
        let mut phys_polys: Vec<Poly> = Vec::new();
        for _ in 0..n_physics_polygons {
            r.u16()?; // key
            phys_polys.push(read_polygon(&mut r)?);
        }
        skip_bsp(&mut r, BspKind::Physics)?;
        let has_drawing_bsp = r.u32()?;
        if has_drawing_bsp != 0 {
            skip_bsp(&mut r, BspKind::Drawing)?;
        }
        r.align4();
        if key == cell_structure {
            let phys_tris = fan_polygons(&vdata, &phys_polys);
            return Some(EnvData {
                pos: vdata.pos,
                keys: vdata.keys,
                polys,
                phys_tris,
            });
        }
    }
    None
}

// ---------------------------------------------------------------------------
// portal dat: setup model (0x02) — part list (partial parse)
// ---------------------------------------------------------------------------

/// SetupModel header (portal 0x02xxxxxx): the per-part GfxObj ids.
/// Layout: u32 Id, u32 Flags (HasParent=0x1, HasDefaultScale=0x2,
/// AllowFreeHeading=0x4, HasPhysicsBSP=0x8), u32 numParts, numParts x u32
/// parts, [HasParent: numParts x u32 parentIndex],
/// [HasDefaultScale: numParts x vec3]. Only the part list is needed for
/// rendering: the per-frame part transforms in the Animation records are
/// model-space absolute (verified against the human setup — knees at
/// z~0.5, feet at z~0.08 in root space), so the parent chain (a physics
/// construct) is not composed.
pub fn parse_setup_parts(buf: &[u8]) -> Option<Vec<u32>> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    let flags = r.u32()?;
    let n = r.u32()? as usize;
    let mut parts = Vec::with_capacity(n);
    for _ in 0..n {
        parts.push(r.u32()?);
    }
    if flags & 0x1 != 0 {
        r.bytes(4 * n)?;
    }
    if flags & 0x2 != 0 {
        r.bytes(12 * n)?;
    }
    Some(parts)
}

/// The embedded per-part static pose of a non-animated SetupModel: a 16-byte
/// header (`[0, 0, 1, 0]`), then one Frame (x,y,z,qw,qx,qy,qz) per part, in
/// part order. Present on the static staircases — it is the parts' model-space
/// placement (the stringers, the treads rising ~0.25 per step, the top
/// landing). `None` when the bytes after the header don't look like per-part
/// frames (the quaternions aren't ~unit), so callers fall back to identity.
pub fn parse_setup_part_frames(buf: &[u8]) -> Option<Vec<Frame>> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    let flags = r.u32()?;
    let n = r.u32()? as usize;
    if n == 0 {
        return None;
    }
    r.bytes(4 * n)?; // parts
    if flags & 0x1 != 0 {
        r.bytes(4 * n)?; // parentIndex
    }
    if flags & 0x2 != 0 {
        r.bytes(12 * n)?; // defaultScale
    }
    // 16-byte header before the frames: [0, 0, numKeyframes, 0].
    r.bytes(16)?;
    let mut frames = Vec::with_capacity(n);
    for _ in 0..n {
        let f = read_frame(&mut r)?;
        let q2 = f.qw * f.qw + f.qx * f.qx + f.qy * f.qy + f.qz * f.qz;
        if !(0.5..=2.0).contains(&q2) {
            return None;
        }
        frames.push(f);
    }
    Some(frames)
}

/// The 5 trailing data ids of a SetupModel record (all at the end of the
/// buffer): DefaultAnimation, DefaultScript, DefaultMotionTable,
/// DefaultSoundTable, DefaultScriptTable. All zero for the base human
/// setups — the live motion table id comes from the wire (ObjectCreate
/// MTable).
pub fn parse_setup_tail(buf: &[u8]) -> Option<[u32; 5]> {
    if buf.len() < 20 {
        return None;
    }
    let o = buf.len() - 20;
    let mut out = [0u32; 5];
    for i in 0..5 {
        out[i] = u32::from_le_bytes([buf[o + i * 4], buf[o + i * 4 + 1], buf[o + i * 4 + 2], buf[o + i * 4 + 3]]);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// portal dat: motion table (0x09)
// ---------------------------------------------------------------------------

/// One animation in a motion entry. `high_frame` -1 = play to the end of
/// the animation record; `framerate` negative = play in reverse.
#[derive(Clone, Copy, Debug)]
pub struct AnimRef {
    pub anim_id: u32,
    pub low_frame: i32,
    pub high_frame: i32,
    pub framerate: f32,
}

/// The animation data for one (stance, motion) cycle entry.
#[derive(Clone, Debug)]
pub struct MotionData {
    pub anims: Vec<AnimRef>,
    pub velocity: Option<[f32; 3]>,
    pub omega: Option<[f32; 3]>,
}

/// A MotionTable (portal 0x09xxxxxx): the (stance, motion) -> animation
/// map. Layout: u32 Id, u32 DefaultStyle, u32 nStyleDefaults +
/// nStyleDefaults x (u32 style, u32 defaultMotion), Cycles, Modifiers,
/// Links — each `u32 count + count x (u32 key + MotionData)`, and Links'
/// inner value is itself a Cycles-form. MotionData = u8 numAnims, u8
/// bitfield, u8 flags, align4, numAnims x AnimData (u32 animId, i32
/// lowFrame, i32 highFrame, f32 framerate), [flags&1: f32 x 3 velocity],
/// [flags&2: f32 x 3 omega]. Cycle key = (stance & 0xffff) << 16 |
/// (motion & 0xfffff) (matches the server's GetCycleLength / motionHash).
#[derive(Clone, Debug)]
pub struct MotionTable {
    pub default_style: u32,
    /// key -> MotionData, for the direct cycles.
    pub cycles: HashMap<u32, MotionData>,
    /// key -> (key -> MotionData): motion-mapping links (e.g. WalkBackwards
    /// resolves to the WalkForward anim set for reversed playback).
    pub links: HashMap<u32, HashMap<u32, MotionData>>,
}

fn parse_motion_data(r: &mut R) -> Option<MotionData> {
    let num_anims = r.u8()? as usize;
    r.u8()?; // bitfield (unused)
    let flags = r.u8()?;
    r.align4();
    let mut anims = Vec::with_capacity(num_anims);
    for _ in 0..num_anims {
        anims.push(AnimRef {
            anim_id: r.u32()?,
            low_frame: r.i32()?,
            high_frame: r.i32()?,
            framerate: r.f32()?,
        });
    }
    let mut velocity = None;
    let mut omega = None;
    if flags & 0x1 != 0 {
        velocity = r.vec3();
    }
    if flags & 0x2 != 0 {
        omega = r.vec3();
    }
    Some(MotionData {
        anims,
        velocity,
        omega,
    })
}

fn parse_motion_cycles(r: &mut R) -> Option<HashMap<u32, MotionData>> {
    let n = r.u32()? as usize;
    let mut m = HashMap::with_capacity(n);
    for _ in 0..n {
        let key = r.u32()?;
        m.insert(key, parse_motion_data(r)?);
    }
    Some(m)
}

pub fn parse_motion_table(buf: &[u8]) -> Option<MotionTable> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    let default_style = r.u32()?;
    let n_style_defaults = r.u32()?;
    for _ in 0..n_style_defaults {
        r.u32()?;
        r.u32()?;
    }
    let cycles = parse_motion_cycles(&mut r)?;
    parse_motion_cycles(&mut r)?; // Modifiers (server timing only — not needed for rendering)
    let mut links = HashMap::new();
    let n_links = r.u32()? as usize;
    for _ in 0..n_links {
        let key = r.u32()?;
        links.insert(key, parse_motion_cycles(&mut r)?);
    }
    Some(MotionTable {
        default_style,
        cycles,
        links,
    })
}

// ---------------------------------------------------------------------------
// portal dat: animation (0x03)
// ---------------------------------------------------------------------------

/// Parsed Animation record (portal 0x03xxxxxx).
/// Layout: u32 Id, u32 Flags (0x1 = has PosFrames), u32 numParts,
/// u32 numFrames, [Flags&1: numFrames x Frame(pos)], numFrames x
/// (numParts x Frame(part) + u32 numHooks + numHooks x Hook). Frame = 7
/// f32. Hooks carry attack/targeting effects (no mesh data) and are
/// skipped.
#[derive(Clone, Debug)]
pub struct Animation {
    pub num_parts: usize,
    pub num_frames: usize,
    /// numFrames x 7, or `None` when Flags&1 is clear.
    pub pos_frames: Option<Vec<f32>>,
    /// numFrames x numParts x 7 (row-major: frame, part, component).
    pub part_frames: Vec<f32>,
}

/// hook types and their extra byte counts (beyond the 8-byte u32 type +
/// i32 direction base). Type 5 (ReplaceObject) is special-cased.
fn hook_extra(type_: u32) -> Option<usize> {
    Some(match type_ {
        0 | 4 | 17 => 0,
        1 | 2 | 6 | 14 | 15 | 16 | 18 | 25 => 4,
        8 | 10 | 20 | 22 => 12,
        7 | 9 | 11 | 21 => 16,
        3 | 13 | 26 => 28,
        12 | 19 | 23 => 8,
        _ => return None, // unknown hook: bail loudly rather than desync the cursor
    })
}

fn skip_hook(r: &mut R) -> Option<()> {
    let type_ = r.u32()?;
    r.i32()?; // direction
    if type_ == 5 {
        r.u16()?; // partIndex
        let b0 = r.u16()?;
        if b0 & 0x8000 != 0 {
            r.u16()?; // packed dataid
        }
        return Some(());
    }
    let extra = hook_extra(type_)?;
    r.bytes(extra)
}

pub fn parse_animation(buf: &[u8]) -> Option<Animation> {
    let mut r = R::new(buf);
    r.u32()?; // Id
    let flags = r.u32()?;
    let num_parts = r.u32()? as usize;
    let num_frames = r.u32()? as usize;

    let mut pos_frames = None;
    if flags & 0x1 != 0 {
        let mut pf = Vec::with_capacity(num_frames * 7);
        for _ in 0..num_frames {
            for _ in 0..7 {
                pf.push(r.f32()?);
            }
        }
        pos_frames = Some(pf);
    }

    let mut part_frames = Vec::with_capacity(num_frames * num_parts * 7);
    for _ in 0..num_frames {
        for _ in 0..num_parts {
            for _ in 0..7 {
                part_frames.push(r.f32()?);
            }
        }
        let n_hooks = r.u32()?;
        for _ in 0..n_hooks {
            skip_hook(&mut r)?;
        }
    }

    Some(Animation {
        num_parts,
        num_frames,
        pos_frames,
        part_frames,
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_gfx_obj, parse_gfx_obj_physics, parse_texture_average};

    /// A DXT1 Texture record whose payload ends exactly on a block boundary:
    /// the 2-bit index decode must stay inside the final 8-byte block
    /// (regression: the old `k/2` byte stride read 3 bytes past the end and
    /// panicked on a 4096-byte payload — the crash killed world entry).
    #[test]
    fn dxt1_average_last_block_stays_in_bounds() {
        let len: usize = 4096; // 512 DXT1 blocks
        let mut buf = Vec::with_capacity(24 + len);
        for v in [1u32, 0, 128, 64, 827611204, len as u32] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend(std::iter::repeat(0xA5u8).take(len));
        let avg = parse_texture_average(&buf, 512, 4096).expect("dxt1 average");
        assert_eq!(avg >> 24, 0xff);
    }

    /// A GfxObj record: 3 identity-keyed vertices + one drawing/physics
    /// polygon with the given vertex ids (the third is out of range).
    fn gfx_obj_buf(flags: u32, ids: [i16; 3]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0x01000001u32.to_le_bytes()); // Id
        b.extend_from_slice(&flags.to_le_bytes());
        b.push(1u8); // c_u32 nSurfaces
        b.extend_from_slice(&0x08000001u32.to_le_bytes()); // Surface
        b.extend_from_slice(&1i32.to_le_bytes()); // VertexType 1
        b.extend_from_slice(&3u32.to_le_bytes()); // count
        for key in 0..3u16 {
            b.extend_from_slice(&key.to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes()); // numUVs
            for _ in 0..3 {
                b.extend_from_slice(&1.0f32.to_le_bytes()); // Origin
            }
            for _ in 0..3 {
                b.extend_from_slice(&0f32.to_le_bytes()); // Normal
            }
        }
        if flags & 0x1 != 0 {
            b.push(1u8); // c_u32 nPoly
            b.extend_from_slice(&0u16.to_le_bytes()); // poly key
            gfx_polygon(&mut b, ids);
            // Physics BSP: one solid leaf.
            b.extend_from_slice(&0x4c454146u32.to_le_bytes()); // T_LEAF
            b.extend_from_slice(&0i32.to_le_bytes()); // LeafIndex
            b.extend_from_slice(&1i32.to_le_bytes()); // Solid
            b.extend_from_slice(&[0u8; 16]); // Sphere
            b.extend_from_slice(&0u32.to_le_bytes()); // polygon count
        } else {
            b.extend_from_slice(&[0u8; 12]); // SortCenter
            b.push(1u8); // c_u32 nPoly
            b.extend_from_slice(&0u16.to_le_bytes()); // poly key
            gfx_polygon(&mut b, ids);
        }
        b
    }

    fn gfx_polygon(b: &mut Vec<u8>, ids: [i16; 3]) {
        b.push(3u8); // num_pts
        b.push(0u8); // stippling (no NOPOS/NOINEG)
        b.extend_from_slice(&1i32.to_le_bytes()); // CullMode None
        b.extend_from_slice(&0i16.to_le_bytes()); // pos_surface
        b.extend_from_slice(&0i16.to_le_bytes()); // neg_surface
        for &i in &ids {
            b.extend_from_slice(&i.to_le_bytes());
        }
        b.extend_from_slice(&[0u8; 3]); // positive stipple/UV indices
    }

    /// A polygon key beyond the vertex array must be dropped, not index
    /// past the positions (regression: a corrupt DAT with identity slot
    /// keys panicked `vdata.pos[s * 3]` — the parsers' documented contract
    /// is to return `None`/skip, mirroring the TS client's caught throw).
    #[test]
    fn gfx_obj_physics_out_of_range_vertex_key() {
        let buf = gfx_obj_buf(0x1, [0, 1, 7]);
        let phys = parse_gfx_obj_physics(&buf, None).expect("physics parses");
        assert!(phys.tris.is_empty(), "polygon must be dropped");
        assert!(phys.solid, "solid flag from the physics BSP leaf");
    }

    /// Mesh triangle indices must stay inside the vertex array (the
    /// renderer indexes `pos[i * 3]` with them — an out-of-range key
    /// panicked the render pass on a corrupt DAT).
    #[test]
    fn gfx_obj_mesh_vertex_indices_stay_in_range() {
        let buf = gfx_obj_buf(0x2, [0, 1, 7]);
        let mesh = parse_gfx_obj(&buf, |_| None, None).expect("mesh");
        let n = mesh.pos.len() / 3;
        for &i in &mesh.tris {
            assert!((i as usize) < n, "tri index {i} beyond {n} vertices");
        }
    }
}
