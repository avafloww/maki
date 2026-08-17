+++
title = "References"
weight = 12
[extra]
group = "Reference"
+++

# `@` References

Type `@` in the input box and Maki opens a completion popup. It does three things: picks a file path, picks a skill, or picks a subagent or model to route the turn through. Each reference has a long prefix, a one-letter short form, and a fixed meaning at submit time.

## Syntax

| Reference | Long form | Short form | What it does |
|---|---|---|---|
| File | `@path/to/file` | - | Stays in the message as plain text. The agent reads the file lazily with its tools when it needs it. |
| Skill | `@skill:name` | `@s:name` | Tells the agent to load that skill with the `skill` tool before answering. |
| Subagent | `@subagent:type` | `@a:type` | Rewrites the turn into a directive: delegate the request to a `task` subagent of the given type. |
| Model | `@model:spec` | `@m:spec` | Paired with a subagent, sets that subagent's model tier. On its own, switches the session model before the run starts. |

Prefixes are case-insensitive, so `@SKILL:pdf` and `@s:pdf` are the same. A bare `@skill:` with nothing after it is not a reference yet, it is just the popup waiting for you to type.

File references are the odd one out. They are never expanded or stripped. Maki does not inject file contents at submit time. The agent gets the path as text and decides when to read it. This keeps big files out of context until they are actually needed.

## Subagents

`@subagent:` (short `@a:`) lists the subagent types the `task` plugin accepts:

- `research` - read-only search and summarize.
- `general` - can modify files.
- `plan_reviewer` - read-only plan audit, plan mode only.

The list is filtered by your current mode. In plan mode, `general` is hidden. Outside plan mode, `plan_reviewer` is hidden. You can never pick a type the plugin would reject.

Submitting `@subagent:research review this package` sends the agent a directive instead of the raw text. The directive names the `task` tool, sets `subagent_type "research"`, and puts your stripped request (`review this package`) in as the delegation body. The agent then spawns the subagent and relays its result.

If you add more than one `@subagent:` in a message, the first wins and the rest are ignored.

## Models

`@model:` (short `@m:`) lists model specs from the available-models slot, the same list the `/model` picker uses.

A model reference next to a subagent reference becomes that subagent's model. Maki treats the spec two ways:

- A literal tier name (`weak`, `medium`, `strong`) sets the subagent's `model_tier`, capped at the current tier.
- Any other spec is an exact-model request. With the `task` plugin's `allow_model` option on, the subagent runs on that exact model. With it off (the default), the agent asks you how to proceed instead of silently picking a model: fall back to the spec's tier when one is assigned, use the current model, or cancel. Enable `allow_model` in the task plugin config to use exact specs without being asked.

So `@subagent:general @m:weak @skill:pdf fix the report` produces one directive with `subagent_type "general"`, `model_tier "weak"`, and an instruction for the subagent to load the `pdf` skill first. Skills named next to a subagent go into the subagent's prompt, not the main agent's.

## Standalone model switch

A `@model:spec` with no subagent reference in the message switches the session model before the run starts. It emits the same action as the `/model` picker, then runs your request with the new model. The token is stripped from the message that reaches the agent.

A bare tier name (`weak`, `medium`, `strong`) resolves to whatever model you have assigned to that tier (with `/model`). So `@model:weak` switches to your weak-tier model, the same model `@m:weak` would give a subagent. If no model is assigned to that tier, the switch fails loudly with an "Invalid model" flash.

If the message reduces to only a model reference (no other text, no skills), Maki switches the model and does not start a run.

## Skills without a subagent

`@skill:pdf @skill:csv summarize this` becomes a short directive telling the agent to load `pdf` and `csv` via the `skill` tool, followed by your stripped request. See [Skills](/docs/skills/) for how skills work.

## What passes through

Not every `@` is a reference. Maki only treats `@` as a reference start when it begins a token, meaning nothing but whitespace comes before it on the line. So `foo@bar`, email addresses, and unknown prefixes like `@nothing:whatever` are left alone and sent to the agent verbatim.
