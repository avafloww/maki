# Splash gallery

Ready-made splash screens, bundled into the binary as requireable modules.
This is **not a loadable plugin** (no `init.lua`, not in `DEFAULT_BUILTINS`);
it only adds files to the bundled module search path so any config can load
a skin on demand:

```lua
require("splash.kaleidoscope")
```

Each module self-activates on require (it calls
`maki.api.set_slot("splash.render", ...)`) and also returns `M` with
`M.render(w, h, t, fade)` for custom slots or cyclers.

| Module | What it draws |
|--------|---------------|
| `splash.kaleidoscope` | 10-fold mirror kaleidoscope over a circle-inversion fractal |
| `splash.voronoi` | animated voronoi cells with warm F2-F1 borders |
| `splash.caustics` | deep-water light caustics, four octaves of warped sines |
| `splash.metaballs` | four merging metaballs with a glow contour |
| `splash.aurora` | five noise-meandering northern-light bands |

User files win outside the `splash.*` namespace; bundled modules are tried
before `./lua` files, so a personal `lua/splash/kaleidoscope.lua` would be
shadowed by the bundled one. Write your own skins under plain names
(`lua/myskin.lua`, `require("myskin")`) to stay clear.
