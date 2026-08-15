-- Mode-scoped `plan_submit` tool: surfaces the plan in the TUI for review and
-- offers accept / refine / cancel. Requires plan mode; registered to load only
-- when plan's toolset includes it (see plugins/mode_plan_override).
-- Enable with:
--   [plugins.plan_submit_tool]
--   enabled = true

local DEFAULT_PLAN = "plan.md"

-- Current mode id from the UI. With no UI (headless/tests) `mode.get()` returns
-- (nil, err), which we treat as "not in plan mode" so the tool stays gated.
local function current_mode()
  local mode, err = maki.api.mode.get()
  if err then
    return nil
  end
  return mode
end

local function split_lines(text)
  local lines = {}
  for line in (text .. "\n"):gmatch("(.-)\n") do
    lines[#lines + 1] = line
  end
  return lines
end

local function handler(input, _ctx)
  if current_mode() ~= "plan" then
    return {
      llm_output = "plan_submit is only available in plan mode",
      is_error = true,
    }
  end

  local path = input.path or DEFAULT_PLAN
  local text, read_err = maki.fs.read(path)
  if not text then
    return { llm_output = "error: cannot read plan file " .. path .. ": " .. read_err, is_error = true }
  end
  local trimmed = text:gsub("%s+", "")
  if trimmed == "" then
    return { llm_output = "error: plan file " .. path .. " is empty", is_error = true }
  end

  local buf = maki.ui.buf()
  buf:set_lines(split_lines(text))

  local win = maki.ui.open_win(buf, {
    title = "Plan",
    width = "70%",
    height = "80%",
    cursor_line = false,
    footer = {
      { "Enter", "accept" },
      { "r", "refine" },
      { "q", "cancel" },
    },
  })

  local decision = "refine"
  while true do
    local ev = win:recv()
    if not ev then
      break
    end
    if ev.type == "key" then
      if ev.key == "enter" then
        decision = "accept"
        break
      elseif ev.key == "r" then
        decision = "refine"
        break
      elseif ev.key == "q" or ev.key == "esc" then
        decision = "cancel"
        break
      end
    end
  end
  win:close()

  if decision == "cancel" then
    return { llm_output = "Plan review cancelled by the user." }
  end
  if decision == "refine" then
    return {
      llm_output = "The user wants you to keep refining the plan in "
        .. path
        .. ". Do NOT start implementation. Revise the plan based on your own review, then call plan_submit again when it is ready.",
    }
  end

  local _, sess_err = maki.session.prompt("implement the plan at " .. path)
  if sess_err then
    return { llm_output = "error handing off to implementation: " .. sess_err, is_error = true }
  end

  -- Print the plan back to the model too, so the accept path keeps it in
  -- context for the follow-up implementation turn.
  return { llm_output = text, format = "markdown" }
end

maki.api.register_tool({
  name = "plan_submit",
  description = [[Submit the finished plan for user review in the interactive UI.

Reads the plan file and opens a window showing it. The user can accept (hands
off to implementation in the current session), refine (keep planning), or
cancel. Call it only once the plan is complete and you are ready to begin
implementation. Requires plan mode.]],
  kind = "execute",
  schema = {
    type = "object",
    properties = {
      path = {
        type = "string",
        description = "Path to the plan file (default: " .. DEFAULT_PLAN .. ")",
      },
    },
  },
  handler = handler,
})
