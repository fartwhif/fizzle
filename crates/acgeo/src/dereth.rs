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
}
