local REGISTRY = "splash"
local SPLASHES = { "aurora", "caustics", "kaleidoscope", "matrix", "metaballs", "voronoi" }

local default = require("splash.default")
maki.api.declare_slot("splash.render", default.render)
maki.store.register(REGISTRY, "default", {
  label = "default",
  description = default.description,
  renderer = default.render,
})

for _, name in ipairs(SPLASHES) do
  local ok, module = pcall(require, "splash." .. name)
  if ok and type(module) == "table" and type(module.render) == "function" then
    maki.store.register(REGISTRY, name, {
      label = name,
      description = module.description,
      renderer = module.render,
    })
  end
end

return { render = default.render }
