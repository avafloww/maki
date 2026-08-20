//! Hermetic real-host tests: a full `PluginHost` (background Lua thread,
//! real builtins) booted on the in-memory FS backend. The `TEST_STATE_DIR`
//! assertions prove the whole host ran disk-free: nothing may create that
//! path on the real filesystem.

use std::sync::Arc;
use std::time::{Duration, Instant};

use maki_agent::AgentMode;
use maki_agent::tools::test_support::stub_ctx;
use maki_agent::tools::ToolRegistry;

const NOTE_PATH: &str = "note.md";
const NOTE_BODY: &str = "hello-maki";
const WROTE_PREFIX: &str = "wrote note.md";
const PROJECTS_PREFIX: &str = "/maki-test-state/projects/";
const SPLASH_DEADLINE: Duration = Duration::from_secs(30);
const SPLASH_BACKOFF: Duration = Duration::from_millis(100);

fn exec(reg: &Arc<ToolRegistry>, tool: &str, input: serde_json::Value) -> String {
    let mut ctx = stub_ctx(&AgentMode::Build);
    ctx.registry = Arc::clone(reg);
    let inv = reg
        .get(tool)
        .unwrap_or_else(|| panic!("tool {tool} not registered"))
        .tool
        .parse(&input)
        .expect("parse failed");
    let result = smol::block_on(inv.execute(&ctx));
    match result.output.expect("tool output") {
        maki_agent::ToolOutput::Plain(s) | maki_agent::ToolOutput::Markdown(s) => s.text,
        other => panic!("unexpected output: {other:?}"),
    }
}

#[test]
fn memory_write_read_roundtrip_stays_in_memory() {
    let state = std::path::Path::new(maki_lua::test_support::TEST_STATE_DIR);
    assert!(!state.exists(), "precondition: TEST_STATE_DIR must not exist on disk");

    let (_handle, guard) = maki_lua::test_support::spawn_host_for_tests(&["memory"]);
    let reg = guard.host().registry();

    let wrote = exec(
        &reg,
        "memory",
        serde_json::json!({
            "command": "write",
            "path": NOTE_PATH,
            "content": NOTE_BODY,
            "tags": ["t"],
        }),
    );
    assert!(wrote.contains(WROTE_PREFIX), "got: {wrote}");

    let read = exec(
        &reg,
        "memory",
        serde_json::json!({ "command": "read", "path": NOTE_PATH }),
    );
    assert!(read.contains(NOTE_BODY), "got: {read}");

    // The note landed in the in-memory backend, not on disk.
    let notes: Vec<_> = guard
        .backend()
        .files()
        .into_iter()
        .filter(|(p, _)| p.to_string_lossy().starts_with(PROJECTS_PREFIX))
        .collect();
    assert_eq!(notes.len(), 1, "exactly one note file in the backend");
    let (path, bytes) = &notes[0];
    let content = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        path.file_name().is_some_and(|n| n == NOTE_PATH),
        "unexpected note path: {path:?}"
    );
    assert!(content.starts_with("---\n"), "frontmatter missing: {content}");
    assert!(content.contains(NOTE_BODY), "body missing: {content}");

    assert!(!state.exists(), "the round-trip must not touch the real disk");
}

#[test]
fn splash_host_boots_disk_free() {
    let state = std::path::Path::new(maki_lua::test_support::TEST_STATE_DIR);
    assert!(!state.exists(), "precondition: TEST_STATE_DIR must not exist on disk");

    let (handle, _guard) = maki_lua::test_support::spawn_host_for_tests(&["splash"]);
    let start = Instant::now();
    let frame = loop {
        if let Some(frame) = handle.splash_frame(80, 20, 10.0, 1.0) {
            break frame;
        }
        assert!(
            start.elapsed() < SPLASH_DEADLINE,
            "splash frame never arrived"
        );
        std::thread::sleep(SPLASH_BACKOFF);
    };
    assert!(!frame.is_empty(), "rows must be non-empty");

    assert!(!state.exists(), "the frame pull must not touch the real disk");
}