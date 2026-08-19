local ListPicker = require("maki.list_picker")

local renderers = {}
local candidate
local committed
local previous_committed
local persisted_name
local last_render
local rollback_pending

local function default_renderer()
  local ok, module = pcall(require, "splash.default")
  if ok and type(module) == "table" and type(module.render) == "function" then
    return module.render
  end
  local standalone = require("splash")
  return standalone.render
end

local function activate_default()
  return default_renderer()
end

local function refresh_renderers()
  renderers = maki.api.collect("splash.gallery")
  renderers.default = {
    label = "Default",
    description = "The built-in maki splash",
    activate = activate_default,
  }
end

local function activate(name)
  refresh_renderers()
  local entry = renderers[name]
  if not entry then
    return nil, "unknown splash: " .. name
  end
  local ok, renderer, err = pcall(entry.activate)
  if not ok then
    return nil, renderer
  end
  if type(renderer) ~= "function" then
    return nil, err or "activation did not return a renderer"
  end
  return renderer
end

local function items()
  refresh_renderers()
  local out = {}
  for name, entry in pairs(renderers) do
    out[#out + 1] = { label = entry.label, detail = entry.description, name = name }
  end
  table.sort(out, function(a, b) return a.label < b.label end)
  return out
end

local function stage(name, renderer)
  candidate = { name = name, renderer = renderer }
end

local function validate(selection)
  if selection.validated then return true end
  local input = last_render or { w = 80, h = 24, t = 0, fade = 1 }
  local ok, frame = pcall(selection.renderer, input.w, input.h, input.t, input.fade)
  if not ok then return nil, frame end
  selection.validated = true
  selection.frame = frame
  return true
end

local function load_preference()
  local state_dir = maki.env.state_dir()
  if not state_dir then return nil end
  local path = maki.fs.joinpath(state_dir, "splash_gallery", "selection.json")
  local content = maki.fs.read(path)
  if not content then return nil end
  local ok, value = pcall(maki.json.decode, content)
  if ok and type(value) == "table" and type(value.name) == "string" then
    return value.name
  end
end

local function persist(name)
  local state_dir = maki.env.state_dir()
  if not state_dir then return true end
  local dir = maki.fs.joinpath(state_dir, "splash_gallery")
  local ok, err = maki.fs.mkdir(dir, { parents = true })
  if not ok and err then return nil, err end
  return maki.fs.atomic_write(maki.fs.joinpath(dir, "selection.json"), maki.json.encode({ name = name }))
end

local function clear_preference()
  local state_dir = maki.env.state_dir()
  if not state_dir then return true end
  return maki.fs.rm(maki.fs.joinpath(state_dir, "splash_gallery", "selection.json"), { force = true })
end

local function save_preference(name)
  local ok, err
  if name == "default" then
    ok, err = clear_preference()
  else
    ok, err = persist(name)
  end
  if ok then persisted_name = name end
  return ok, err
end

local function restore_preference()
  if persisted_name == committed.name then
    rollback_pending = nil
    return true
  end
  local ok, err = save_preference(committed.name)
  if ok then rollback_pending = nil end
  return ok, err
end

local function commit(selection)
  local valid, validation_err = validate(selection)
  if not valid then
    candidate = nil
    return nil, validation_err
  end
  local saved, save_err = save_preference(selection.name)
  if not saved then
    candidate = nil
    return nil, save_err
  end
  previous_committed, committed, candidate = committed, selection, nil
  rollback_pending = nil
  return true
end

local function render(prev, w, h, t, fade)
  last_render = { w = w, h = h, t = t, fade = fade }
  if candidate then
    local pending = candidate
    local ok, frame = pcall(pending.renderer, w, h, t, fade)
    if ok then
      pending.validated = true
      pending.frame = frame
      return frame
    end
    if candidate == pending then candidate = nil end
  end

  local ok, frame = pcall(committed.renderer, w, h, t, fade)
  if ok then return frame end
  if previous_committed then
    committed, previous_committed = previous_committed, nil
    rollback_pending = true
    local rendered, restored_frame = pcall(committed.renderer, w, h, t, fade)
    if rendered then return restored_frame end
  end
  return prev(w, h, t, fade)
end

maki.api.set_slot("splash.render", render)

for _, module_name in ipairs({ "aurora", "caustics", "kaleidoscope", "matrix", "metaballs", "voronoi" }) do
  local ok, module = pcall(require, "splash." .. module_name)
  if ok and type(module) == "table" and type(module.render) == "function" then
    maki.api.register("splash.gallery", module_name, {
      label = module_name,
      description = "Bundled splash renderer",
      activate = function() return module.render end,
    })
  end
end
refresh_renderers()
committed = { name = "default", renderer = default_renderer() }
local saved = load_preference()
persisted_name = saved or "default"
if saved and saved ~= "default" and renderers[saved] then
  local renderer = activate(saved)
  if renderer then
    previous_committed = committed
    committed = { name = saved, renderer = renderer }
  end
elseif saved and saved ~= "default" then
  local ok, err = restore_preference()
  if not ok then maki.ui.flash("splash rollback failed: " .. tostring(err)) end
end

local function command(opts)
  if rollback_pending then
    local restored, restore_err = restore_preference()
    if not restored then maki.ui.flash("splash rollback failed: " .. tostring(restore_err)) end
  end

  local name = opts.fargs[1]
  if name then
    local selected = name:lower()
    local renderer, err
    if selected == "default" then
      renderer = activate_default()
    else
      renderer, err = activate(selected)
    end
    if not renderer then return maki.ui.flash(err) end
    local selection = { name = selected, renderer = renderer }
    local committed_ok, commit_err = commit(selection)
    if not committed_ok then maki.ui.flash("splash selection failed: " .. tostring(commit_err)) end
    return
  end
  local picker_items = items()
  local result = ListPicker.open(picker_items, {
    title = "Splash gallery",
    timeout_ms = 100,
    on_change = function(item)
      local renderer = activate(item.name)
      if renderer then stage(item.name, renderer) end
    end,
  })
  if result and result.type == "choice" then
    local item = picker_items[result.index]
    local renderer, err = activate(item.name)
    if renderer then
      local selection = candidate
      if not selection or selection.name ~= item.name then
        selection = { name = item.name, renderer = renderer }
      end
      local committed_ok, commit_err = commit(selection)
      if not committed_ok then maki.ui.flash("splash selection failed: " .. tostring(commit_err)) end
    else
      candidate = nil
      maki.ui.flash(err)
    end
  else
    candidate = nil
  end
end

maki.api.register_command({
  name = "/splash",
  description = "Preview and select a splash renderer",
  nargs = "?",
  completion = {
    get_items = function()
      local out = { { label = "default", insertion = "default", description = "Reset to the built-in splash" } }
      for _, item in ipairs(items()) do
        if item.name ~= "default" then
          out[#out + 1] = {
            label = item.label,
            insertion = item.name,
            description = item.detail,
          }
        end
      end
      return out
    end,
  },
  handler = command,
})

return { render = render }
