use std::collections::HashSet;
use std::time::{Duration, Instant};

use maki_lua::test_support::{spawn_host_for_tests, spawn_host_for_tests_with_state};
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
/// Matrix steady state averages ~345 filled cells with ~24 sigma of spawn
/// variance; 200 is far below that and far above the label-only frame (~15)
/// a broken rain would produce.
fn min_filled(skin: &str) -> usize {
    match skin {
        "matrix" => 200,
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
            &format!(
                r#"
                local module = require("splash.{skin}")
                assert(type(module.render) == "function")
                maki.api.set_slot("splash.render", function(_, w, h, t, fade)
                  return module.render(w, h, t, fade)
                end)
                "#
            ),
        )
        .expect("gallery renderer loads through the bundled module path");
    (handle, guard)
}

fn pull_size(handle: &EventHandle, width: u16, height: u16, t: f32) -> SplashFrame {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(frame) = handle.splash_frame(width, height, t, 1.0) {
            return frame;
        }
        assert!(Instant::now() < deadline, "splash renderer timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn pull(handle: &EventHandle, t: f32) -> SplashFrame {
    pull_size(handle, W as u16, H as u16, t)
}

fn pull_glyph(handle: &EventHandle, t: f32, glyph: char) -> SplashFrame {
    for _ in 0..20 {
        let frame = pull(handle, t);
        if reconstruct(&frame)
            .iter()
            .flatten()
            .all(|cell| cell.ch == glyph)
        {
            return frame;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("splash never rendered '{glyph}'")
}

#[derive(Clone, PartialEq, Eq)]
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
    let glyphs = frame
        .rows
        .iter()
        .map(|segment| segment.glyphs.as_str())
        .collect::<String>();
    assert_eq!(
        glyphs.chars().count(),
        W * H,
        "{skin}: frame must cover the canvas"
    );
    assert!(
        glyphs.contains(skin),
        "{skin}: frame must contain its skin label"
    );
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
    drive(&handle, 30.0);
    let mut frame = pull(&handle, 30.2);
    let mut filled_max = 0usize;
    for i in 0..5 {
        frame = pull(&handle, 30.2 + i as f32 * 0.4);
        filled_max = filled_max.max(filled(&frame).len());
    }
    check_frame(skin, &frame);
    let threshold = min_filled(skin);
    assert!(
        filled_max > threshold,
        "{skin}: expected > {threshold} filled cells, best of 5 frames was {filled_max}"
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
    let frame_a = pull(&handle, 20.3);
    let frame_b = pull(&handle, 20.9);
    check_frame(skin, &frame_a);
    check_frame(skin, &frame_b);
    let a = reconstruct(&frame_a);
    let b = reconstruct(&frame_b);
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
fn caustics_does_not_replicate_four_by_four_samples() {
    const BLOCK_SIZE: usize = 4;

    let (handle, _guard) = host("caustics");
    let grid = reconstruct(&pull(&handle, 8.3));
    let mut blocks = 0;
    let mut replicated = 0;
    for rows in grid.chunks_exact(BLOCK_SIZE) {
        for x in (0..W).step_by(BLOCK_SIZE) {
            blocks += 1;
            let first = &rows[0][x];
            if rows
                .iter()
                .all(|row| row[x..x + BLOCK_SIZE].iter().all(|cell| cell == first))
            {
                replicated += 1;
            }
        }
    }
    assert!(
        replicated * 4 < blocks,
        "caustics replicated {replicated}/{blocks} four-by-four sample blocks"
    );
}

#[test]
fn voronoi_renders_at_188x53() {
    const LARGE_W: u16 = 188;
    const LARGE_H: u16 = 53;

    let (handle, _guard) = host("voronoi");
    let frame = pull_size(&handle, LARGE_W, LARGE_H, 1.0);
    let glyphs = frame
        .rows
        .iter()
        .map(|segment| segment.glyphs.as_str())
        .collect::<String>();
    assert_eq!(glyphs.chars().count(), LARGE_W as usize * LARGE_H as usize);
    assert!(glyphs.contains("voronoi"));
}

#[test]
#[ignore = "reports local splash render timing; run with --ignored --nocapture"]
fn shader_port_timing_at_188x53() {
    const BENCH_W: u16 = 188;
    const BENCH_H: u16 = 53;
    const WARMUP_FRAMES: usize = 10;
    const SAMPLES: usize = 30;

    for skin in ["caustics", "voronoi"] {
        let (handle, _guard) = host(skin);
        for frame in 0..WARMUP_FRAMES {
            handle
                .splash_frame(BENCH_W, BENCH_H, frame as f32 / 30.0, 1.0)
                .expect("splash renderer must produce a frame during warmup");
        }
        let started = Instant::now();
        for frame in 0..SAMPLES {
            let splash = handle
                .splash_frame(BENCH_W, BENCH_H, (WARMUP_FRAMES + frame) as f32 / 30.0, 1.0)
                .expect("splash renderer must produce a frame while timing");
            let cells = splash
                .rows
                .iter()
                .map(|segment| segment.glyphs.chars().count())
                .sum::<usize>();
            assert_eq!(cells, BENCH_W as usize * BENCH_H as usize);
        }
        let milliseconds = started.elapsed().as_secs_f64() * 1_000.0 / SAMPLES as f64;
        eprintln!("{skin} {BENCH_W}x{BENCH_H}: {milliseconds:.2} ms/frame");
    }
}

#[test]
fn gallery_modules_are_reachable_without_self_activation() {
    let (handle, guard) = spawn_host_for_tests(&["splash"]);
    let skins = SKINS
        .iter()
        .map(|skin| format!("\"{skin}\""))
        .collect::<Vec<_>>()
        .join(", ");
    guard
        .host()
        .load_source(
            "gallery_module_contract",
            &format!(
                r##"
                maki.api.set_slot("splash.render", function(_, w, h)
                  local rows = {{}}
                  for y = 1, h do rows[y] = {{ {{ glyphs = string.rep("X", w), style = "#ffffff" }} }} end
                  return rows
                end)
                for _, skin in ipairs({{{skins}}}) do
                  local module = require("splash." .. skin)
                  assert(type(module) == "table" and type(module.render) == "function")
                end
                "##
            ),
        )
        .expect("gallery modules are requireable");

    assert!(
        reconstruct(&pull(&handle, 1.0))
            .iter()
            .flatten()
            .all(|cell| cell.ch == 'X'),
        "requiring gallery modules must not replace the active splash layer"
    );
}

#[test]
fn matrix_resets_when_splash_is_shown_again() {
    let (handle, guard) = host("matrix");
    drive(&handle, 30.0);
    let before = (0..5)
        .map(|i| filled(&pull(&handle, 30.2 + i as f32 * 0.4)).len())
        .max()
        .unwrap();
    handle.fire_autocmd("SplashShown", serde_json::Value::Null);
    guard
        .host()
        .load_source("splash_shown_barrier", "return nil")
        .expect("SplashShown callback completes");
    let after = filled(&pull(&handle, 0.0)).len();
    assert!(before > 200, "matrix must reach steady-state rain");
    assert!(after < 50, "SplashShown must reset matrix state");
}

#[test]
fn renderer_selection_and_runtime_rollback_are_transactional() {
    let (handle, guard) = spawn_host_for_tests(&["splash", "splash_gallery"]);
    guard
        .host()
        .load_source(
            "gallery_transaction_fixtures",
            r##"
            local function filled(glyph)
              return function(w, h)
                local rows = {}
                for y = 1, h do
                  rows[y] = { { glyphs = string.rep(glyph, w), style = "#ffffff" } }
                end
                return rows
              end
            end

            maki.api.register("splash.gallery", "stable", {
              label = "Stable",
              activate = function() return filled("S") end,
            })
            maki.api.register("splash.gallery", "broken", {
              label = "Broken",
              activate = function() return function() error("candidate failed") end end,
            })
            maki.api.register("splash.gallery", "fragile", {
              label = "Fragile",
              activate = function()
                local render = filled("F")
                local calls = 0
                return function(...)
                  calls = calls + 1
                  if calls > 2 then error("committed renderer failed") end
                  return render(...)
                end
              end,
            })
            "##,
        )
        .expect("transaction fixtures register");

    handle
        .run_command_for_test(
            "splash_gallery".into(),
            "/splash".into(),
            "stable".into(),
            0,
        )
        .recv_timeout(Duration::from_secs(5))
        .expect("stable command completes");
    let _stable = pull_glyph(&handle, 1.0, 'S');
    guard
        .host()
        .load_source(
            "stable_persistence",
            r##"
            local path = maki.fs.joinpath(maki.env.state_dir(), "splash_gallery", "selection.json")
            assert(maki.json.decode(maki.fs.read(path)).name == "stable")
            "##,
        )
        .expect("stable selection persists");

    handle
        .run_command_for_test(
            "splash_gallery".into(),
            "/splash".into(),
            "broken".into(),
            0,
        )
        .recv_timeout(Duration::from_secs(5))
        .expect("broken command completes");
    let _after_broken = pull_glyph(&handle, 2.0, 'S');
    guard
        .host()
        .load_source(
            "broken_persistence",
            r##"
            local path = maki.fs.joinpath(maki.env.state_dir(), "splash_gallery", "selection.json")
            assert(maki.json.decode(maki.fs.read(path)).name == "stable")
            "##,
        )
        .expect("invalid selection does not persist");
    handle
        .run_command_for_test(
            "splash_gallery".into(),
            "/splash".into(),
            "fragile".into(),
            0,
        )
        .recv_timeout(Duration::from_secs(5))
        .expect("fragile command completes");
    let _fragile = pull_glyph(&handle, 2.5, 'F');
    let _rolled_back = pull_glyph(&handle, 4.0, 'S');
    guard
        .host()
        .load_source("rollback_barrier", "return nil")
        .expect("queued rollback persists");
    guard
        .host()
        .load_source(
            "rollback_persistence",
            r##"
            local path = maki.fs.joinpath(maki.env.state_dir(), "splash_gallery", "selection.json")
            assert(maki.json.decode(maki.fs.read(path)).name == "stable")
            "##,
        )
        .expect("runtime rollback repairs persisted selection");
}

#[test]
fn completion_lifecycle_previews_commits_and_cancels() {
    let (handle, guard) = spawn_host_for_tests(&["splash", "splash_gallery"]);
    guard
        .host()
        .load_source(
            "completion_gallery_fixture",
            r##"
            local function renderer(glyph)
              return function(w, h)
                local rows = {}
                for y = 1, h do rows[y] = { { glyphs = string.rep(glyph, w), style = "#ffffff" } } end
                return rows
              end
            end
            maki.api.register("splash.gallery", "preview", {
              label = "Preview",
              activate = function() return renderer("P") end,
            })
            "##,
        )
        .unwrap();
    let context = maki_lua::CommandArgumentContext {
        command: "/splash".into(),
        plugin: "splash_gallery".into(),
        args: "pre".into(),
        arg: "pre".into(),
        index: 0,
        mode: "build".into(),
        session: 1,
        generation: 1,
    };
    let items = handle
        .collect_command_argument_items(context.clone(), maki_agent::CancelToken::none())
        .unwrap()
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let item = items
        .into_iter()
        .find(|item| item.insertion == "preview")
        .unwrap();

    handle.command_argument_lifecycle(
        context.clone(),
        maki_lua::CommandArgumentLifecycle::Highlight,
        Some(item.clone()),
        maki_agent::CancelToken::none(),
    );
    let _preview = pull_glyph(&handle, 1.0, 'P');
    handle.command_argument_lifecycle(
        context.clone(),
        maki_lua::CommandArgumentLifecycle::Cancel,
        None,
        maki_agent::CancelToken::none(),
    );
    guard
        .host()
        .load_source("completion_cancel_barrier", "return nil")
        .unwrap();
    let cancelled = pull(&handle, 1.1);
    assert!(
        reconstruct(&cancelled)
            .iter()
            .flatten()
            .any(|cell| cell.ch != 'P')
    );

    handle.command_argument_lifecycle(
        context.clone(),
        maki_lua::CommandArgumentLifecycle::Highlight,
        Some(item.clone()),
        maki_agent::CancelToken::none(),
    );
    handle.command_argument_lifecycle(
        context,
        maki_lua::CommandArgumentLifecycle::Accept,
        Some(item),
        maki_agent::CancelToken::none(),
    );
    guard
        .host()
        .load_source("completion_accept_barrier", "return nil")
        .unwrap();
    let _accepted = pull_glyph(&handle, 1.2, 'P');
}

#[test]
fn persisted_runtime_rollback_survives_restart() {
    let state_dir = tempfile::tempdir().unwrap();
    {
        let host = spawn_host_for_tests_with_state(
            &["splash", "splash_gallery"],
            state_dir.path().to_owned(),
        );
        host.load_source(
            "restart_fixture",
            r##"
            local calls = 0
            maki.api.register("splash.gallery", "fragile", {
              label = "Fragile",
              activate = function()
                return function(w, h)
                  calls = calls + 1
                  if calls > 2 then error("failed") end
                  local rows = {}
                  for y = 1, h do rows[y] = { { glyphs = string.rep("F", w), style = "#ffffff" } } end
                  return rows
                end
              end,
            })
            "##,
        )
        .unwrap();
        let handle = host.event_handle();
        handle
            .run_command_for_test(
                "splash_gallery".into(),
                "/splash".into(),
                "fragile".into(),
                0,
            )
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let _ = pull_glyph(&handle, 1.0, 'F');
        let _ = pull(&handle, 2.0);
        let _ = pull(&handle, 3.0);
        host.load_source("restart_rollback_barrier", "return nil")
            .unwrap();
    }

    let host =
        spawn_host_for_tests_with_state(&["splash", "splash_gallery"], state_dir.path().to_owned());
    host.load_source(
        "restart_assertion",
        r##"
        local path = maki.fs.joinpath(maki.env.state_dir(), "splash_gallery", "selection.json")
        assert(maki.fs.read(path) == nil)
        "##,
    )
    .expect("restart observes repaired default preference");
}

#[test]
fn persisted_missing_or_invalid_selection_uses_default_and_repairs_state() {
    for content in [r#"{"name":"missing"}"#, "not json"] {
        let state_dir = tempfile::tempdir().unwrap();
        let gallery_dir = state_dir.path().join("splash_gallery");
        std::fs::create_dir(&gallery_dir).unwrap();
        let selection_path = gallery_dir.join("selection.json");
        std::fs::write(&selection_path, content).unwrap();

        let host = spawn_host_for_tests_with_state(
            &["splash", "splash_gallery"],
            state_dir.path().to_owned(),
        );
        let handle = host.event_handle();
        let frame = pull(&handle, 1.0);
        let glyphs = frame
            .rows
            .iter()
            .map(|segment| segment.glyphs.as_str())
            .collect::<String>();
        assert!(glyphs.contains("luna-maki"));
        host.load_source("startup_repair_barrier", "return nil")
            .unwrap();
        assert!(
            !selection_path.exists(),
            "startup fallback must durably clear {content:?}"
        );
    }
}

#[test]
fn active_contributor_reload_re_resolves_renderer() {
    let (handle, guard) = spawn_host_for_tests(&["splash", "splash_gallery"]);
    let source = |glyph| {
        format!(
            r##"
            maki.api.register("splash.gallery", "third_party", {{
              label = "Third party",
              activate = function()
                return function(w, h)
                  local rows = {{}}
                  for y = 1, h do rows[y] = {{ {{ glyphs = string.rep("{glyph}", w), style = "#ffffff" }} }} end
                  return rows
                end
              end,
            }})
            "##
        )
    };
    guard
        .host()
        .load_source("contributor", &source('A'))
        .unwrap();
    handle
        .run_command_for_test(
            "splash_gallery".into(),
            "/splash".into(),
            "third_party".into(),
            0,
        )
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let _active = pull_glyph(&handle, 1.0, 'A');

    guard
        .host()
        .load_source("contributor", &source('B'))
        .unwrap();
    let _reloaded = pull_glyph(&handle, 1.2, 'B');
}

#[test]
fn default_is_owned_only_by_standalone_splash() {
    let (handle, guard) = spawn_host_for_tests(&["splash", "splash_gallery"]);
    guard
        .host()
        .load_source(
            "default_ownership",
            r##"
            local ok = pcall(require, "splash.default")
            assert(not ok, "gallery must not bundle a duplicate default module")
            "##,
        )
        .unwrap();
    assert!(pull(&handle, 1.0).rows.len() > 1);
}
