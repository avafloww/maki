# Splash screens

Six self-contained `splash.render` overrides you can drop into your config to
try a different home screen. Each file is ordinary drop-in Lua, matching the
matrix-rain pattern: it computes its own theme colors, self-activates on load,
and replaces the whole screen. No Rust changes, no registry, no config keys.

| File | What it draws |
|------|---------------|
| `pentagram.lua`   | a self-intersecting five-pointed star, one full turn every 5 s |
| `flowers.lua`     | ASCII flowers rising from the bottom, swaying as they go up |
| `printer.lua`     | a printer that prints a sheet which grows, then drops next page |
| `tunnel.lua`      | rings of rectangles receding to a point with a depth cue |
| `comets.lua`      | shooting stars with fading trails along fixed diagonals |
| `wavebanner.lua`  | a word whose letters bob on a traveling wave |

## Activate

The executing session already staged copies into `~/.config/maki/lua/`, so you
can play right away. Open `~/.config/maki/init.lua` and add a `require` for the
skin you want:

```lua
require("pentagram")   -- pick one; comment the others out
```

Each file self-activates when `require`d via `maki.api.set_slot("splash.render",
...)`. To switch screens, edit which `require` line runs. `matrix_splash.lua`
(when you un-comment its `require`) is another option, just as easy.

If the staged copies are missing, copy from this repo instead:

```sh
cp examples/splash/*.lua ~/.config/maki/lua/
```

## Notes

- A full replace only works while the bundled `splash` plugin stays enabled, so
  `splash.render` keeps its default slot to wrap.
- Each renderer must stay pure and pull-driven: it runs on the Lua thread while
  the UI waits for the frame, so no blocking maki calls. These scripts only
  read theme colors and `maki.version()`.
- Animation only runs under `ui.splash_animation = true` (the default). With it
  off, any `splash.render` override still shows a static frame after the entry
  fade, but stops being re-pulled.