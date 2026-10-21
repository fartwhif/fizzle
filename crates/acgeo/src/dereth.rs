//! The Dereth continent landmass for the Map widget (ported from
//! `acclient-ts/src/dereth.ts`).
//!
//! A landblock is land iff its CellLandblock (cell dat, id =
//! `landblock | 0xFFFF`) has any height byte > 0 — open water is a flat
//! height-0 landblock. The full 255x255 mask is derived from the retail cell
//! dat once (lazily, ~0.5 s) and cached; the Map widget renders it as a
//! cartographic continent with the player's landblock as a single point.

use std::sync::OnceLock;

use crate::acegeom::parse_cell_landblock;
use crate::dats::{self, DatDatabase};
use crate::DerethLand;

/// Per-process cache: `(resolved dir, mask)`. The client resolves one dat dir
/// for its whole session; a later call with a different resolved dir
/// recomputes (the first entry stays cached).
static CACHE: OnceLock<(String, Option<DerethLand>)> = OnceLock::new();

/// Compute (and cache) the continent landmask from the retail cell dat.
/// Returns `None` when the dats are unavailable. The first call reads every
/// CellLandblock record (~0.5 s); later calls are instant.
#[must_use]
pub fn get_dereth_land(explicit_dir: Option<&str>) -> Option<DerethLand> {
    let dir = dats::find_dat_dir(explicit_dir)?;
    if let Some((cached, land)) = CACHE.get() {
        if *cached == dir {
            return land.clone();
        }
    }
    let land = scan(&dir);
    let _ = CACHE.set((dir, land.clone()));
    land
}

/// Scan the cell dat for land landblocks (the CellLandblock records).
fn scan(dir: &str) -> Option<DerethLand> {
    let mut d = DatDatabase::open(format!("{dir}/client_cell_1.dat")).ok()?;
    let mut land = vec![0u8; 255 * 255];
    let mut min_x = 255u32;
    let mut max_x = 0u32;
    let mut min_y = 255u32;
    let mut max_y = 0u32;
    let mut found = false;
    let ids: Vec<u32> = d.ids().to_vec();
    for &id in &ids {
        if id & 0xffff != 0xffff {
            continue;
        }
        let lb = (id >> 16) & 0xffff;
        let x = (lb >> 8) & 0xff;
        let y = lb & 0xff;
        // The id space allows x/y = 255; the mask is the 255x255 Dereth
        // grid, so an out-of-grid record is dropped (it would index past
        // the end of `land` — or diagonally into the next row for x = 255).
        if x >= 255 || y >= 255 {
            continue;
        }
        let Some(rec) = d.read(id) else {
            continue;
        };
        let Some(data) = parse_cell_landblock(&rec) else {
            continue;
        };
        if data.height.iter().any(|&h| h > 0) {
            land[y as usize * 255 + x as usize] = 1;
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            found = true;
        }
    }
    if !found {
        return None;
    }
    Some(DerethLand {
        min_x,
        max_x,
        min_y,
        max_y,
        land,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landmask_shape_and_bounds() {
        if dats::find_dat_dir(None).is_none() {
            eprintln!("skipped: retail dats unavailable");
            return;
        }
        let Some(land) = get_dereth_land(None) else {
            eprintln!("skipped: no landmask in the dats");
            return;
        };
        // The mask is the full 255x255 grid, indexed y*255+x, flags 0/1.
        assert_eq!(land.land.len(), 255 * 255);
        assert!(land.land.iter().all(|&f| f <= 1));
        // The bounding box is ordered, inside the grid, and tight: each
        // edge carries at least one land landblock.
        assert!(land.min_x <= land.max_x && land.min_y <= land.max_y);
        assert!(land.max_x < 255 && land.max_y < 255);
        let at = |x: u32, y: u32| land.land[(y * 255 + x) as usize];
        let west = (land.min_y..=land.max_y).any(|y| at(land.min_x, y) == 1);
        let east = (land.min_y..=land.max_y).any(|y| at(land.max_x, y) == 1);
        let south = (land.min_x..=land.max_x).any(|x| at(x, land.min_y) == 1);
        let north = (land.min_x..=land.max_x).any(|x| at(x, land.max_y) == 1);
        assert!(west && east && south && north, "bbox must be tight");
        // Dereth is a continent, not the whole grid or a sliver.
        let total = land.land.iter().filter(|&&f| f == 1).count();
        assert!(total > 100 && total < 255 * 255);
    }

    /// A CellLandblock record (id, flat terrain, all heights `height`).
    fn cell_landblock(id: u32, height: u8) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // HasObjects
        for _ in 0..81 {
            b.extend_from_slice(&0u16.to_le_bytes()); // terrain
        }
        b.extend(std::iter::repeat_n(height, 81));
        b
    }

    /// A single-directory-node cell DAT (header @ 0x140, the node in the
    /// 0x400 sector, one 0x800 sector per record).
    fn minimal_cell_dat(records: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut f = vec![0u8; 0x1400 + records.len() * 0x800];
        let u32 = |v: u32| v.to_le_bytes();
        f[0x140..0x144].copy_from_slice(&u32(0x5442)); // FileType "BT"
        f[0x144..0x148].copy_from_slice(&u32(4096)); // BlockSize
        f[0x148..0x14c].copy_from_slice(&u32((0x1400 + records.len() * 0x800) as u32)); // FileSize
        f[0x14c..0x150].copy_from_slice(&u32(2)); // DataSet = cell
        f[0x160..0x164].copy_from_slice(&u32(0x400)); // BTree
        let node = 0x400 + 4;
        f[node + 62 * 4..node + 62 * 4 + 4].copy_from_slice(&u32(records.len() as u32));
        for (i, (id, rec)) in records.iter().enumerate() {
            let e = node + 252 + i * 24;
            f[e..e + 4].copy_from_slice(&u32(1)); // BitFlags
            f[e + 4..e + 8].copy_from_slice(&u32(*id));
            f[e + 8..e + 12].copy_from_slice(&u32(0x1400 + i as u32 * 0x800));
            f[e + 12..e + 16].copy_from_slice(&u32(rec.len() as u32));
            f[e + 20..e + 24].copy_from_slice(&u32(1)); // Iteration
            let s = 0x1400 + i * 0x800;
            f[s + 4..s + 4 + rec.len()].copy_from_slice(rec);
        }
        f
    }

    /// The cell dat's id space allows landblocks with x or y = 255
    /// (`lb = y << 8 | x`), but the mask is the 255x255 Dereth grid: such
    /// records must be skipped, not written past (or diagonally into) the
    /// array (regression: landblock 0xFFFF indexed `y * 255 + x` = 65535
    /// into a 65025-entry mask and panicked the Map widget's scan).
    #[test]
    fn cell_landblock_outside_grid_is_ignored() {
        let dir = std::env::temp_dir().join(format!("acgeo-dereth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("client_portal.dat"), []).unwrap();
        // Cell dat id = landblock | 0xFFFF, landblock = (x << 8) | y.
        let records = [
            // In-grid land (x=1, y=0 -> lb 0x0100) — the only cell that may be set.
            (0x0100ffffu32, cell_landblock(0x0100ffff, 1)),
            // Out of grid (x=255, y=0 -> lb 0xFF00) — used to leak into cell (0, 1).
            (0xff00ffffu32, cell_landblock(0xff00ffff, 1)),
            // Out of grid (x=255, y=255 -> lb 0xFFFF) — panicked the mask indexing.
            (0xffff_ffffu32, cell_landblock(0xffff_ffff, 1)),
        ];
        std::fs::write(
            dir.join("client_cell_1.dat"),
            minimal_cell_dat(&records),
        )
        .unwrap();

        let land = get_dereth_land(Some(dir.to_str().unwrap())).expect("in-grid record yields a mask");
        // Only the in-grid landblock (1, 0) is set; the out-of-grid
        // records left no trace (neither a panic nor a stray cell).
        assert_eq!(land.land.iter().filter(|&&f| f == 1).count(), 1);
        assert_eq!(land.land[1], 1); // cell (x=1, y=0)
        assert_eq!(land.min_x, 1);
        assert_eq!(land.max_x, 1);
        assert_eq!(land.min_y, 0);
        assert_eq!(land.max_y, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
