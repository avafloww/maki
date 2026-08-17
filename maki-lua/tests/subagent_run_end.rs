//! Reproduction tests for async subagent lifecycle regressions.
//!
//! 1. `subagent_outlives_the_run_that_spawned_it`: a subagent must survive its
//!    parent run ending normally (the run's cancel must not close it).
//! 2. `subagent_completion_surfaces_reply_to_parent`: after a subagent finishes
//!    a run, its transcript must be surfaced to the parent so the UI can queue
//!    the reply back to the main agent (SubagentHistory must be emitted, not
//!    only on explicit close).

use std::sync::Arc;

use maki_agent::AgentEvent;
use maki_agent::tools::ToolRegistry;
use maki_lua::PluginHost;
use maki_providers::Role;
use serde_json::{Value, json};

mod common;
use common::{ctx_with_canned_provider, exec_tool, production_like_ctx};

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

maki.api.register_tool({
  name = "probe_send",
  description = "send a message to the subagent session",
  schema = { type = "object", properties = { message = { type = "string" } }, additionalProperties = false },
  audiences = { "main" },
  handler = function(input)
    local _, err = session_holder.sess:send(input.message)
    if err then
      return { llm_output = "send failed: " .. err, is_error = true }
    end
    return maki.json.encode({ ok = true })
  end,
})
"#;

fn load_probe_host() -> (Arc<ToolRegistry>, PluginHost) {
    let reg = Arc::new(ToolRegistry::new());
    let host = PluginHost::new(Arc::clone(&reg)).unwrap();
    host.load_source("probe", PROBE_SRC).unwrap();
    (reg, host)
}

/// A subagent spawned during a run must survive that run ending normally.
/// `status()` should report it as `running`/`done`, never `closed`.
#[test]
fn subagent_outlives_the_run_that_spawned_it() {
    let (reg, _host) = load_probe_host();
    let (ctx, run_trigger) = production_like_ctx();

    exec_tool(&reg, &ctx, "probe_spawn", json!({})).expect("spawn failed");

    let before = exec_tool(&reg, &ctx, "probe_status", json!({})).unwrap();
    eprintln!("status (before run end) -> {before}");
    assert_ne!(
        before["status"], "closed",
        "subagent must be alive after spawn"
    );

    run_trigger.cancel();

    let mut status = Value::Null;
    for _ in 0..20 {
        status = exec_tool(&reg, &ctx, "probe_status", json!({})).unwrap();
        eprintln!("status (after run end) -> {status}");
        if status["status"] == "done" || status["status"] == "closed" {
            break;
        }
    }
    assert_ne!(
        status["status"], "closed",
        "a spawned subagent must not be closed by its parent run ending normally: {status}"
    );
}

/// After a subagent finishes a run, its transcript must be surfaced to the
/// parent (as a SubagentHistory envelope) so the UI can queue the reply back to
/// the main agent with a header. Regression: this was only emitted on explicit
/// close, so a completed async subagent's reply never reached the main agent.
#[test]
fn subagent_completion_surfaces_reply_to_parent() {
    let (reg, _host) = load_probe_host();
    let (ctx, parent_rx, _run_trigger) = ctx_with_canned_provider();

    exec_tool(&reg, &ctx, "probe_spawn", json!({})).expect("spawn failed");
    exec_tool(
        &reg,
        &ctx,
        "probe_send",
        json!({ "message": "do the thing" }),
    )
    .expect("send failed");

    // Pump the executor and drain the parent event channel until the subagent's
    // completed run surfaces its history.
    let mut found = false;
    for _ in 0..100 {
        for envelope in parent_rx.try_iter() {
            if let AgentEvent::SubagentHistory { messages, .. } = &envelope.event {
                let has_reply = messages
                    .iter()
                    .any(|m| matches!(m.role, Role::Assistant) && !m.content.is_empty());
                eprintln!("subagent surfaced history, has reply: {has_reply}");
                if has_reply {
                    found = true;
                }
            }
        }
        if found {
            break;
        }
        smol::block_on(async { smol::Timer::after(std::time::Duration::from_millis(5)).await });
    }
    assert!(
        found,
        "a completed subagent's reply must reach the parent as SubagentHistory"
    );
}
