//! Regression: the 3D renderer produces shaded (non-uniform) output for the
//! spawn landblock when retail DATs are available (skips otherwise).
use acgeo::{Camera, World3D, WorldProbe};

#[test]
fn render3d_shading() {
    let dat = std::env::var("AC_DAT_DIR").ok();
    let mut w = World3D::new(dat.as_deref());
    if !w.available() {
        eprintln!("SKIP: no retail dats");
        return;
    }
    // +blah spawns ~12.3E, 28.5S => block-local (12.3, 163.5), landblock 0.
    let (cx, cy) = (12.3f32, 163.5f32);
    w.set_landblock(0);
    let ground = w.height_at(cx, cy).unwrap_or(0.0);
    let cam = Camera { x: cx, y: cy, z: ground + 1.7, heading: 0.0 };
    let frame = w
        .render(0, &cam, &[], 60, 12, false, None, false)
        .expect("render returns Some with dats + terrain");
    let distinct = frame.cells.iter().cloned().collect::<std::collections::HashSet<_>>();
    let mut rows_uniform = 0;
    for r in 0..frame.rows {
        let row = &frame.cells[r * frame.cols..(r + 1) * frame.cols];
        if row.iter().all(|&c| c == row[0]) {
            rows_uniform += 1;
        }
    }
    println!(
        "ground={ground} cells={} distinct_rgb={} uniform_rows={rows_uniform}/{}",
        frame.cells.len(),
        distinct.len(),
        frame.rows
    );
    println!("sample={:?}", &frame.cells[..6.min(frame.cells.len())]);
    let n = distinct.len();
    assert!(
        n > 8,
        "3D frame should show shading variation, got {n}"
    );
}
