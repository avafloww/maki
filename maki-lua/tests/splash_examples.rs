use std::collections::HashSet;
use std::time::Duration;

use maki_lua::test_support::spawn_host_for_tests;
use maki_lua::{EventHandle, SplashFrame, SplashStyle};

const W: usize = 80;
const H: usize = 20;
const PAPER: (u8, u8, u8) = (0x3a, 0x3c, 0x4e);
const WHITE: (u8, u8, u8) = (255, 255, 255);

fn example_source(name: &str) -> &'static str {
    match name {
        "pentagram" => include_str!("../../examples/splash/pentagram.lua"),
        "flowers" => include_str!("../../examples/splash/flowers.lua"),
        "printer" => include_str!("../../examples/splash/printer.lua"),
        "tunnel" => include_str!("../../examples/splash/tunnel.lua"),
        "comets" => include_str!("../../examples/splash/comets.lua"),
        "wavebanner" => include_str!("../../examples/splash/wavebanner.lua"),
        _ => panic!("unknown example {name}"),
    }
}

fn host(name: &str) -> (EventHandle, maki_lua::test_support::PluginHostGuard) {
    let (handle, guard) = spawn_host_for_tests(&["splash"]);
    guard
        .host()
        .load_source(&format!("{name}_example"), example_source(name))
        .expect("example loads");
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
    style: SplashStyle,
}

/// Reconstruct the per-cell grid from the flattened frame rows. Every example
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

fn check_frame(frame: &SplashFrame) {
    assert!(!frame.rows.is_empty(), "frame must not be empty");
    let grid = reconstruct(frame);
    assert_eq!(grid.len(), H, "exactly {H} rows");
    for row in &grid {
        assert_eq!(row.len(), W, "each row must be {W} cells");
    }
}

fn cells(frame: &SplashFrame, ch: char) -> Vec<(usize, usize)> {
    let grid = reconstruct(frame);
    let mut out = Vec::new();
    for (y, row) in grid.iter().enumerate() {
        for (x, c) in row.iter().enumerate() {
            if c.ch == ch {
                out.push((y, x));
            }
        }
    }
    out
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

fn is_fg(style: &SplashStyle, want: (u8, u8, u8)) -> bool {
    matches!(style, SplashStyle::Rgba { fg, .. } if *fg == want)
}

#[test_case::test_case("pentagram" ; "pentagram_loads")]
#[test_case::test_case("flowers" ; "flowers_loads")]
#[test_case::test_case("printer" ; "printer_loads")]
#[test_case::test_case("tunnel" ; "tunnel_loads")]
#[test_case::test_case("comets" ; "comets_loads")]
#[test_case::test_case("wavebanner" ; "wavebanner_loads")]
fn example_loads_and_renders(name: &str) {
    let (handle, _guard) = host(name);
    let frame = pull(&handle, 1.0);
    check_frame(&frame);
}

#[test]
fn pentagram_rotates_periodically() {
    let (handle, _guard) = host("pentagram");
    let t0 = filled(&pull(&handle, 0.0));
    let t5 = filled(&pull(&handle, 5.0));
    let t06 = filled(&pull(&handle, 0.6));
    let overlap = |a: &HashSet<(usize, usize)>, b: &HashSet<(usize, usize)>| {
        a.iter().filter(|c| b.contains(c)).count() as f64 / a.len() as f64
    };
    assert!(
        overlap(&t0, &t5) >= 0.95,
        "5 s period: {:.3}",
        overlap(&t0, &t5)
    );
    assert!(
        overlap(&t0, &t06) < 0.9,
        "star visibly rotates: {:.3}",
        overlap(&t0, &t06)
    );
}

#[test]
fn pentagram_leading_vertex_advances() {
    let (handle, _guard) = host("pentagram");
    let bright = |t: f32| {
        reconstruct(&pull(&handle, t))
            .iter()
            .enumerate()
            .flat_map(|(y, row)| {
                row.iter()
                    .enumerate()
                    .filter(|(_, c)| is_fg(&c.style, WHITE))
                    .map(move |(x, _)| (y, x))
            })
            .collect::<Vec<_>>()
    };
    let at0 = bright(0.0);
    let at1 = bright(1.0);
    assert_eq!(at0.len(), 1, "exactly one bright leading vertex");
    let (y0, x0) = at0[0];
    let (y1, x1) = at1[0];
    assert!((y0, x0) != (y1, x1), "leading vertex moved after a second");
}

#[test]
fn flowers_rise_over_time() {
    let (handle, _guard) = host("flowers");
    let blooms = |t: f32| {
        let rows = cells(&pull(&handle, t), 'o')
            .into_iter()
            .map(|(y, _)| y)
            .collect::<Vec<_>>();
        (*rows.iter().min().expect("at least one bloom"), rows.len())
    };
    let (min0, n0) = blooms(0.0);
    let (min1, n1) = blooms(2.0);
    assert_eq!(n0, n1, "same flower count");
    assert!(n0 > 0, "some blooms visible");
    assert!(min1 < min0, "blooms rise: {min1} < {min0}");
}

#[test]
fn printer_pages_grow_and_reset() {
    let (handle, _guard) = host("printer");
    let bottom = |t: f32| {
        reconstruct(&pull(&handle, t))
            .iter()
            .enumerate()
            .flat_map(|(y, row)| {
                row.iter()
                    .filter(|c| is_fg(&c.style, PAPER))
                    .map(move |_| y)
            })
            .max()
            .unwrap_or(0)
    };
    let b0 = bottom(0.5);
    let b1 = bottom(1.5);
    let b2 = bottom(6.0);
    assert!(b1 > b0, "sheet grows on one page: {b1} > {b0}");
    assert!(b2 < b1, "fresh page short: {b2} < {b1}");
}

#[test]
fn tunnel_moves_and_is_denser_at_center() {
    let (handle, _guard) = host("tunnel");
    let grid0 = reconstruct(&pull(&handle, 0.0));
    let grid1 = reconstruct(&pull(&handle, 0.5));
    let diff = grid0
        .iter()
        .flatten()
        .zip(grid1.iter().flatten())
        .filter(|(a, b)| a.ch != b.ch)
        .count();
    assert!(diff > 20, "tunnel animates: {diff} cells changed");

    let cx = W / 2;
    let cy = H / 2;
    let m_max = (W.max(H) / 2) as f64;
    let density = |lo: f64, hi: f64| {
        let mut on = 0f64;
        let mut total = 0f64;
        for (y, row) in grid0.iter().enumerate() {
            for (x, c) in row.iter().enumerate() {
                let m = (x as isize - cx as isize)
                    .abs()
                    .max((y as isize - cy as isize).abs());
                let norm = m as f64 / m_max;
                if !(lo..=hi).contains(&norm) {
                    continue;
                }
                total += 1.0;
                if c.ch != ' ' {
                    on += 1.0;
                }
            }
        }
        on / total
    };
    let center = density(0.0, 0.4);
    let edge = density(0.6, 1.0);
    assert!(center >= edge, "center denser: {center:.2} >= {edge:.2}");
}

#[test]
fn comets_heads_advance() {
    let (handle, _guard) = host("comets");
    let heads = |t: f32| {
        let mut v = cells(&pull(&handle, t), '*');
        v.sort_by_key(|(y, x)| (*y, *x));
        v
    };
    let at0 = heads(0.0);
    let at1 = heads(0.5);
    assert!(!at0.is_empty(), "comet heads visible");
    assert_eq!(at0.len(), at1.len(), "same comet count");
    for (a, b) in at0.iter().zip(at1.iter()) {
        assert!(
            b.1 > a.1,
            "head advanced right: ({},{}) -> ({},{})",
            a.0,
            a.1,
            b.0,
            b.1
        );
    }
}

#[test]
fn wavebanner_letters_present_and_bob() {
    let (handle, _guard) = host("wavebanner");
    let positions = |t: f32| {
        let grid = reconstruct(&pull(&handle, t));
        grid.iter()
            .enumerate()
            .flat_map(|(y, row)| {
                row.iter()
                    .enumerate()
                    .filter(|(_, c)| "WAVE".contains(c.ch))
                    .map(move |(x, _)| (y, x))
            })
            .collect::<HashSet<_>>()
    };
    let text = positions(0.0);
    for ch in "WAVE".chars() {
        assert!(
            reconstruct(&pull(&handle, 0.0))
                .iter()
                .flatten()
                .any(|c| c.ch == ch),
            "letter '{ch}' present"
        );
    }
    assert!(!text.is_empty());
    assert!(text != positions(1.0), "letters bob as t changes");
}
