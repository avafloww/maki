local REGISTRY = "splash"
local SPLASHES = { "aurora", "caustics", "kaleidoscope", "matrix", "metaballs", "voronoi" }

local default = require("splash.default")
maki.api.declare_slot("splash.render", default.render)
maki.api.register(REGISTRY, "default", {
  label = "default",
  description = default.description,
  activate = function()
    return default.render
  end,
})

for _, name in ipairs(SPLASHES) do
  local ok, module = pcall(require, "splash." .. name)
  if ok and type(module) == "table" and type(module.render) == "function" then
    maki.api.register(REGISTRY, name, {
      label = name,
      description = module.description,
      activate = function()
        return module.render
      end,
    })
  end
end

return { render = default.render }
