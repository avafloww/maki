//! Headless-safe half of the `plan_submit` gate (AC.5).
//!
//! The plan_submit tool must refuse to run outside plan mode. The full
//! render/accept/reject UI flow is covered only by the pre-existing `maki-ui`
//! app tests (kept as-is); here we assert the plugin-level gate fires without a
//! UI: with `mode.get()` answering nil (the headless shape — no UI lane answers
//! the mode query), `current_mode()` is nil and the tool errors before touching
//! `maki.ui.action`.

use std::sync::Arc;

use maki_agent::tools::test_support::stub_ctx;
use maki_agent::tools::ToolRegistry;
use maki_agent::AgentMode;
use maki_lua::PluginHost;
use serde_json::json;

mod common;
use common::exec_tool;

const PLAN_SUBMIT_SRC: &str = include_str!("../../plugins/plan_submit_tool/init.lua");
const GATE_ERR: &str = "plan_submit is only available in plan mode";

#[test]
fn plan_submit_rejected_outside_plan_mode() {
    let reg = Arc::new(ToolRegistry::new());
    let _host = PluginHost::new(Arc::clone(&reg)).unwrap();
    // Stub the mode to a non-plan id (the same pattern the task_policy suite
    // uses to gate without a UI). `current_mode() ~= "plan"` fires the gate
    // before `maki.ui.action` is ever reached, so no UI lane is needed.
    let prelude = "maki.api.mode.get = function() return 'build' end\n\n";
    _host.load_source("plan_submit_policy", &format!("{prelude}\n{PLAN_SUBMIT_SRC}"))
        .unwrap();

    let ctx = stub_ctx(&AgentMode::Build);
    let err = exec_tool(&reg, &ctx, "plan_submit", json!({})).unwrap_err();
    assert!(err.contains(GATE_ERR), "got: {err}");
}
