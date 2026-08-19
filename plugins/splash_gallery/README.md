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
`splash.render` layer and switches the renderer called by that layer.

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
shadowed by the bundled one. Write your own skins under plain names
(`lua/myskin.lua`, `require("myskin")`) to stay clear.
