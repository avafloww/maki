+++
title = "Modes"
weight = 3
[extra]
group = "Concepts"
+++

# Modes

Maki ships with two agent modes: **build** and **plan**. A mode bundles a badge
in the input bar, a system-prompt snippet the model follows, and, optionally, a
write restriction and its own visible toolset. This page shows what the built-in
modes do, and how a Lua plugin defines a new mode or overrides a built-in one.

## The built-in modes

Tab toggles between them (build is the default).

- **build `[BUILD]`** - the default. Full toolset, no restrictions.
- **plan `[PLAN]`** - analyse and plan. Writes are locked to a single plan
  file, and the model gets a directive telling it never to touch anything else.

Switching to plan mode allocates a plan file under `plans/`. The `write` and
`edit` tools only allow edits to that file while in plan mode.

## What a mode is

Under the hood a mode is a definition in a shared registry:

- **name** (`"build"`, `"plan"`, or a custom id) and a **label** for the badge.
- **system_prompt** - a snippet appended to the system prompt, like the plan
  directive. `{plan_path}` and the other prompt variables are filled in.
- **restrict_write_to** (optional) - when set, every non-matching write is
  blocked, exactly like the plan-file-only rule.
- **tools** (optional) - when set, the model sees *only* this exact toolset for
  that mode. When absent, the mode inherits the default (build) set. This is how
  a tool like `plan_submit` exists only while you are in plan mode.

The built-in `build` and `plan` are pre-registered entries. Overriding one is
the same call as defining a new mode: it fully replaces the definition.

## Defining and overriding modes from Lua

The registry lives on the API as `maki.api.mode`. Define a mode or override a
built-in with `define`:

```lua
maki.api.mode.define({
  name = "audit",                 -- a new custom mode
  label = "[AUDIT]",
  system_prompt = [[You only review code. You never change it.]],
  restrict_write_to = "audit.md",
  tools = { "read", "grep", "glob", "write", "edit" },
})
```

Override the built-in plan mode the same way:

```lua
-- Replaces the built-in plan directive and toolset.
maki.api.mode.define({
  name = "plan",
  label = "[PLAN]",
  system_prompt = function(ctx)
    return "My stricter plan-mode directive, plan file: " .. (ctx.plan_path or "?")
  end,
  tools = { "read", "grep", "glob", "write", "edit", "plan_submit" },
})
```

`system_prompt` may be a string or a function of `{ cwd, plan_path }` returning
a string. Because a definition fully replaces the built-in, a partial override
(for example only `tools`, no `system_prompt`) drops the built-in directive;
supply both when you override.

Other methods:

```lua
maki.api.mode.get()          -- current mode id: "build", "plan", or a custom name
maki.api.mode.set("plan")    -- enter a mode; fails if it is not defined
maki.api.mode.list()         -- all modes as { name, label }
maki.api.mode.reset("plan")  -- drop a plugin override, restore the built-in
maki.api.mode.reset()        -- restore every built-in
```

Switching modes fires the autocmd `ModeChanged` with data `{ mode = "<id>" }`.

## Example: a plan-review workflow

The repositories ship two opt-in example plugins that put this together. They
are bundled but disabled; enable them from `init.lua` or config:

```toml
[plugins.mode_plan_override]
enabled = true

[plugins.plan_submit_tool]
enabled = true
```

- `mode_plan_override` replaces the built-in `plan` mode with a directive that
  focuses on producing a reviewable artifact, restricts writes to the plan file,
  and swaps the toolset to read tools plus `write`/`edit`/`plan_submit`. It also
  adds `/plan` and `/build` slash commands.
- `plan_submit_tool` is a mode-scoped tool: it surfaces the finished plan in a
  TUI window and offers **accept** (hands off to implementation), **refine**
  (keep planning), or **cancel**. It only exists in plan mode because plan's
  toolset lists it.

The built-in `task` tool also grows a `plan_reviewer` subagent type when the
plan override is active: a read-only audit that checks plan shape and
test-to-acceptance-criteria coverage and answers `VERDICT: pass|fail`. It is
only spawnable inside plan mode.

With all three enabled, a typical loop is:

1. Switch to plan mode (`/plan`). The model drafts `plan.md` using the stricter
   directive and the reduced toolset.
2. The model calls `task` with `subagent_type = "plan_reviewer"` to audit the
   plan, then iterates until `VERDICT: pass`.
3. The model calls `plan_submit`; you review the plan in the window and accept.
4. Switch to build mode (`/build`) to implement with the full toolset.

## Persistence and other surfaces

The active mode is persisted with the session, so a custom mode survives a
restart (it falls back to build with a warning if its plugin is not loaded
then). Custom modes also appear in the Agent Client Protocol session modes when
the ACP server is started from a live plugin host.