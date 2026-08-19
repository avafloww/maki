+++
title = "Home-screen splash"
weight = 40
[extra]
group = "Guides"
+++

# Home-screen splash

When maki starts with no conversation, it shows the home screen: an animated starfield, the logo, a tagline, a tip, the help line, and the version in the top-right corner. All of it is drawn by a Lua plugin (`plugins/splash/init.lua`, bundled and enabled by default), so you can replace or tweak any of it from `init.lua` with no Rust rebuild and no new config.

Rust keeps the operationally sensitive pieces a plugin should not own: the frame clock, the repaint cadence, the entry-fade value, and the version/update check. The plugin answers a per-frame question with the pixels for the whole screen.

## The `splash.render` slot

Every time the splash is allowed to animate, the UI pulls a frame from the slot `splash.render` and blits it. The bundled `splash` plugin owns the slot and renders the default screen. Your plugin can wrap it with `maki.api.set_slot("splash.render", ...)`.

```lua
maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  -- call prev(w, h, t, fade) to keep the default and tweak it,
  -- or ignore it to fully replace the screen.
  return my_frame(w, h, t, fade)
end)
```

The function receives:

- `w`, `h` the splash area size in cells
- `t` seconds since this splash session started
- `fade` 0 to 1, the entry-fade value Rust computes itself

It returns a table of `h` rows, each an array of segments. A segment is `{ glyphs = string, style = ... }`. The `glyphs` strings of a row concatenate to exactly `w` cells. Space characters are skipped by the blitter unless a segment carries an explicit style (explicit styles paint every char, spaces included, so they erase the starfield behind text).

The `style` is one of:

- `"field"` reuse the fixed `" .:+*"` wave-intensity LUT. Use one `"field"` segment per background row; the blitter colors each glyph from the accent palette.
- `"#rrggbb"` a foreground hex color.
- `{ fg = "#rrggbb", bg = "#rrggbb", bold = false }` an explicit cell style. Use this for text rows so the opaque background keeps the starfield behind it as `[field lead, text, field tail]`.

A still splash is never animated: the cadence is `IDLE` after the entry fade, so `splash.render` stops being called. `ui.splash_animation = false` means a fading-in still splash that settles, not a frozen starfield. With it on, the starfield drifts at full frame rate.

## Who fades what

Rust computes `fade` and passes it down; the plugin owns applying it. The default folds `fade` into the starfield's intensity-to-glyph map (a dimmer cell shows a lower bucket, not a dimmed color) and bakes the per-element alphas into explicit text colors. Rust never fades a row. An override can apply `fade` the same way, or simply ignore it.

## Version and the update notice

Rust keeps running the update check. The result is mirrored into the Lua runtime, and plugins read it with `maki.version()`:

```lua
local v = maki.version()
-- v.current        string, e.g. "0.4.8"
-- v.latest         string | nil
-- v.update_available  boolean
```

The default plugin draws the top-right version text and, when an update exists, appends ` run maki update to get v<latest>`. It queries `maki.version()` inside `splash.render`, so the version UI is fully plugin-owned.

## Bundled gallery

Six ready-made skins ship inside the binary as requireable modules under the `splash.*` namespace. Nothing loads until you ask for it; each require one line in `init.lua`:

```lua
require("splash.kaleidoscope")
```

| Module | What it draws |
|--------|---------------|
| `splash.kaleidoscope` | 10-fold mirror kaleidoscope over a circle-inversion fractal |
| `splash.voronoi` | animated voronoi cells with warm glowing borders |
| `splash.caustics` | deep-water light caustics |
| `splash.metaballs` | merging metaballs with a glow contour |
| `splash.aurora` | northern-light bands drifting over a night gradient |
| `splash.matrix` | green falling-code rain, resets on `SplashShown` |

Each module self-activates on require and also returns `M` with `M.render(w, h, t, fade)`, so a small cycler can rotate through them:

```lua
local skins = {
  require("splash.kaleidoscope"),
  require("splash.aurora"),
}
maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  local skin = skins[(math.floor(t / 3) % #skins) + 1]
  return skin.render(w, h, t, fade)
end)
```

Bundled modules resolve before files in your config's `lua/` dir, so keep personal skins on plain names (`require("myskin")`) outside the `splash.*` namespace. The gallery sources live in `plugins/splash_gallery/` in the repo and follow the same single-file pattern as the custom skins below, so they double as worked examples.

## Lifecycle events

`SplashShown` fires when the home screen appears (startup, and after returning to it). `SplashHidden` fires when it goes away, once the first message or turn lands. A plugin uses these to reset per-show state, like picking a fresh tip or clearing matrix-rain columns:

```lua
maki.api.create_autocmd("SplashShown", {
  callback = function()
    -- reset anything that should start fresh each time the home screen shows
  end,
})
```

## Matrix rain: a whole-screen override

This is a complete `init.lua` override. It ignores `prev` because it replaces the whole screen, keeps its own per-width column state, resets on `SplashShown`, and draws the version itself in the top-right to show that plugins own that corner too.

```lua
local GLYPHS = "!<>-_\\/[]{}=+*^?#"
local BG = "#000000"

local state = { cols = {}, last_t = 0 }

local function reset(w)
  local c = {}
  for x = 1, w do
    c[x] = { y = math.random(-20, 0), speed = 0.15 + math.random() * 0.35 }
  end
  state.cols[w] = c
end

maki.api.create_autocmd("SplashShown", { callback = function() state.cols = {} end })

local function drop_glyph()
  return string.sub(GLYPHS, math.random(1, #GLYPHS), math.random(1, #GLYPHS))
end

local function matrix_frame(w, h, t, fade)
  if not state.cols[w] then reset(w) end
  local dt = math.max(0, t - state.last_t)
  state.last_t = t

  local grid = {}
  for _ = 1, h do grid[#grid + 1] = {} end
  for x = 1, w do
    local c = state.cols[w][x]
    c.y = c.y + c.speed * dt
    local head = math.floor(c.y)
    for k = 0, 5 do
      local yy = head - k
      if yy >= 1 and yy <= h then
        local fg = "#00ff41"
        if k == 0 then fg = "#c8ffd0" elseif k > 1 then fg = "#005c17" end
        grid[yy][x] = { ch = drop_glyph(), fg = fg }
      end
    end
  end

  -- a title line and the version, proving text + version are plugin-owned
  local title = "matrix rain"
  local v = maki.version()
  local vs = "v" .. v.current

  local rows = {}
  for y = 1, h do
    local segs, buf = {}, {}
    for x = 1, w do
      if grid[y][x] and (y ~= h - 4) and x ~= (w - #vs) then
        if #buf > 0 then
          segs[#segs + 1] = { glyphs = table.concat(buf), style = { fg = BG, bg = BG, bold = false } }
          buf = {}
        end
        segs[#segs + 1] = { glyphs = grid[y][x].ch, style = { fg = grid[y][x].fg, bg = BG, bold = false } }
      else
        buf[#buf + 1] = " "
      end
    end
    segs[#segs + 1] = { glyphs = table.concat(buf), style = { fg = BG, bg = BG, bold = false } }
    if y == h - 4 then
      segs[1] = { glyphs = string.rep(" ", math.floor((w - #title) / 2)), style = { fg = BG, bg = BG, bold = false } }
      segs[2] = { glyphs = title, style = { fg = "#00ff41", bg = BG, bold = true } }
    elseif y == 1 then
      segs[1] = { glyphs = string.rep(" ", w - #vs - 1), style = { fg = BG, bg = BG, bold = false } }
      segs[2] = { glyphs = vs, style = { fg = "#00ff41", bg = BG, bold = false } }
    end
    rows[y] = segs
  end
  return rows
end

maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  return matrix_frame(w, h, t, fade)
end)
```

## More splash screens

`examples/splash/` ships six self-contained whole-screen overrides you can
drop into `~/.config/maki/lua/` and `require` to explore different home
screens: a spinning pentagram, rising flowers, an ASCII printer, a perspective
tunnel, shooting comets, and a wave banner. They work exactly like the example
above. See [`examples/splash/README.md`](../../../../examples/splash/README.md)
for how to copy and switch between them.

Two notes on writing overrides:

- The renderer must be pure and pull-driven. It runs on the Lua thread while the UI waits for the frame, so do not call blocking maki API from inside it (for example an `open_win` that waits on the UI would deadlock). The default and the example above only read `maki.version()` and theme colors.
- A full replace only works while the bundled `splash` plugin stays enabled, so `splash.render` keeps its default. Wrapping with `prev(...)` lets a layer tweak the default instead of replacing it.