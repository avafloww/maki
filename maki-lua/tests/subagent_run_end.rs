//! Reproduction for: async subagents die as soon as the run that spawned them
//! ends, so `task_get` reports `closed` and the subagent never surfaces in the
//! UI task list.
//!
//! Hypothesis: a subagent's `child_cancel` is derived from the parent run's
//! cancel (`agent_ctx.cancel.child()` in `maki.agent.session`). At the end of a
//! normal run the UI's `clear_cancel_trigger` drops the run's `CancelTrigger`,
//! and `CancelTrigger::drop` FIRES the cancel. That cascades into the child and
//! closes every in-flight subagent's driver before it can finish.
//!
//! The subagent is created with NO `model_spec`, so `session()` inherits the
//! parent context's mock provider (stub_ctx's `NullProvider`) and no message is
//! sent, so the driver merely parks on its input queue — no model call, no
//! network, no credentials. This isolates the cancel-lifecycle behavior.

use std::sync::Arc;

use maki_agent::cancel::{CancelMap, CancelToken, CancelTrigger};
use maki_agent::tools::test_support::stub_ctx;
use maki_agent::tools::{ToolContext, ToolOutput, ToolRegistry};
use maki_agent::{AgentMode, ToolOutput};
use maki_lua::PluginHost;
use serde_json::{json, Value};

const PROBE_SRC: &str = r#"
session_holder = { sess = nil }

maki.api.register_tool({
  name = "probe_spawn",
  description = "create a subagent session without running it",
  schema = { type = "object", properties = {}, additionalProperties = false },
  audiences = { "main" },
  handler = function(input, ctx)
    local sess, err = maki.agent.session(ctx, { name = "probe" })
    if not sess then
      return { llm_output = "spawn failed: " .. err, is_error = true }
    end
    session_holder.sess = sess
    return maki.json.encode({ task_id = sess:session_id() })
  end,
})

maki.api.register_tool({
  name = "probe_status",
  description = "read the subagent session's status",
  schema = { type = "object", properties = {}, additionalProperties = false },
  audiences = { "main" },
  handler = function()
    return maki.json.encode(session_holder.sess:status())
  end,
})
"#;

fn load_probe_host() -> (Arc<ToolRegistry>, PluginHost) {
    let reg = Arc::new(ToolRegistry::new());
    let host = PluginHost::new(Arc::clone(&reg)).unwrap();
    host.load_source("probe", PROBE_SRC).unwrap();
    (reg, host)
}

/// A real tool context that mimics production: a live parent cancel token plus
/// a real subagent-cancel map, so spawned subagents derive from a run cancel.
/// The run `CancelTrigger` is returned separately so the test controls when the
/// run's cancel fires (dropping/firing the trigger is what production does at
/// run end).
fn production_like_ctx() -> (ToolContext, CancelTrigger) {
    let mut ctx = stub_ctx(&AgentMode::Build);
    let (run_trigger, run_cancel) = CancelToken::new();
    ctx.cancel = run_cancel;
    ctx.subagent_cancels = Arc::new(CancelMap::new());
    (ctx, run_trigger)
}

fn exec_tool(
    reg: &ToolRegistry,
    ctx: &ToolContext,
    name: &str,
    input: Value,
) -> Result<Value, String> {
    let inv = reg
        .get(name)
        .unwrap_or_else(|| panic!("tool {name} not registered"))
        .tool
        .parse(&input)
        .expect("parse failed");
    let out = smol::block_on(async { inv.execute(ctx).await })
        .output
        .map(|out| match out {
            ToolOutput::Plain(s) | ToolOutput::Markdown(s) => s.text,
            other => panic!("unexpected output: {other:?}"),
        })
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&out).map_err(|e| format!("invalid json {out:?}: {e}"))
}

/// A subagent spawned during a run must survive that run ending normally.
/// `status()` should report it as `running`/`done`, never `closed`, unless it
/// was explicitly despawned or a global cancel fired.
#[test]
fn subagent_outlives_the_run_that_spawned_it() {
    let (reg, _host) = load_probe_host();
    let (ctx, run_trigger) = production_like_ctx();

    let spawned = exec_tool(&reg, &ctx, "probe_spawn", json!({})).expect("spawn failed");
    eprintln!("spawn -> {spawned}");
    let task_id = spawned["task_id"].as_str().unwrap().to_owned();
    let _ = task_id;

    // Before the run ends, the subagent must be alive (running), not closed.
    let before = exec_tool(&reg, &ctx, "probe_status", json!({})).unwrap();
    eprintln!("status (before run end) -> {before}");
    assert_ne!(
        before["status"],
        "closed",
        "subagent must be alive right after spawn: {before}"
    );

    // Simulate the spawning run ending normally: production drops the run's
    // CancelTrigger in clear_cancel_trigger, which fires the run cancel.
    run_trigger.cancel();

    // Give the driver a beat to observe the cancel, then check the outcome.
    let mut status = Value::Null;
    for _ in 0..20 {
        status = exec_tool(&reg, &ctx, "probe_status", json!({})).unwrap();
        eprintln!("status (after run end) -> {status}");
        if status["status"] == "done" || status["status"] == "closed" {
            break;
        }
    }
    assert_ne!(
        status["status"],
        "closed",
        "a spawned subagent must not be closed by its parent run ending normally: {status}"
    );
}