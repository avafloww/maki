-- Opt-in override of the built-in `plan` mode. Enable with:
--   [plugins.mode_plan_override]
--   enabled = true
--
-- Replaces plan with a polytoken-style directive (analyse + write only the
-- plan file), swaps the active toolset to read-only tools + write/edit/
-- plan_submit, and adds /plan and /build slash commands.

local PLAN_FILE = "plan.md"

-- Directive spliced into the system prompt while plan mode is active. It
-- mirrors the built-in plan gating but focuses the model on producing a
-- reviewable artifact, not on implementation.
local PLAN_DIRECTIVE = [[
<system-reminder>
# Plan Mode

CRITICAL: Plan mode ACTIVE. Analyse, search, and read freely, but your only
writable target is the plan file ({plan_path}). Do NOT modify any other file.
Use the write/edit tools ONLY on the plan file.

## Responsibility

Your responsibility is to produce a well-formed plan that accomplishes the
user's goal. The plan must be comprehensive yet concise. Ask the Question tool
freely to resolve ambiguity. Do not start implementing.

## Plan artifact

One document at {plan_path} with these sections:

- Goal
- Implementation Summary
- Implementation Plan (numbered steps)
- Acceptance Criteria (AC.n, each mapping to a named test)
- Test Strategy
- Review Strategy
- Documentation Strategy
- Risks and Blockers

When the plan is complete and the user is ready to proceed, call plan_submit to
surface it in the UI for review before the user accepts implementation.
</system-reminder>
]]

local ok, err = maki.api.mode.define({
  name = "plan",
  label = "[PLAN]",
  system_prompt = PLAN_DIRECTIVE,
  restrict_write_to = PLAN_FILE,
  tools = { "read", "grep", "glob", "write", "edit", "plan_submit" },
  -- tool search / mcp stay available through the per-request MCP injection.
})
if not ok then
  maki.log.warn("mode_plan_override: define failed: " .. err)
end

local function set_mode(name)
  local switched, e = maki.api.mode.set(name)
  if not switched then
    maki.log.warn("mode_plan_override: set(" .. name .. ") failed: " .. e)
  end
end

maki.api.register_command({
  name = "/plan",
  description = "Switch to plan mode (analyse and write only the plan file)",
  handler = function(_args)
    set_mode("plan")
  end,
})

maki.api.register_command({
  name = "/build",
  description = "Switch to build mode (full tool access)",
  handler = function(_args)
    set_mode("build")
  end,
})
