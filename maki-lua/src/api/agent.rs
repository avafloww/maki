//! `maki.agent` exposes subagent primitives to Lua plugins. Policy (retries,
//! validation, concurrency) lives in the task plugin, not here.

use std::collections::HashMap;
use std::pin::pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_lock::Mutex as AsyncMutex;
use futures::future::{Either, select};
use maki_agent::agent::tool_dispatch::{self, Emit};
use maki_agent::cancel::{CancelMap, CancelSlot, CancelToken};
use maki_agent::tools::interpreter_bridge;
use maki_agent::tools::registry::ToolRegistry;
use maki_agent::tools::schema::sanitize_tool_input_schema;
use maki_agent::tools::{
    Deadline, DescriptionContext, FileReadTracker, LocalToolFn, LocalTools, ToolAudience,
    ToolContext, ToolFilter, ToolLive,
};
use maki_agent::{
    Agent, AgentEvent, AgentInput, AgentMode, AgentParams, AgentRunParams, DoneReason,
    EMPTY_RESPONSE_MARKER, Envelope, EventSender, History, McpSession, SubagentInfo, ToolDoneEvent,
};
use maki_lua_macro::{lua_class, lua_fn, lua_table};
use maki_providers::model::ModelTier;
use maki_providers::provider;
use maki_providers::{ContentBlock, Model, ModelError, Role, ThinkingConfig, TokenUsage, add_cost};
use maki_storage::id::MakiId;
use maki_storage::sessions::StoredThinking;
use mlua::{Function, IntoLuaMulti, Lua, Result as LuaResult, Table, Value as LuaValue};
use serde_json::Value as JsonValue;
use tracing::info;

use crate::api::ui::buf::BufHandle;
use crate::api::util::convert::{json_to_lua, lua_to_json, lua_tool_result};
use crate::api::util::ctx::{AgentContext, LuaCtx};
use crate::api::util::pair::{Pair, err_pair, try_pair};
use crate::runtime::CANCELLED_MSG;

const SESSION_CLOSED_ERR: &str = "session closed";
const DEFAULT_SESSION_AUDIENCE: ToolAudience = ToolAudience::GENERAL_SUB;

/// A per-session structured-output commit slot.
type CommitSlot = Arc<Mutex<Option<JsonValue>>>;

/// Build the `(model, provider)` pair that backs a `maki.agent.session` spawn.
/// `inherit_provider` (or an absent `model_spec`) reuses the parent model and
/// provider so a test-side wrapper can drive a real spawned session through a
/// caller-provided provider without network. Otherwise build a provider for the
/// given `model_spec`.
async fn build_session_provider(
    model_spec: &Option<String>,
    inherit_provider: bool,
    agent_ctx: &AgentContext,
) -> Result<(Model, Arc<dyn provider::Provider>), String> {
    if inherit_provider || model_spec.is_none() {
        Ok((
            Model::clone(&agent_ctx.model),
            Arc::clone(&agent_ctx.provider),
        ))
    } else {
        let spec = model_spec.as_deref().expect("model_spec present");
        let mut m = Model::from_spec_with_policy(spec, &agent_ctx.model_policy)
            .map_err(|e| e.to_string())?;
        let p = provider::from_model_async(&mut m, agent_ctx.timeouts)
            .await
            .map_err(|e| e.to_string())?;
        Ok((m, Arc::from(p)))
    }
}

/// Per-session structured-output commit slots, keyed by the session's ui_id.
/// `maki.agent.report_task_result` routes a commit to whichever session is
/// running the tool, so the background driver can surface it as `captured`.
fn commit_registry() -> &'static Mutex<HashMap<String, CommitSlot>> {
    static REG: OnceLock<Mutex<HashMap<String, CommitSlot>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_commit_slot(id: &str, slot: CommitSlot) {
    commit_registry()
        .lock()
        .unwrap()
        .insert(id.to_string(), slot);
}

fn unregister_commit_slot(id: &str) {
    commit_registry().lock().unwrap().remove(id);
}

/// Observable phase of a subagent run, read by `status()`.
enum SubagentStatus {
    Running,
    Done(SubagentRunResult),
    Closed,
}

/// Terminal result of one subagent message, surfaced by `status()`/`prompt`.
#[derive(Clone)]
struct SubagentRunResult {
    text: String,
    captured: Option<JsonValue>,
    usage: TokenUsage,
    error: Option<String>,
}

fn resolve_model_from_ctx(ctx: &AgentContext, tier: Option<&str>) -> Result<Model, String> {
    let Some(tier_str) = tier else {
        return Ok(Model::clone(&ctx.model));
    };
    let requested: ModelTier = tier_str.parse().map_err(|e: ModelError| e.to_string())?;
    let effective = requested.min(ctx.model.tier);
    if effective == ctx.model.tier {
        return Ok(Model::clone(&ctx.model));
    }
    let slug = &ctx.model.provider;
    maki_providers::model_registry::spec_for_tier(slug, effective)
        .or_else(|| maki_providers::model_registry::spec_for_tier_any(effective))
        .filter(|spec| ctx.model_policy.allows(spec))
        .and_then(|s| Model::from_spec(&s).ok())
        .map(Ok)
        .unwrap_or_else(|| {
            Model::from_tier_with_policy(slug, effective, &ctx.model_policy)
                .map_err(|e| e.to_string())
        })
}

fn model_to_lua_table(lua: &Lua, model: &Model) -> LuaResult<Table> {
    let tbl = lua.create_table()?;
    tbl.set("id", model.id.clone())?;
    tbl.set("tier", model.tier.to_string())?;
    tbl.set("provider", model.provider.to_string())?;
    tbl.set("spec", model.spec())?;
    Ok(tbl)
}

fn dispatch_ctx<'a>(ctx: &'a LuaCtx, method: &str) -> Result<&'a AgentContext, String> {
    ctx.agent()
        .ok_or_else(|| ctx.cap_err(&format!("maki.agent.{method}")))
}

/// Forwards subagent events to the parent, stamped with the subagent identity.
/// Usage takes two paths: live on the tool header while the run goes on (last
/// turn's tokens plus the run's summed cost), and one total per run on
/// `usage_tx`, which `prompt` waits for.
async fn relay_session_events(
    sub_rx: flume::Receiver<Envelope>,
    parent_tx: EventSender,
    subagent_info: Arc<OnceLock<SubagentInfo>>,
    usage_tx: flume::Sender<TokenUsage>,
    live_sink: Option<flume::Sender<ToolLive>>,
    silent: bool,
) {
    let mut cost = None;
    while let Ok(mut envelope) = sub_rx.recv_async().await {
        if silent {
            // Keep the driver's usage barrier satisfied (it waits on usage_rx)
            // without relaying the subagent's turn into the parent session.
            if let AgentEvent::Done { usage, .. } = &envelope.event {
                let _ = usage_tx.send(*usage);
            }
            continue;
        }
        match &envelope.event {
            AgentEvent::TurnComplete(turn) => {
                add_cost(&mut cost, turn.cost);
                if let Some(sink) = &live_sink {
                    let _ = sink.send(ToolLive::Usage(turn.usage.format_sum_cost(cost)));
                }
            }
            AgentEvent::Done { usage, .. } => {
                let _ = usage_tx.send(*usage);
                continue;
            }
            AgentEvent::Error { .. }
            | AgentEvent::ToolOutput { .. }
            | AgentEvent::ToolPending { .. }
            | AgentEvent::SubagentHistory { .. } => continue,
            _ => {}
        }
        envelope.subagent = subagent_info.get().cloned();
        let _ = parent_tx.send_envelope(envelope);
    }
}

/// Look up the model that the current agent is using, or pick a cheaper one.
/// You might want a cheaper model for simple subtasks (summaries, classification)
/// without hard-coding a model name.
///
/// The returned table has fields: `id` (string), `tier` (string),
/// `provider` (string), `spec` (string).
///
/// @param ctx LuaCtx Agent context.
/// @param opts table? Optional fields:
///   `tier` (string?) - target tier, e.g. `"fast"`, `"mid"`, `"best"`. Clamped to
///     the parent tier so you cannot escalate.
///   `spec` (string?) - exact model spec string, e.g. `"claude-3-5-haiku-20241022"`.
///     Takes precedence over `tier`.
/// @return (table?, string?) Model table on success, or `(nil, err)` on failure.
/// @example
/// local model, err = maki.agent.resolve_model(ctx, { tier = "fast" })
/// if err then error(err) end
/// print(model.spec, model.tier)
#[lua_fn]
async fn resolve_model(
    lua: Lua,
    ctx: mlua::UserDataRef<LuaCtx>,
    opts: Option<Table>,
) -> LuaResult<Pair<Table>> {
    let agent = try_pair!(dispatch_ctx(&ctx, "resolve_model"));
    let tier_str = opts
        .as_ref()
        .and_then(|t| t.get::<Option<String>>("tier").ok().flatten());
    let spec_str = opts
        .as_ref()
        .and_then(|t| t.get::<Option<String>>("spec").ok().flatten());

    let model = match spec_str {
        Some(ref spec) => try_pair!(Model::from_spec_with_policy(spec, &agent.model_policy)),
        None => try_pair!(resolve_model_from_ctx(agent, tier_str.as_deref())),
    };
    Ok((Some(model_to_lua_table(&lua, &model)?), None))
}

/// Build a system prompt from a built-in template. Environment variables like
/// `{cwd}` are substituted automatically. Use this when you need a ready-made
/// prompt for a subagent session.
///
/// @param ctx LuaCtx Agent context.
/// @param opts table Required fields:
///   `prompt_id` (string) - one of `"research"`, `"general"`, `"system"`.
/// Optional fields:
///   `instructions` (string|boolean?) - extra text appended to the prompt.
///     `true` loads instructions from the project `.maki/instructions` file.
///     `false` or nil omits them.
/// @return (string?, string?) The assembled prompt string, or `(nil, err)` on failure.
/// @example
/// local prompt, err = maki.agent.system_prompt(ctx, {
///   prompt_id = "research",
///   instructions = true,
/// })
/// if err then error(err) end
#[lua_fn]
async fn system_prompt(
    _lua: Lua,
    ctx: mlua::UserDataRef<LuaCtx>,
    opts: Table,
) -> LuaResult<Pair<String>> {
    let slots = Arc::clone(&try_pair!(dispatch_ctx(&ctx, "system_prompt")).prompt_slots);
    // Nothing may hold the ctx borrow across the wait: a cancel hook firing
    // meanwhile needs `ctx:finish`, which takes it mutably.
    drop(ctx);
    let prompt_id_str: String = opts.get("prompt_id")?;
    let prompt_id = match prompt_id_str.as_str() {
        "research" => maki_agent::prompt::PromptId::Research,
        "general" => maki_agent::prompt::PromptId::General,
        "system" => maki_agent::prompt::PromptId::System,
        other => return Ok(err_pair(format!("unknown prompt_id: {other}"))),
    };

    let vars = maki_agent::template::env_vars();
    let instructions_val: LuaValue = opts.get("instructions")?;
    let instructions = match instructions_val {
        LuaValue::Boolean(true) => {
            let cwd = vars.apply("{cwd}").into_owned();
            smol::unblock(move || maki_agent::agent::load_instruction_text(&cwd)).await
        }
        LuaValue::Boolean(false) | LuaValue::Nil => String::new(),
        LuaValue::String(s) => s.to_str()?.to_owned(),
        _ => return Err(mlua::Error::runtime("instructions must be bool or string")),
    };

    let assembled = maki_agent::prompt::assemble(prompt_id, &slots, &instructions);
    Ok((Some(vars.apply(&assembled).into_owned()), None))
}

/// Get the list of tool definitions for a given audience. Pass the result
/// straight into `maki.agent.session()` or use it to inspect what tools are
/// available.
///
/// @param ctx LuaCtx Agent context.
/// @param opts table Required fields:
///   `audience` (string) - tool audience filter, e.g. `"general"`, `"subagent"`,
///     `"general_sub"`.
/// Optional fields:
///   `only` (string[]?) - include only these tool names.
///   `except` (string[]?) - exclude these tool names.
///   `workflow` (boolean?) - use workflow-mode descriptions. Default: `false`.
///   `spec` (string?) - evaluate capability exclusions against this model spec.
/// @return (table?, string?) Array of tool definition tables, or `(nil, err)` on failure.
/// @example
/// local defs, err = maki.agent.tools(ctx, {
///   audience = "general_sub",
///   except = { "bash", "write" },
/// })
/// if err then error(err) end
/// print(#defs .. " tools available")
#[lua_fn]
async fn tools(lua: Lua, ctx: mlua::UserDataRef<LuaCtx>, opts: Table) -> LuaResult<Pair<LuaValue>> {
    let agent = try_pair!(dispatch_ctx(&ctx, "tools"));
    let audience_str: String = opts.get("audience")?;
    let audience = try_pair!(
        ToolAudience::parse_name(&audience_str)
            .ok_or_else(|| format!("unknown audience: {audience_str}"))
    );

    let only: Option<Vec<String>> = opts.get("only")?;
    let except: Option<Vec<String>> = opts.get("except")?;
    let workflow: bool = opts.get::<Option<bool>>("workflow")?.unwrap_or(false);
    let spec_str: Option<String> = opts.get("spec")?;

    let parsed = spec_str
        .as_deref()
        .and_then(|spec| Model::from_spec_with_policy(spec, &agent.model_policy).ok());
    let model = parsed.as_ref().unwrap_or(&agent.model);

    let base = match (only, except) {
        (Some(o), _) => ToolFilter::Only(o),
        (_, Some(e)) => ToolFilter::AllExcept(e),
        _ => ToolFilter::All,
    };
    let disabled: Vec<&str> = agent
        .config
        .disabled_tools
        .iter()
        .map(String::as_str)
        .collect();
    let filter = base
        .excluding(&disabled)
        .excluding(maki_agent::tools::capability_exclusions(model));

    let vars = maki_agent::template::env_vars();
    let ctx_desc = DescriptionContext {
        filter: &filter,
        audience,
        workflow,
    };
    // Base definitions only: the session injects MCP definitions per
    // request, so baking them into a tools array would freeze the catalog.
    let defs = ToolRegistry::global().definitions(&vars, &ctx_desc, model.supports_tool_examples());

    Ok((Some(json_to_lua(&lua, &defs)?), None))
}

/// Run a tool by name and wait for the result. This is how you call built-in
/// tools (like `read`, `bash`, `glob`) from Lua without going through the LLM.
///
/// Live events (streaming output, annotations, cumulative usage) are delivered
/// through optional callbacks while the tool runs.
///
/// @param ctx LuaCtx Agent context.
/// @param name string Tool name, e.g. `"bash"`, `"read"`.
/// @param input table|any Tool input (JSON-serializable). Must match the tool's `input_schema`.
/// @param opts table? Optional fields:
///   `timeout` (integer?) - deadline in seconds.
///   `on_live_buf` (function?) - called with a `BufHandle` for each live buffer
///     the tool publishes. Must not yield.
///   `on_annotation` (function?) - called with an annotation string for each
///     annotation event. Must not yield.
///   `on_usage` (function?) - called with a formatted cumulative token usage
///     string. Must not yield.
/// @return (string?, string?) Tool output text, or `(nil, err)` on failure.
/// @example
/// local out, err = maki.agent.call_tool(ctx, "bash", {
///   command = "ls -la",
///   timeout = 10,
/// })
/// if err then error(err) end
/// print(out)
#[lua_fn]
async fn call_tool(
    lua: Lua,
    ctx: mlua::UserDataRef<LuaCtx>,
    name: String,
    input: LuaValue,
    opts: Option<Table>,
) -> LuaResult<Pair<String>> {
    let input_json = lua_to_json(&lua, &input)?;
    let agent = try_pair!(dispatch_ctx(&ctx, "call_tool"));
    let mut tctx = agent.to_tool_context();
    let (mut on_buf, mut on_ann, mut on_usage, mut rx) = (None, None, None, None);
    if let Some(o) = opts {
        if let Some(secs) = o.get::<Option<u64>>("timeout")? {
            tctx.deadline = Deadline::after(Duration::from_secs(secs));
        }
        on_buf = o.get::<Option<Function>>("on_live_buf")?;
        on_ann = o.get::<Option<Function>>("on_annotation")?;
        on_usage = o.get::<Option<Function>>("on_usage")?;
        if on_buf.is_some() || on_ann.is_some() || on_usage.is_some() {
            let (tx, r) = flume::unbounded();
            tctx.live_sink = Some(tx);
            rx = Some(r);
        }
    }
    drop(ctx);
    if let Err(e) = tctx.deadline.check() {
        return Ok(err_pair(e));
    }
    let cbs = LiveCallbacks {
        tool: &name,
        on_buf,
        on_ann,
        on_usage,
    };
    let done = dispatch_racing_live(&tctx, &name, &input_json, rx, &cbs).await;
    // Same fallback the UI applies on tool completion, so a batch child's
    // header carries the annotation its standalone run would get.
    let annotation = done
        .annotation
        .clone()
        .or_else(|| (!done.is_error).then(|| done.output.annotation()).flatten());
    if let Some(a) = annotation {
        cbs.deliver(ToolLive::Annotation(a)).await;
    }
    match interpreter_bridge::flatten(&done) {
        Ok(text) => Ok((Some(text), None)),
        Err(err) => Ok((None, Some(err))),
    }
}

/// Create a new subagent session. The session inherits the parent model and
/// MCP handle unless you override them. You get back a `Session` object that
/// you can send messages to with `:prompt()`.
///
/// This is the main way to spin up a sub-conversation with its own history
/// and tool set.
///
/// @param ctx LuaCtx Agent context.
/// @param opts table Optional fields:
///   `model_spec` (string?) - model spec string to use instead of the parent model.
///   `system` (string?) - system prompt. Defaults to empty.
///   `tools` (table?) - tool definitions array (from `maki.agent.tools()`).
///   `local_tools` (table?) - map of `name -> spec` for Lua-backed tools. Each spec
///     requires `description` (string), `input_schema` (table), and
///     `handler` (function). The handler receives the input table and must return
///     `(string)` or `(nil, err)`.
///   `name` (string?) - display name for logs and UI.
///   `audience` (string?) - tool audience for capability gating. Default: `"general_sub"`.
///   `mcp` (boolean?) - give the session access to MCP tools. Their
///     definitions are injected automatically each turn (deferred behind
///     `tool_search`), so don't put MCP definitions in `tools`. The session
///     starts with no loaded tools of its own. Default: `true`.
///   `thinking` (string|integer?) - thinking mode: `"off"`, `"adaptive"`, an
///     effort level (`"minimal"`, `"low"`, `"medium"`, `"high"`, `"xhigh"`,
///     `"max"`), or a budget integer (token count). Inherits parent setting
///     if omitted.
///   `fast` (boolean?) - use fast mode. Inherits parent setting if omitted.
///   `inherit_provider` (boolean?) - reuse the parent model and provider instead
///     of building one from `model_spec`. Used by tests (and tools that want to
///     thread a caller-provided provider) to drive a real spawned session without
///     hitting the network. `model_spec` is ignored when this is `true`. Default: `false`.
///   `silent` (boolean?) - do not relay the session's turns, annotations, or
///     usage into the parent session's UI or event stream. The session still
///     completes and `:prompt()` still returns its result (including a commit
///     set via a `local_tools` handler). Use for hidden one-shot classification.
/// @return (Session?, string?) Session handle, or `(nil, err)` on failure.
/// @example
/// local tools = maki.agent.tools(ctx, { audience = "general_sub" })
/// local sess, err = maki.agent.session(ctx, {
///   system = "You are a research assistant.",
///   tools = tools,
///   name = "researcher",
/// })
/// if err then error(err) end
/// local result = sess:prompt("Summarize this file.")
/// sess:close()
#[lua_fn]
async fn session(
    lua: Lua,
    ctx: mlua::UserDataRef<LuaCtx>,
    opts: Table,
) -> LuaResult<Pair<mlua::AnyUserData>> {
    let agent_ctx = try_pair!(dispatch_ctx(&ctx, "session")).clone();
    drop(ctx);
    let model_spec: Option<String> = opts.get("model_spec")?;
    let system: Option<String> = opts.get("system")?;
    let tools_val: Option<LuaValue> = opts.get("tools")?;
    let local_tools_tbl: Option<Table> = opts.get("local_tools")?;
    let name: Option<String> = opts.get("name")?;
    let thinking_val: Option<LuaValue> = opts.get("thinking")?;
    let audience = match opts.get::<Option<String>>("audience")? {
        Some(s) => {
            try_pair!(ToolAudience::parse_name(&s).ok_or_else(|| format!("unknown audience: {s}")))
        }
        None => DEFAULT_SESSION_AUDIENCE,
    };
    let inherit_provider: bool = opts
        .get::<Option<bool>>("inherit_provider")?
        .unwrap_or(false);
    let fast: bool = opts
        .get::<Option<bool>>("fast")?
        .unwrap_or(agent_ctx.opts.fast);
    let mcp_enabled: bool = opts.get::<Option<bool>>("mcp")?.unwrap_or(true);
    let silent: bool = opts.get::<Option<bool>>("silent")?.unwrap_or(false);

    let (model, provider): (Model, Arc<dyn provider::Provider>) =
        try_pair!(build_session_provider(&model_spec, inherit_provider, &agent_ctx).await);
    // A standalone task shows its model via SubagentInfo on the header;
    // a dispatching caller (batch) gets the same thing as a live annotation.
    if !silent && let Some(sink) = &agent_ctx.live_sink {
        let _ = sink.send(ToolLive::Annotation(model.spec()));
    }

    let mut tools_json: JsonValue = match tools_val {
        Some(val) => {
            let tools = lua_to_json(&lua, &val)?;
            if !tools.is_array() {
                return Err(mlua::Error::runtime("tools must be an array"));
            }
            tools
        }
        None => JsonValue::Array(vec![]),
    };

    let mut local_map: HashMap<String, LocalToolFn> = HashMap::new();
    if let Some(tbl) = local_tools_tbl {
        let defs = tools_json.as_array_mut().expect("checked above");
        for pair in tbl.pairs::<String, Table>() {
            let (name, spec) = pair?;
            let description = try_pair!(
                spec.get::<String>("description")
                    .map_err(|_| format!("local_tools.{name}: 'description' is required"))
            );
            let input_schema = lua_to_json(&lua, &spec.get::<LuaValue>("input_schema")?)?;
            let sanitized_schema = sanitize_tool_input_schema(input_schema);
            let handler = try_pair!(
                spec.get::<Function>("handler")
                    .map_err(|_| format!("local_tools.{name}: 'handler' is required"))
            );
            defs.push(serde_json::json!({
                "name": name,
                "description": description,
                "input_schema": sanitized_schema,
            }));
            let weak = lua.weak();
            local_map.insert(
                name,
                Arc::new(move |input: &JsonValue| call_local_tool(&weak, &handler, input))
                    as LocalToolFn,
            );
        }
    }

    let thinking = match thinking_val {
        Some(LuaValue::String(s)) => match StoredThinking::parse_setting(&s.to_str()?) {
            Ok(stored) => ThinkingConfig::from(stored),
            Err(e) => return Ok(err_pair(format!("invalid thinking: {e}"))),
        },
        Some(LuaValue::Integer(n)) => match u32::try_from(n) {
            Ok(tokens) if tokens > 0 => ThinkingConfig::Budget(tokens),
            _ => return Ok(err_pair(format!("invalid thinking budget: {n}"))),
        },
        Some(LuaValue::Number(n)) if n >= 1.0 && n <= f64::from(u32::MAX) => {
            ThinkingConfig::Budget(n as u32)
        }
        Some(LuaValue::Number(n)) => {
            return Ok(err_pair(format!("invalid thinking budget: {n}")));
        }
        Some(_) => return Err(mlua::Error::runtime("thinking must be string or number")),
        None => agent_ctx.opts.thinking,
    };

    let (sub_tx, sub_rx) = flume::unbounded::<Envelope>();
    let sub_event_tx = EventSender::new(sub_tx, agent_ctx.event_tx.run_id());
    let parent_tx = agent_ctx.event_tx.clone();
    let (answer_tx, answer_rx) = flume::unbounded::<String>();

    let subagent_info: Arc<OnceLock<SubagentInfo>> = Arc::new(OnceLock::new());
    let (usage_tx, usage_rx) = flume::unbounded();

    smol::spawn(relay_session_events(
        sub_rx,
        parent_tx.clone(),
        Arc::clone(&subagent_info),
        usage_tx,
        agent_ctx.live_sink.clone(),
        silent,
    ))
    .detach();

    // Register a cancel trigger so the child token does not fire on drop
    // and kill the subagent at birth. The fallback key gets its own id:
    // it keys `subagent_cancels`, so sharing the session id would make two
    // subagents running at once collide.
    let ui_id = agent_ctx
        .tool_use_id
        .clone()
        .unwrap_or_else(|| format!("session-{}", MakiId::generate()));
    // The subagent's cancel is independent of the parent run's cancel: it is
    // triggered only through the shared `subagent_cancels` map (task_despawn,
    // CancelSubagent, global CancelAll). Deriving it from `agent_ctx.cancel`
    // (the run's token, which the UI fires at normal run end by dropping the
    // run's CancelTrigger) would close every in-flight subagent as soon as the
    // spawning run finishes.
    let (child_trigger, child_cancel) = CancelToken::new();
    // Several sessions can share one `ui_id`, so keep the slot and retire
    // only ours on close instead of clearing the whole key.
    let cancel_slot = agent_ctx
        .subagent_cancels
        .insert(ui_id.clone(), child_trigger);

    let name = name.unwrap_or_default();
    info!(name = %name, model = %model.id, "subagent session opened");

    let commit = Arc::new(Mutex::new(None));
    register_commit_slot(&ui_id, Arc::clone(&commit));

    let (input_tx, input_rx) = flume::unbounded::<String>();
    let status = Arc::new(Mutex::new(SubagentStatus::Running));
    let (done_tx, done_rx) = flume::unbounded::<SubagentRunResult>();

    let driver = SubagentDriver {
        params: AgentParams {
            provider,
            model: model.clone(),
            config: agent_ctx.config.clone(),
            tool_output_lines: maki_config::ToolOutputLines::default(),
            permissions: Arc::clone(&agent_ctx.permissions),
            session_id: agent_ctx.session_id.clone(),
            mailbox: None,
            timeouts: agent_ctx.timeouts,
            file_tracker: FileReadTracker::fresh(),
            prompt_slots: Arc::clone(&agent_ctx.prompt_slots),
            modes: Arc::clone(&agent_ctx.modes),
            subagent_cancels: Arc::new(CancelMap::new()),
            registry: Arc::clone(maki_agent::tools::ToolRegistry::global_arc()),
            audience,
            question_mode: agent_ctx.question_mode,
            model_policy: Arc::clone(&agent_ctx.model_policy),
        },
        system: system.unwrap_or_default(),
        tools: tools_json,
        thinking,
        fast,
        mcp: agent_ctx
            .mcp
            .as_ref()
            .filter(|_| mcp_enabled)
            .map(McpSession::fresh),
        history: History::new(Vec::new()),
        sub_event_tx,
        child_cancel,
        answer_rx: Arc::new(AsyncMutex::new(answer_rx)),
        answer_tx: Some(answer_tx),
        parent_cancels: Arc::clone(&agent_ctx.subagent_cancels),
        ui_id: ui_id.clone(),
        cancel_slot,
        parent_event_tx: parent_tx.clone(),
        subagent_info: Arc::clone(&subagent_info),
        local_tools: Arc::new(local_map),
        name: name.clone(),
        usage: TokenUsage::default(),
        usage_rx,
        start: Instant::now(),
        commit,
        input_tx: Some(input_tx.clone()),
        replied: false,
        closed: false,
    };

    smol::spawn(subagent_driver(
        driver,
        input_rx,
        Arc::clone(&status),
        done_tx,
    ))
    .detach();

    let sess = lua.create_userdata(LuaSession {
        id: ui_id,
        input_tx,
        status,
        done_rx,
        parent_cancels: Arc::clone(&agent_ctx.subagent_cancels),
        cancel_slot,
    })?;
    Ok((Some(sess), None))
}

lua_table! {
    /// Subagent primitives for plugins that need to talk to an LLM.
    ///
    /// This module gives you the building blocks: resolve which model to use,
    /// build a system prompt, list available tools, call a tool directly, or
    /// open a full session with its own conversation history.
    ///
    /// Policy like retries, validation, and concurrency lives in the calling
    /// plugin, not here.
    ///
    /// ```lua
    /// local tools = maki.agent.tools(ctx, { audience = "general_sub" })
    /// local sess = maki.agent.session(ctx, {
    ///   system = "You are a helpful assistant.",
    ///   tools = tools,
    /// })
    /// local r = sess:prompt("Hello!")
    /// print(r.text)
    /// sess:close()
    /// ```
    "maki.agent" => pub(crate) fn create_agent_table(), DOCS [
        resolve_model, system_prompt, tools, call_tool, session, report_task_result,
    ]
}

/// Must use `call_async`, not `call`: callbacks that yield (highlight,
/// markdown) hit the C-call boundary otherwise.
struct LiveCallbacks<'a> {
    tool: &'a str,
    on_buf: Option<Function>,
    on_ann: Option<Function>,
    on_usage: Option<Function>,
}

impl LiveCallbacks<'_> {
    async fn deliver(&self, ev: ToolLive) {
        let res = match ev {
            ToolLive::Buf(buf) => call_opt(&self.on_buf, BufHandle::foreign(buf)).await,
            ToolLive::Annotation(ann) => call_opt(&self.on_ann, ann).await,
            ToolLive::Usage(usage) => call_opt(&self.on_usage, usage).await,
        };
        if let Some(Err(e)) = res {
            tracing::warn!(tool = self.tool, error = %e, "call_tool callback failed");
        }
    }
}

async fn call_opt(f: &Option<Function>, arg: impl IntoLuaMulti) -> Option<LuaResult<()>> {
    match f {
        Some(f) => Some(f.call_async::<()>(arg).await),
        None => None,
    }
}

/// Like `interpreter_bridge::dispatch`, but keeps the full `ToolDoneEvent`
/// (the annotation lives there) and feeds live events to `cbs` while the
/// child runs.
async fn dispatch_racing_live(
    tctx: &ToolContext,
    name: &str,
    input: &JsonValue,
    rx: Option<flume::Receiver<ToolLive>>,
    cbs: &LiveCallbacks<'_>,
) -> ToolDoneEvent {
    let run = tool_dispatch::run(
        &tctx.registry,
        tctx.mcp.as_ref(),
        String::new(),
        name,
        input,
        tctx,
        Emit::Silent,
    );
    let Some(rx) = rx else {
        return run.await;
    };
    let mut run = pin!(run);
    loop {
        match select(run.as_mut(), pin!(rx.recv_async())).await {
            Either::Left((done, _)) => {
                while let Ok(ev) = rx.try_recv() {
                    cbs.deliver(ev).await;
                }
                return done;
            }
            Either::Right((Ok(ev), _)) => cbs.deliver(ev).await,
            // The sender is gone but no result arrived: just wait for the run.
            Either::Right((Err(_), _)) => return run.await,
        }
    }
}

/// Owns a subagent's history and run loop, driven in the background.
/// The driver lives off the main agent's call stack so the main agent can
/// keep working while a subagent runs, and can queue more messages to it.
struct SubagentDriver {
    params: AgentParams,
    system: String,
    tools: JsonValue,
    thinking: ThinkingConfig,
    fast: bool,
    /// Fresh per session so `tool_search` loads never leak between a
    /// subagent and its parent.
    mcp: Option<McpSession>,
    history: History,
    sub_event_tx: EventSender,
    child_cancel: maki_agent::cancel::CancelToken,
    answer_rx: Arc<AsyncMutex<flume::Receiver<String>>>,
    answer_tx: Option<flume::Sender<String>>,
    parent_cancels: Arc<CancelMap<String>>,
    /// Stable identity for UI, cancel, and history. Falls back to a synthetic
    /// id for workflow-mode sessions (no model-issued tool call exists).
    /// Shared with any sibling session the same tool call opened.
    ui_id: String,
    /// Which registration under [`ui_id`](Self::ui_id) is ours.
    cancel_slot: CancelSlot,
    parent_event_tx: EventSender,
    subagent_info: Arc<OnceLock<SubagentInfo>>,
    local_tools: LocalTools,
    name: String,
    usage: TokenUsage,
    usage_rx: flume::Receiver<TokenUsage>,
    start: Instant,
    /// Structured-output commit slot, surfaced as `captured` on completion.
    commit: Arc<Mutex<Option<JsonValue>>>,
    /// Queue used to submit further messages to this subagent (also carried
    /// on `SubagentInfo` so the UI can route tab submits to it).
    input_tx: Option<flume::Sender<String>>,
    /// Whether a completed run has already surfaced its history to the parent,
    /// so `close` does not re-emit a duplicate reply to the main agent.
    replied: bool,
    closed: bool,
}

impl SubagentDriver {
    /// Surface this subagent's transcript to the parent so its latest reply
    /// reaches the main agent. Idempotent: only the first completed run after a
    /// spawn delivers a reply; `close` uses the same guard to avoid re-queueing
    /// the same text when the user eventually despawns the subagent.
    fn emit_history(&mut self) {
        if self.replied {
            return;
        }
        self.replied = true;
        let _ = self.parent_event_tx.send(AgentEvent::SubagentHistory {
            tool_use_id: self.ui_id.clone(),
            messages: self.history.as_slice().to_vec(),
        });
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.parent_cancels.retire(&self.ui_id, self.cancel_slot);
        if !self.replied {
            let messages =
                std::mem::replace(&mut self.history, History::new(Vec::new())).into_vec();
            let _ = self.parent_event_tx.send(AgentEvent::SubagentHistory {
                tool_use_id: self.ui_id.clone(),
                messages,
            });
        }
        info!(
            name = %self.name,
            duration_ms = self.start.elapsed().as_millis() as u64,
            input_tokens = self.usage.total_input(),
            output_tokens = self.usage.output,
            "subagent session closed",
        );
    }

    /// Run one user message to completion and surface the result. The
    /// structured-output / summary nudge policy lives in the calling plugin
    /// (which queues nudges via `send`) so `task_policy` can observe it.
    async fn run_one(&mut self, message: String) -> SubagentRunResult {
        if self.subagent_info.get().is_none() {
            let _ = self.subagent_info.set(SubagentInfo {
                parent_tool_use_id: self.ui_id.clone(),
                name: self.name.clone(),
                prompt: Some(message.clone()),
                model: Some(self.params.model.spec()),
                answer_tx: self.answer_tx.take(),
                input_tx: self.input_tx.clone(),
            });
        }
        self.run_agent(message).await
    }

    async fn run_agent(&mut self, message: String) -> SubagentRunResult {
        let history_len = self.history.len();
        set_active_commit(Some(Arc::clone(&self.commit)));
        let result = self.run_agent_inner(message, history_len).await;
        set_active_commit(None);
        result
    }

    async fn run_agent_inner(&mut self, message: String, history_len: usize) -> SubagentRunResult {
        let mut agent = Agent::new(
            self.params.clone(),
            AgentRunParams {
                history: &mut self.history,
                system: self.system.clone(),
                event_tx: self.sub_event_tx.clone(),
                tools: self.tools.clone(),
            },
        )
        .with_user_response_rx(Arc::clone(&self.answer_rx))
        .with_cancel(self.child_cancel.clone())
        .with_mcp(self.mcp.clone())
        .with_local_tools(Arc::clone(&self.local_tools));

        let input = AgentInput {
            message,
            mode: AgentMode::Build,
            images: Vec::new(),
            preamble: Vec::new(),
            thinking: self.thinking,
            fast: self.fast,
            workflow: false,
            prompt: None,
        };
        let error = match agent.run(input).await {
            Ok(DoneReason::Cancelled) => Some(CANCELLED_MSG.to_owned()),
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        };
        drop(agent);
        // Waiting here doubles as an ordering barrier: the relay reaches `Done` only
        // after every `TurnComplete`, so all our `ToolLive::Usage` messages sit in the
        // live channel before `dispatch_racing_live` drains it for the last time.
        match self.usage_rx.recv_async().await {
            Ok(usage) => self.usage += usage,
            Err(_) => tracing::warn!(
                name = %self.name,
                "subagent usage tracker stopped, token counts may lag"
            ),
        }

        let text = self.history.as_slice()[history_len.min(self.history.len())..]
            .iter()
            .rfind(|m| matches!(m.role, Role::Assistant))
            .and_then(|m| {
                m.content.iter().find_map(|b| match b {
                    ContentBlock::Text { text } if text != EMPTY_RESPONSE_MARKER => {
                        Some(text.as_str())
                    }
                    _ => None,
                })
            })
            .map_or_else(String::new, str::to_owned);

        SubagentRunResult {
            text,
            captured: self.commit.lock().unwrap().take(),
            usage: self.usage,
            error,
        }
    }
}

/// Background driver: owns the run loop so one message no longer blocks the
/// main agent. The status slot is the single source of truth for `status()`.
async fn subagent_driver(
    mut driver: SubagentDriver,
    input_rx: flume::Receiver<String>,
    status: Arc<Mutex<SubagentStatus>>,
    done_tx: flume::Sender<SubagentRunResult>,
) {
    loop {
        let cancel = driver.child_cancel.clone();
        let recv_fut = input_rx.recv_async();
        let cancel_fut = async move { cancel.cancelled().await };
        let next = select(Box::pin(recv_fut), Box::pin(cancel_fut)).await;
        let message = match next {
            Either::Left((Ok(message), _)) => message,
            Either::Left((Err(_), _)) | Either::Right(((), _)) => break,
        };
        if driver.child_cancel.is_cancelled() {
            break;
        }
        *status.lock().unwrap() = SubagentStatus::Running;
        let result = driver.run_one(message).await;
        if driver.child_cancel.is_cancelled() {
            break;
        }
        *status.lock().unwrap() = SubagentStatus::Done(result.clone());
        let _ = done_tx.send(result);
        driver.emit_history();
    }
    unregister_commit_slot(&driver.ui_id);
    driver.close();
    *status.lock().unwrap() = SubagentStatus::Closed;
}

struct LuaSession {
    id: String,
    input_tx: flume::Sender<String>,
    status: Arc<Mutex<SubagentStatus>>,
    done_rx: flume::Receiver<SubagentRunResult>,
    parent_cancels: Arc<CancelMap<String>>,
    cancel_slot: CancelSlot,
}

impl Drop for LuaSession {
    fn drop(&mut self) {
        // Firing the child cancel token (by retiring the still-active slot)
        // wakes the driver even if it is parked on the input queue, so it
        // flushes history and retires its own side of the entry.
        self.parent_cancels.retire(&self.id, self.cancel_slot);
    }
}

/// Send a message to the subagent and wait for its full response. The agent
/// loop runs to completion, calling tools as needed. Conversation history is
/// kept across calls, so you can have a multi-turn conversation. This is a
/// blocking compatibility wrapper over `send` + the completion notifier; the
/// async `send`/`status` pair is preferred for background work.
///
/// The returned table has fields: `text` (string), `duration_ms` (integer),
/// `input_tokens` (integer), `output_tokens` (integer). `text` is an empty
/// string when the subagent produced no text block (e.g. it only called
/// tools).
///
/// @param message string User message to send.
/// @return (table?, string?) Result table on success, or `(nil, err)` on
/// failure. A run cut short after streaming some text hands you both: the
/// error and a `{ text = <what it streamed> }` table.
/// @example
/// local r, err = sess:prompt("What files are in this project?")
/// if err then error(err) end
/// print(r.text)
/// print(r.input_tokens .. " input, " .. r.output_tokens .. " output tokens")
#[lua_fn]
async fn prompt(
    lua: Lua,
    this: mlua::UserDataRef<LuaSession>,
    message: String,
) -> LuaResult<Pair<Table>> {
    let input_tx = this.input_tx.clone();
    let done_rx = this.done_rx.clone();
    drop(this);
    if input_tx.send(message).is_err() {
        return Ok((None, Some(SESSION_CLOSED_ERR.to_owned())));
    }
    match done_rx.recv_async().await {
        Err(_) => Ok((None, Some(SESSION_CLOSED_ERR.to_owned()))),
        Ok(result) => build_prompt_result(&lua, result),
    }
}

fn build_prompt_result(lua: &Lua, result: SubagentRunResult) -> LuaResult<Pair<Table>> {
    if let Some(e) = &result.error {
        let tbl = if result.text.is_empty() {
            None
        } else {
            let t = lua.create_table()?;
            t.set("text", result.text)?;
            Some(t)
        };
        return Ok((tbl, Some(e.clone())));
    }
    let tbl = lua.create_table()?;
    tbl.set("text", result.text)?;
    tbl.set("input_tokens", result.usage.total_input())?;
    tbl.set("output_tokens", result.usage.output)?;
    if let Some(captured) = &result.captured {
        tbl.set("captured", json_to_lua(lua, captured)?)?;
    }
    Ok((Some(tbl), None))
}

/// Non-blocking: enqueue a message to the subagent's driver and return
/// immediately. Poll `status()` for the result.
///
/// @param message string User message to send.
/// @return (boolean?, string?) `true` on success, or `(nil, err)` if the session is closed.
#[lua_fn]
async fn send(
    _lua: Lua,
    this: mlua::UserDataRef<LuaSession>,
    message: String,
) -> LuaResult<Pair<bool>> {
    let input_tx = this.input_tx.clone();
    let status = Arc::clone(&this.status);
    drop(this);
    if matches!(*status.lock().unwrap(), SubagentStatus::Closed) {
        return Ok((None, Some(SESSION_CLOSED_ERR.to_owned())));
    }
    input_tx
        .send(message)
        .map_err(|_| mlua::Error::runtime(SESSION_CLOSED_ERR))?;
    Ok((Some(true), None))
}

/// Read the subagent's current status without blocking.
///
/// @return (table?) Table with `status` ("running" | "done" | "closed"), and
///   when done, `result` `{ text, input_tokens, output_tokens, duration_ms, captured? }`
///   and possibly `error`.
#[lua_fn]
async fn status(lua: Lua, this: mlua::UserDataRef<LuaSession>) -> LuaResult<Pair<Table>> {
    let status = Arc::clone(&this.status);
    drop(this);
    let tbl = lua.create_table()?;
    match &*status.lock().unwrap() {
        SubagentStatus::Running => {
            tbl.set("status", "running")?;
        }
        SubagentStatus::Closed => {
            tbl.set("status", "closed")?;
        }
        SubagentStatus::Done(result) => {
            tbl.set("status", "done")?;
            let r = lua.create_table()?;
            r.set("text", result.text.clone())?;
            r.set("input_tokens", result.usage.total_input())?;
            r.set("output_tokens", result.usage.output)?;
            if let Some(captured) = &result.captured {
                r.set("captured", json_to_lua(&lua, captured)?)?;
            }
            tbl.set("result", r)?;
            if let Some(e) = &result.error {
                tbl.set("error", e.clone())?;
            }
        }
    }
    Ok((Some(tbl), None))
}

/// Close the session and flush its history back to the parent agent. You can
/// call this multiple times safely. If you forget, it runs automatically when
/// the session is garbage collected. Firing the cancel token wakes the driver
/// so it flushes, even if it is parked on the input queue.
///
/// @return
#[lua_fn]
async fn close(_lua: Lua, this: mlua::UserDataRef<LuaSession>) -> LuaResult<()> {
    this.parent_cancels.retire(&this.id, this.cancel_slot);
    Ok(())
}

/// Return this session's stable id, used as a `task_id` by the task plugin.
///
/// @return (string) The session's id.
#[lua_fn]
fn session_id(lua: &Lua, this: &LuaSession) -> LuaResult<String> {
    let _ = lua;
    Ok(this.id.clone())
}

lua_class! {
    /// A subagent session with its own conversation history.
    ///
    /// Create one with `maki.agent.session()`, then send messages with
    /// `:prompt()` or `:send()`. The session remembers previous turns, so you
    /// can have a multi-step conversation. Call `:close()` when you are done,
    /// or let garbage collection handle it.
    "maki.agent.Session" => LuaSession, SESSION_DOCS [prompt, send, status, close, session_id]
}

/// Commit a structured-output result from the currently-running subagent
/// driver. The task plugin's `structured_output` local tool validates the
/// value in Lua, then routes it here so the background driver can surface it
/// as `captured` on completion.
///
/// @param value table The validated result to commit.
/// @return (boolean?, string?) `true` when a session is active, else `(nil, err)`.
#[lua_fn]
fn report_task_result(lua: &Lua, value: LuaValue) -> LuaResult<Pair<bool>> {
    with_active_commit(|slot| {
        let json = lua_to_json(lua, &value).map_err(|e| mlua::Error::runtime(e.to_string()))?;
        *slot.lock().unwrap() = Some(json);
        Ok((Some(true), None))
    })
    .unwrap_or(Ok((None, Some(SESSION_CLOSED_ERR.to_owned()))))
}

static ACTIVE_COMMIT: OnceLock<Mutex<Option<CommitSlot>>> = OnceLock::new();

fn with_active_commit<R>(f: impl FnOnce(&CommitSlot) -> R) -> Option<R> {
    let guard = ACTIVE_COMMIT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap();
    guard.as_ref().map(f)
}

fn set_active_commit(slot: Option<CommitSlot>) {
    let mut guard = ACTIVE_COMMIT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap();
    *guard = slot;
}

/// Weak Lua ref avoids a reference cycle when the session is stored in userdata.
fn call_local_tool(
    weak: &mlua::WeakLua,
    f: &Function,
    input: &JsonValue,
) -> Result<String, String> {
    let lua = weak.try_upgrade().ok_or("Lua runtime shut down")?;
    let arg = json_to_lua(&lua, input).map_err(|e| e.to_string())?;
    let values = f.call::<mlua::MultiValue>(arg).map_err(|e| e.to_string())?;
    lua_tool_result(values)
}

#[cfg(test)]
mod tests {
    use maki_agent::tools::test_support::stub_ctx;
    use maki_agent::{DoneReason, TurnCompleteEvent};
    use maki_providers::Message;
    use serde_json::json;

    use super::*;

    /// `inherit_provider` must select the reuse branch even when `model_spec`
    /// is present, so a test-side wrapper can drive a real spawned session
    /// through the parent provider instead of building one from the spec
    /// (which would hit the network).
    #[test]
    fn inherit_provider_selects_reuse_branch_over_model_spec() {
        let ctx = stub_ctx(&AgentMode::Build);
        let parent_model = ctx.model.spec();
        let parent_provider = Arc::clone(&ctx.provider);
        let agent = AgentContext::from(&ctx);

        let (model, provider) = smol::block_on(build_session_provider(
            &Some("anthropic/claude-opus-4-20250514".into()),
            true,
            &agent,
        ))
        .unwrap();

        assert!(
            Arc::ptr_eq(&provider, &parent_provider),
            "must reuse parent provider"
        );
        assert_eq!(model.spec(), parent_model, "must reuse parent model");
    }

    #[test]
    fn absent_model_spec_reuses_parent_provider() {
        let ctx = stub_ctx(&AgentMode::Build);
        let parent_provider = Arc::clone(&ctx.provider);
        let agent = AgentContext::from(&ctx);

        let (_, provider) = smol::block_on(build_session_provider(&None, false, &agent)).unwrap();
        assert!(Arc::ptr_eq(&provider, &parent_provider));
    }

    fn call(src: &str, input: JsonValue) -> Result<String, String> {
        let lua = Lua::new();
        let f: Function = lua.load(src).eval().unwrap();
        call_local_tool(&lua.weak(), &f, &input)
    }

    #[test]
    fn local_tool_handler_result_conventions() {
        let input = json!({"x": "1"});
        assert_eq!(
            call("function(v) return 'ok:' .. v.x end", input.clone()),
            Ok("ok:1".into())
        );
        assert_eq!(
            call("function() return nil, 'bad' end", input.clone()),
            Err("bad".into())
        );
        assert_eq!(
            call("function() end", input.clone()),
            Err(crate::api::util::convert::NIL_TOOL_RESULT_ERR.into())
        );
        let raised = call("function() error('boom') end", input.clone()).unwrap_err();
        assert!(raised.contains("boom"), "got: {raised}");
        let wrong = call("function() return 42 end", input).unwrap_err();
        assert!(wrong.contains("expected string"), "got: {wrong}");
    }

    const RUN_ID: u64 = 7;
    const PARENT_ID: &str = "task-1";
    const IGNORED_ERROR: &str = "handled by the session caller";
    const DONE_USAGE: TokenUsage = tokens(150, 30);

    const fn tokens(input: u32, output: u32) -> TokenUsage {
        TokenUsage {
            input,
            output,
            cache_creation: 0,
            cache_read: 0,
        }
    }

    fn envelope(event: AgentEvent) -> Envelope {
        Envelope {
            event,
            subagent: None,
            run_id: RUN_ID,
        }
    }

    fn turn(usage: TokenUsage, cost: f64) -> AgentEvent {
        AgentEvent::TurnComplete(Box::new(TurnCompleteEvent {
            message: Message::default(),
            usage,
            model: "test-model".into(),
            cost: Some(cost),
            context_size: None,
        }))
    }

    #[test]
    fn relay_session_events_reports_live_usage_and_done_total() {
        let (sub_tx, sub_rx) = flume::unbounded();
        let (parent_raw_tx, parent_rx) = flume::unbounded();
        let subagent_info = Arc::new(OnceLock::new());
        subagent_info
            .set(SubagentInfo {
                parent_tool_use_id: PARENT_ID.into(),
                name: "research".into(),
                prompt: None,
                model: None,
                answer_tx: None,
                input_tx: None,
            })
            .unwrap();
        let (usage_tx, usage_rx) = flume::unbounded();
        let (live_tx, live_rx) = flume::unbounded();

        for event in [
            turn(tokens(100, 20), 0.25),
            turn(tokens(50, 10), 0.5),
            AgentEvent::Error {
                message: IGNORED_ERROR.into(),
            },
            AgentEvent::Done {
                usage: DONE_USAGE,
                num_turns: 2,
                reason: DoneReason::EndTurn,
            },
        ] {
            sub_tx.send(envelope(event)).unwrap();
        }
        drop(sub_tx);

        smol::block_on(relay_session_events(
            sub_rx,
            EventSender::new(parent_raw_tx, RUN_ID),
            subagent_info,
            usage_tx,
            Some(live_tx),
            false,
        ));

        let live = live_rx
            .drain()
            .map(|event| match event {
                ToolLive::Usage(usage) => usage,
                _ => panic!("relay must only publish usage"),
            })
            .collect::<Vec<_>>();
        let expected = [
            tokens(100, 20).format_sum_cost(Some(0.25)),
            tokens(50, 10).format_sum_cost(Some(0.75)),
        ];
        assert_eq!(live, expected);
        assert_eq!(usage_rx.try_recv(), Ok(DONE_USAGE));

        let forwarded = parent_rx.drain().collect::<Vec<_>>();
        assert_eq!(forwarded.len(), expected.len());
        assert!(forwarded.iter().all(|envelope| {
            matches!(envelope.event, AgentEvent::TurnComplete(_))
                && envelope
                    .subagent
                    .as_ref()
                    .is_some_and(|info| info.parent_tool_use_id == PARENT_ID)
        }));
    }

    #[test]
    fn commit_slot_is_reachable_while_active() {
        let lua = Lua::new();
        let slot: Arc<Mutex<Option<JsonValue>>> = Arc::new(Mutex::new(None));
        // No active session yet: report_task_result fails.
        let (res, err) = report_task_result(&lua, LuaValue::Integer(1))
            .map_err(|e| e.to_string())
            .unwrap();
        assert!(err.is_some(), "no active commit slot must error");
        let _ = res;

        set_active_commit(Some(Arc::clone(&slot)));
        let (ok, err) = report_task_result(&lua, LuaValue::Integer(42)).unwrap();
        assert!(ok == Some(true), "commit must succeed while active");
        assert!(err.is_none());
        assert_eq!(*slot.lock().unwrap(), Some(json!(42)));
        set_active_commit(None);
    }

    #[test]
    fn commit_registry_registers_and_unregisters_by_id() {
        let slot: Arc<Mutex<Option<JsonValue>>> = Arc::new(Mutex::new(None));
        register_commit_slot("session-1", Arc::clone(&slot));
        assert!(commit_registry().lock().unwrap().contains_key("session-1"));
        unregister_commit_slot("session-1");
        assert!(!commit_registry().lock().unwrap().contains_key("session-1"));
    }
}
