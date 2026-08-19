# Scene composition recipes

Small, battle-tested building blocks for splashes that depict a *scene*
(regions, horizons, perspective) rather than a field or an orbit. All coords
are the template's isotropic `(nx, ny)`, y down, ny in [-1, 1].

## Horizon split (sky / floor, sea / sky, day / night)

Split `M.shade` on `ny` against a horizon line. Put the horizon ABOVE center
(`ny = -0.2` or so) — more sky than floor almost always reads better, and the
floor's perspective lines need only a few rows to sell depth.

```lua
function M.shade(nx, ny, t)
  if ny < HOR then
    -- sky region
  else
    -- floor region
  end
end
```

Keep the horizon itself one clean line (a hard brightness or hue step at
`ny == HOR`). Haze or glow that bleeds across the line is the most common way
these scenes turn to mush — if your dump looks unreadable, sharpen the step
first.

## Perspective grid floor

For points below the horizon, fake depth by dividing by the distance to the
horizon:

```lua
local dy = ny - HOR                 -- > 0 below the horizon
local z = 0.4 / math.max(dy, 0.001) -- depth: large near viewer, small far
local wx = nx * z                   -- world x: fans vertical lines toward center
local gx = math.abs((wx * 1.5) % 1 - 0.5)
local gz = math.abs((z * 1.2 - t * 2.0) % 1 - 0.5)  -- scrolls toward viewer
local line = smoothstep(0.06, 0.0, math.min(gx, gz) * math.min(z * 0.06, 1.0))
```

The `z * 0.06` factor thins lines with distance; without it the far rows turn
into static. Fade the whole floor contribution by something like
`math.min(z, 1.5) * 0.7` so the far field falls off naturally.

## Striped sun

A disk with horizontal stripe gaps sells "retro sun" instantly:

```lua
local dx = nx - SUN_X
local dyy = (ny - SUN_Y) * 1.6      -- squash y: dome, not circle
local d = math.sqrt(dx * dx + dyy * dyy)
local sun = smoothstep(0.45, 0.40, d)
-- gap stripes, denser near the bottom of the disk
local frac = (ny * 14.0 - t * 0.5) % 1
local gap = smoothstep(0.03, 0.0, math.abs(frac - 0.5) - 0.42)
if ny > SUN_Y then                  -- stripes only in the lower half
  sun = sun * gap
end
```

## Region palettes

For regioned scenes, compute each region's rgb in its own branch and do the
final `r, g, b` assignment once at the end — it keeps the "every cell ends up
with a color and a glyph" invariant obvious and makes regions easy to rebalance.

## One light source per scene

Sun, horizon glow, floor grid: they all read as lit by the sun if their
brightest color is the same hue. Pick one accent hue for the scene's light and
derive everything else as dimmer or complementary versions of it.
