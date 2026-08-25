+++
title = "ACP"
weight = 22
[extra]
group = "Guides"
+++

# ACP (Agent Client Protocol)

Run Makima inside your editor. `makima acp` starts an [ACP](https://agentclientprotocol.com/) server over stdio, so any ACP-capable editor (like [Zed](https://zed.dev/)) can drive Makima as its coding agent.

```bash
makima acp
```

## Zed setup

Add Makima as a custom agent in Zed's `settings.json`:

```json
"agent_servers": {
  "Makima": {
    "default_config_options": {
      "model": "deepseek/deepseek-v4-flash"
    },
    "type": "custom",
    "command": "makima",
    "args": ["acp"],
    "env": {}
  }
}
```

The `model` value is a `provider/model-id` spec, same format as `makima --model`.

## What works

- **Sessions persist.** Loading a session replays the full conversation in the editor, so you can resume where you left off. A session that is open in another terminal cannot be loaded.
- **Model switching.** Pick a model from the editor's dropdown, mid-session. All configured providers show up. Providers that list their models over the wire (OpenRouter and friends) are discovered in the background, so the dropdown keeps filling up for a moment after the session starts, one provider at a time.
- **Modes.** Switch between build (full access) and plan (plan-file writes only) from the editor.
- **Permissions.** Tool permission prompts appear in the editor: allow or reject, once or always.
- **Questions.** The `question` tool becomes a native form in the editor (ACP elicitation). If the client does not support elicitation, the tool is dropped and the model asks in plain text.
- **Live tool calls.** Tool progress streams as it happens, including sub-agents and batched calls.
- **Images and context.** Prompts can include images and editor-attached files.
- **Slash commands.** The editor receives the active command list and updates when plugin or MCP registrations change. A known leading slash command runs before model input. Unknown slash-prefixed text remains a model prompt. Known commands cannot include images.

ACP supports `/model <spec>` through its model-control path. TUI-only built-ins remain visible but return an unsupported-frontend error when invoked. Custom Markdown commands, MCP prompts, and Lua commands use the same registry and collision rules as the TUI. See [Commands](/docs/commands/).

Authentication, providers, and permissions come from your normal Makima config. Set up [providers](/docs/providers/) first and ACP sessions just work.

```bash
makima acp
makima acp -m anthropic/claude-sonnet-4-6
makima acp --yolo
makima --no-jit acp
```

`makima acp` only takes `-m` / `--model` and `--yolo` as subcommand flags. Global flags like `--no-jit` must come before the subcommand (`makima --no-jit acp`, not `makima acp --no-jit`).

Plan mode in ACP uses the same state-directory plan files as the TUI (`…/plans/<slug>.md`), not the SDK's `./plan.md`.

A session open in another terminal is locked by it: loading fails until that instance exits, which releases the lock automatically (or a few seconds after it crashes).
