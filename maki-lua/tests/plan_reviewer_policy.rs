//! plan_reviewer through the real driver (AC.10).
//!
//! The reviewer is spawned via the real `task` plugin (`subagent_type =
//! "plan_reviewer"`), which gates it to plan mode and hands it a `research_sub`
//! (read-only) tool set. These tests drive the real `maki.agent.session` (wrapped
//! only to set `inherit_provider = true`) against a canned provider, so the
//! verdict text and the tool set the reviewer actually received are observable.
//! The plan-mode UI flow on a fail verdict is UI behavior, intentionally not
//! tested here.

use std::sync::Arc;
use std::time::Duration;

use maki_agent::tools::{ToolContext, ToolRegistry};
use maki_lua::PluginHost;
use serde_json::{json, Value};

mod common;
use common::{canned_reply, ctx_with_provider, exec_tool, tool_names, CannedProvider};

const TASK_PLUGIN_SRC: &str = include_str!("../../plugins/task/init.lua");
const PLAN_REVIEWER_ONLY_ERR: &str = "plan_reviewer is only available in plan mode";

/// Wrap the real `maki.agent.session` to reuse the parent (canned) provider, and
/// stub the model/system lookups so no real model resolution or file read is
/// needed. `maki.agent.tools` is left real when `real_tools` is set (so the
/// reviewer's tool set can be inspected); otherwise it is stubbed to empty.
const SESSION_WRAPPER: &str = r#"
local real_session = maki.agent.session
maki.agent.session = function(ctx, opts)
  opts.inherit_provider = true
  return real_session(ctx, opts)
end
maki.agent.resolve_model = function(ctx, opts)
  return { spec = "anthropic/claude-sonnet-4-20250514" }
end
maki.agent.system_prompt = function(ctx, opts)
  return "sys"
end
"#;

const TOOLS_STUB: &str = "maki.agent.tools = function(ctx, opts) return {} end\n";

/// A read tool visible to `research_sub` and a write tool visible only to
/// `general_sub`, so the real `maki.agent.tools` can demonstrate the reviewer's
/// audience-based read-only filtering: the reviewer (research_sub) gets `read`
/// but not `write`.
const FAKE_READ_WRITE_TOOLS: &str = r#"
maki.api.register_tool({
  name = "read",
  description = "fake read",
  kind = "execute",
  schema = { type = "object", properties = {}, additionalProperties = false },
  audiences = { "research_sub" },
  handler = function() return "read-only" end,
})
maki.api.register_tool({
  name = "write",
  description = "fake write",
  kind = "execute",
  schema = { type = "object", properties = {}, additionalProperties = false },
  audiences = { "general_sub" },
  handler = function() return "wrote" end,
})
"#;

fn load_reviewer_host(mode: &str, real_tools: bool) -> (Arc<ToolRegistry>, PluginHost) {
    let reg: Arc<ToolRegistry> = if real_tools {
        ToolRegistry::global_arc().clone()
    } else {
        Arc::new(ToolRegistry::new())
    };
    let host = PluginHost::new(Arc::clone(&reg)).unwrap();
    let mode_stub = format!("maki.api.mode.get = function() return '{mode}' end\n");
    let tools = if real_tools {
        FAKE_READ_WRITE_TOOLS
    } else {
        TOOLS_STUB
    };
    let src = format!("{mode_stub}\n{tools}\n{SESSION_WRAPPER}\n{TASK_PLUGIN_SRC}");
    host.load_source("plan_reviewer_policy", &src).unwrap();
    (reg, host)
}

fn reviewer_input(description: &str) -> Value {
    let mut input = json!({ "description": description, "prompt": "Review the plan." });
    input["subagent_type"] = json!("plan_reviewer");
    input
}

fn wait_task_done(reg: &ToolRegistry, ctx: &ToolContext, task_id: &str) -> Value {
    for _ in 0..400 {
        if let Ok(out) = exec_tool(reg, ctx, "task_get", json!({ "task_id": task_id }))
            && (out["status"] == "done" || out["status"] == "closed")
        {
            return out;
        }
        smol::block_on(async { smol::Timer::after(Duration::from_millis(5)).await });
    }
    panic!("task {task_id} did not finish");
}

#[test]
fn plan_reviewer_audits_plan_and_returns_verdict() {
    let (reg, _host) = load_reviewer_host("plan", false);

    let pass = Arc::new(CannedProvider::new(vec![canned_reply("VERDICT: pass")]));
    let (ctx_pass, _rx, _trigger) = ctx_with_provider(Arc::clone(&pass));
    let spawn = exec_tool(&reg, &ctx_pass, "task_spawn", reviewer_input("pass-review"))
        .expect("reviewer must spawn in plan mode");
    let status = wait_task_done(&reg, &ctx_pass, spawn["task_id"].as_str().unwrap());
    assert!(
        status["result"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("VERDICT: pass")),
        "pass verdict must surface: {status}"
    );

    let fail = Arc::new(CannedProvider::new(vec![
        canned_reply("VERDICT: fail\n- finding: missing AC mapping"),
    ]));
    let (ctx_fail, _rx, _trigger) = ctx_with_provider(Arc::clone(&fail));
    let spawn = exec_tool(&reg, &ctx_fail, "task_spawn", reviewer_input("fail-review"))
        .expect("reviewer must spawn in plan mode");
    let status = wait_task_done(&reg, &ctx_fail, spawn["task_id"].as_str().unwrap());
    assert!(
        status["result"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("VERDICT: fail")),
        "fail verdict with findings must surface: {status}"
    );
}

#[test]
fn plan_reviewer_is_read_only_even_when_asked_to_write() {
    let provider = Arc::new(CannedProvider::new(vec![canned_reply("VERDICT: pass")]));
    let (ctx, _rx, _trigger) = ctx_with_provider(Arc::clone(&provider));
    let (reg, _host) = load_reviewer_host("plan", true);

    let spawn = exec_tool(&reg, &ctx, "task_spawn", reviewer_input("readonly-review"))
        .expect("reviewer must spawn in plan mode");
    wait_task_done(&reg, &ctx, spawn["task_id"].as_str().unwrap());

    let names = tool_names(&provider.captured_tools()[0]);
    assert!(
        names.iter().any(|n| n == "read"),
        "reviewer (research_sub) must receive the read tool: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "write"),
        "reviewer must exclude write tools (read-only enforced): {names:?}"
    );
}

#[test]
fn plan_reviewer_only_in_plan_mode() {
    let provider = Arc::new(CannedProvider::new(vec![]));
    let (ctx, _rx, _trigger) = ctx_with_provider(Arc::clone(&provider));
    let (reg, _host) = load_reviewer_host("build", false);

    let err = exec_tool(&reg, &ctx, "task_spawn", reviewer_input("gated-review"))
        .expect_err("plan_reviewer must be blocked outside plan mode");
    assert!(err.contains(PLAN_REVIEWER_ONLY_ERR), "got: {err}");
    assert!(
        provider.captured_thinking().is_empty(),
        "no session must spawn outside plan mode"
    );
}
