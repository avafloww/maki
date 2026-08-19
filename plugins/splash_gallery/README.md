# Splash gallery

Ready-made splash screens, bundled into the binary as requireable modules.
This is a loadable default plugin. It owns `/splash` and exposes the bundled
renderers through a persistent gallery:

Use `/splash` to preview and select a renderer. The modules are also available
for custom slots and cyclers:

```lua
local kaleidoscope = require("splash.kaleidoscope")
local rows = kaleidoscope.render(w, h, t, fade)
```

Requiring a module does not activate it. The gallery keeps one stable
`splash.render` layer and switches the renderer called by that layer. The
standalone `splash` plugin owns the default renderer. The gallery delegates to
the previous slot for `/splash default`.

| Module | What it draws |
|--------|---------------|
| `splash.kaleidoscope` | 10-fold mirror kaleidoscope over a circle-inversion fractal |
| `splash.voronoi` | animated voronoi cells with warm F2-F1 borders |
| `splash.caustics` | deep-water light caustics, four octaves of warped sines |
| `splash.metaballs` | four merging metaballs with a glow contour |
| `splash.aurora` | five noise-meandering northern-light bands |
| `splash.matrix` | green falling-code rain, resets on `SplashShown` |

User files win outside the `splash.*` namespace; bundled modules are tried
before `./lua` files, so a personal `lua/splash/kaleidoscope.lua` would be
shadowed by the bundled one. Write skins under plain names
(`lua/myskin.lua`, `require("myskin")`) to stay clear.

Third-party plugins can contribute an entry:

```lua
local myskin = require("myskin")
maki.api.register("splash.gallery", "myskin", {
  label = "My skin",
  description = "A short description",
  activate = function() return myskin.render end,
})
```

The host removes contributions when their plugin unloads. The gallery
re-resolves active and staged entries when contributions change, so a reload
does not retain callbacks from the old plugin instance.
