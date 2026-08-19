use std::collections::HashSet;
use std::time::Duration;

use maki_lua::test_support::spawn_host_for_tests;
use maki_lua::{EventHandle, SplashFrame, SplashStyle};

const W: usize = 80;
const H: usize = 20;
const SKINS: &[&str] = &[
    "kaleidoscope",
    "voronoi",
    "caustics",
    "metaballs",
    "aurora",
    "matrix",
];

/// The full-field shader ports paint most cells; matrix is a sparse rain.
fn min_filled(skin: &str) -> usize {
    match skin {
        "matrix" => 300,
        _ => 500,
    }
}

/// Stateful skins (matrix) start with heads above the screen and need
/// simulated seconds before the rain is flowing. Pure skins are unaffected
/// by extra pulls, so drive every skin the same way.
fn drive(handle: &EventHandle, to: f32) {
    let mut t = 0.5;
    while t <= to {
        pull(handle, t);
        t += 0.5;
    }
}

fn host(skin: &str) -> (EventHandle, maki_lua::test_support::PluginHostGuard) {
    let (handle, guard) = spawn_host_for_tests(&["splash"]);
    guard
        .host()
        .load_source(
            &format!("{skin}_gallery"),
            &format!("require(\"splash.{skin}\")"),
        )
        .expect("gallery skin loads through the bundled module path");
    (handle, guard)
}

fn pull(handle: &EventHandle, t: f32) -> SplashFrame {
    loop {
        if let Some(f) = handle.splash_frame(W as u16, H as u16, t, 1.0) {
            return f;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Clone)]
struct Cell {
    ch: char,
    #[allow(dead_code)]
    style: SplashStyle,
}

/// Reconstruct the per-cell grid from the flattened frame rows. Every skin
/// must paint exactly `H` rows of `W` cells.
fn reconstruct(frame: &SplashFrame) -> Vec<Vec<Cell>> {
    let mut grid = vec![
        vec![
            Cell {
                ch: ' ',
                style: SplashStyle::Field
            };
            W
        ];
        H
    ];
    let (mut y, mut x) = (0usize, 0usize);
    for seg in &frame.rows {
        for ch in seg.glyphs.chars() {
            if x == W {
                x = 0;
                y += 1;
                if y == H {
                    break;
                }
            }
            grid[y][x] = Cell {
                ch,
                style: seg.style.clone(),
            };
            x += 1;
        }
    }
    grid
}

fn check_frame(skin: &str, frame: &SplashFrame) {
    assert!(!frame.rows.is_empty(), "{skin}: frame must not be empty");
    let grid = reconstruct(frame);
    assert_eq!(grid.len(), H, "{skin}: exactly {H} rows");
    for row in &grid {
        assert_eq!(row.len(), W, "{skin}: each row must be {W} cells");
    }
}

fn filled(frame: &SplashFrame) -> HashSet<(usize, usize)> {
    let grid = reconstruct(frame);
    grid.iter()
        .enumerate()
        .flat_map(|(y, row)| {
            row.iter()
                .enumerate()
                .filter(|(_, c)| c.ch != ' ')
                .map(move |(x, _)| (y, x))
        })
        .collect()
}

#[test_case::test_case("kaleidoscope" ; "kaleidoscope_loads")]
#[test_case::test_case("voronoi" ; "voronoi_loads")]
#[test_case::test_case("caustics" ; "caustics_loads")]
#[test_case::test_case("metaballs" ; "metaballs_loads")]
#[test_case::test_case("aurora" ; "aurora_loads")]
#[test_case::test_case("matrix" ; "matrix_loads")]
fn skin_loads_and_renders(skin: &str) {
    let (handle, _guard) = host(skin);
    drive(&handle, 20.0);
    let frame = pull(&handle, 20.5);
    check_frame(skin, &frame);
    let threshold = min_filled(skin);
    assert!(
        filled(&frame).len() > threshold,
        "{skin}: expected > {threshold} filled cells, got {}",
        filled(&frame).len()
    );
}

#[test_case::test_case("kaleidoscope" ; "kaleidoscope_animates")]
#[test_case::test_case("voronoi" ; "voronoi_animates")]
#[test_case::test_case("caustics" ; "caustics_animates")]
#[test_case::test_case("metaballs" ; "metaballs_animates")]
#[test_case::test_case("aurora" ; "aurora_animates")]
#[test_case::test_case("matrix" ; "matrix_animates")]
fn skin_animates(skin: &str) {
    let (handle, _guard) = host(skin);
    drive(&handle, 20.0);
    let a = reconstruct(&pull(&handle, 20.3));
    let b = reconstruct(&pull(&handle, 20.9));
    let diff = a
        .iter()
        .flatten()
        .zip(b.iter().flatten())
        .filter(|(x, y)| x.ch != y.ch)
        .count();
    assert!(
        diff > 100,
        "{skin}: frame visibly animates: {diff} cells changed"
    );
}

#[test]
fn every_gallery_skin_is_reachable() {
    for skin in SKINS {
        let (_handle, guard) = spawn_host_for_tests(&["splash"]);
        guard
            .host()
            .load_source(
                &format!("{skin}_reach"),
                &format!("assert(require(\"splash.{skin}\").render)"),
            )
            .unwrap_or_else(|e| panic!("splash.{skin} must be requireable: {e}"));
    }
}
