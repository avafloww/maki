local ListPicker = require("maki.list_picker")

local REGISTRY = "splash.gallery"
local renderers = {}
local renderers_revision = -1
local candidate
local committed = { name = "default" }
local previous_committed
local persisted_name
local last_render
local rollback_queued = false

local function refresh_renderers()
  local revision = maki.api.contribution_revision(REGISTRY)
  if revision ~= renderers_revision then
    renderers = maki.api.collect(REGISTRY)
    renderers.default = {
      label = "default",
      description = "The standard Maki splash screen.",
    }
    renderers_revision = revision
  end
end

local function resolve(selection)
  if selection.name == "default" then
    selection.renderer = nil
    selection.revision = maki.api.contribution_revision(REGISTRY)
    return true
  end
  refresh_renderers()
  if selection.revision == renderers_revision and type(selection.renderer) == "function" then
    return true
  end
  local entry = renderers[selection.name]
  if not entry then
    return nil, "unknown splash: " .. selection.name
  end
  local ok, renderer, err = pcall(entry.activate)
  if not ok then
    return nil, renderer
  end
  if type(renderer) ~= "function" then
    return nil, err or "activation did not return a renderer"
  end
  selection.renderer = renderer
  selection.revision = renderers_revision
  selection.validated = nil
  selection.frame = nil
  return true
end

local function items()
  refresh_renderers()
  local out = {}
  for name, entry in pairs(renderers) do
    out[#out + 1] = { label = entry.label, detail = entry.description, name = name }
  end
  table.sort(out, function(a, b)
    return a.label < b.label
  end)
  return out
end

local function item_index(list, name)
  for index, item in ipairs(list) do
    if item.name == name then
      return index
    end
  end
  return 1
end

local function invoke(selection, input)
  local resolved, err = resolve(selection)
  if not resolved then
    return nil, err
  end
  if selection.name == "default" then
    return pcall(input.prev, input.w, input.h, input.t, input.fade)
  end
  return pcall(selection.renderer, input.w, input.h, input.t, input.fade)
end

local function validate(selection)
  if selection.validated then
    return true
  end
  local input = last_render
  if not input then
    if selection.name == "default" then
      return true
    end
    input = { w = 80, h = 24, t = 0, fade = 1 }
  end
  local ok, frame = invoke(selection, input)
  if not ok then
    return nil, frame
  end
  selection.validated = true
  selection.frame = frame
  return true
end

local function load_preference()
  local state_dir = maki.env.state_dir()
  if not state_dir then
    return nil
  end
  local path = maki.fs.joinpath(state_dir, "splash_gallery", "selection.json")
  local content = maki.fs.read(path)
  if not content then
    return nil, false
  end
  local ok, value = pcall(maki.json.decode, content)
  if ok and type(value) == "table" and type(value.name) == "string" then
    return value.name, false
  end
  return nil, true
end

local function persist(name)
  local state_dir = maki.env.state_dir()
  if not state_dir then
    return true
  end
  local path = maki.fs.joinpath(state_dir, "splash_gallery", "selection.json")
  if name == "default" then
    return maki.fs.rm(path, { force = true })
  end
  local dir = maki.fs.joinpath(state_dir, "splash_gallery")
  local ok, err = maki.fs.mkdir(dir, { parents = true })
  if not ok and err then
    return nil, err
  end
  return maki.fs.atomic_write(path, maki.json.encode({ name = name }))
end

local function save_preference(name)
  local ok, err = persist(name)
  if ok then
    persisted_name = name
  end
  return ok, err
end

local function queue_rollback()
  if rollback_queued or persisted_name == committed.name then
    return
  end
  rollback_queued = true
  maki.async.run(function()
    local ok, err = save_preference(committed.name)
    rollback_queued = false
    if not ok then
      maki.ui.flash("splash rollback failed: " .. tostring(err))
    end
  end)
end

local function stage(name)
  local selection = { name = name }
  local resolved, err = resolve(selection)
  if not resolved then
    candidate = nil
    return nil, err
  end
  candidate = selection
  return selection
end

local function cancel()
  candidate = nil
end

local function commit(selection)
  selection = selection or candidate
  if not selection then
    return nil, "no splash selected"
  end
  local valid, validation_err = validate(selection)
  if not valid then
    cancel()
    return nil, validation_err
  end
  local saved, save_err = save_preference(selection.name)
  if not saved then
    cancel()
    return nil, save_err
  end
  previous_committed, committed, candidate = committed, selection, nil
  rollback_queued = false
  return true
end

local function rollback()
  candidate = nil
  if previous_committed then
    committed, previous_committed = previous_committed, nil
  else
    committed = { name = "default" }
  end
  queue_rollback()
end

local function render(prev, w, h, t, fade)
  local input = { prev = prev, w = w, h = h, t = t, fade = fade }
  last_render = input
  if candidate then
    local pending = candidate
    local ok, frame = invoke(pending, input)
    if ok then
      pending.validated = true
      pending.frame = frame
      return frame
    end
    if candidate == pending then
      candidate = nil
    end
  end

  local ok, frame = invoke(committed, input)
  if ok then
    return frame
  end
  rollback()
  local restored, restored_frame = invoke(committed, input)
  if restored then
    return restored_frame
  end
  committed = { name = "default" }
  queue_rollback()
  return prev(w, h, t, fade)
end

maki.api.set_slot("splash.render", render)

local descriptions = {
  aurora = "Drifting northern lights over a dark gradient.",
  caustics = "Deep-water light patterns shimmering across the screen.",
  kaleidoscope = "A mirrored fractal kaleidoscope with tenfold symmetry.",
  matrix = "Green falling-code rain with persistent animated trails.",
  metaballs = "Glowing metaballs that merge and flow together.",
  voronoi = "Animated Voronoi cells with warm glowing borders.",
}

for _, module_name in ipairs({ "aurora", "caustics", "kaleidoscope", "matrix", "metaballs", "voronoi" }) do
  local ok, module = pcall(require, "splash." .. module_name)
  if ok and type(module) == "table" and type(module.render) == "function" then
    maki.api.register(REGISTRY, module_name, {
      label = module_name,
      description = descriptions[module_name],
      activate = function()
        return module.render
      end,
    })
  end
end

local saved, invalid_saved = load_preference()
persisted_name = invalid_saved and "invalid" or saved or "default"
if saved and saved ~= "default" then
  local selection = { name = saved }
  if resolve(selection) then
    previous_committed = committed
    committed = selection
  else
    queue_rollback()
  end
elseif invalid_saved then
  queue_rollback()
end

local function select_and_commit(name)
  if not candidate and committed.name == name then
    return true
  end
  local selection, err = stage(name)
  if not selection then
    return nil, err
  end
  return commit(selection)
end

local function command(opts)
  local name = opts.fargs[1]
  if name then
    local ok, err = select_and_commit(name:lower())
    if not ok then
      maki.ui.flash("splash selection failed: " .. tostring(err))
    end
    return
  end

  local picker_items = items()
  local result = ListPicker.open(picker_items, {
    title = "Splash gallery",
    cursor = item_index(picker_items, committed.name),
    timeout_ms = 100,
    notify_initial = true,
    on_change = function(item)
      local _, err = stage(item.name)
      if err then
        maki.ui.flash("splash preview failed: " .. tostring(err))
      end
    end,
  })
  if result and result.type == "choice" then
    local item = picker_items[result.index]
    local selection = candidate
    if not selection or selection.name ~= item.name then
      selection = stage(item.name)
    end
    local ok, err = commit(selection)
    if not ok then
      maki.ui.flash("splash selection failed: " .. tostring(err))
    end
  else
    cancel()
  end
end

maki.api.register_command({
  name = "/splash",
  description = "Preview and select a splash renderer",
  nargs = "?",
  completion = {
    get_items = function()
      local out = {}
      for _, item in ipairs(items()) do
        out[#out + 1] = {
          label = item.label,
          insertion = item.name,
          description = item.detail,
        }
      end
      return out
    end,
    on_highlight = function(_, item)
      local _, err = stage(item.insertion)
      if err then
        maki.ui.flash("splash preview failed: " .. tostring(err))
      end
    end,
    on_accept = function(_, item)
      local selection = candidate
      if not selection or selection.name ~= item.insertion then
        selection = stage(item.insertion)
      end
      local ok, err = commit(selection)
      if not ok then
        maki.ui.flash("splash selection failed: " .. tostring(err))
      end
    end,
    on_cancel = cancel,
  },
  handler = command,
})

return { render = render }
