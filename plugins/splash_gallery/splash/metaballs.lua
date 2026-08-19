-- Bundled splash-gallery skin, part of the maki distribution.
-- Activate from init.lua with:   require("splash.metaballs")
-- Requiring self-activates it via maki.api.set_slot("splash.render", ...);
-- the module also returns M with M.render(w, h, t, fade) for custom cyclers.
--
-- Metaballs splash. Software port of the WGSL "Metaballs" shader from

local RAMP = " .:-=+*#%@"
local M = {}

local function theme_or(name, fallback)
  local c = maki.ui.theme_color(name)
  if c then
    return {
      tonumber(string.sub(c, 2, 3), 16),
      tonumber(string.sub(c, 4, 5), 16),
      tonumber(string.sub(c, 6, 7), 16),
    }
  end
  return fallback
end

local BG, FG, ACCENT, BG_HEX
local style_cache = {}

local function refresh_colors()
  BG = theme_or("background", { 40, 42, 54 })
  FG = theme_or("foreground", { 248, 248, 242 })
  ACCENT = theme_or("accent", { 255, 184, 108 })
  BG_HEX = string.format("#%02x%02x%02x", BG[1], BG[2], BG[3])
  style_cache = {}
end

local function rgb_to_hex(c, a)
  return string.format(
    "#%02x%02x%02x",
    math.floor(c[1] * a + 0.5),
    math.floor(c[2] * a + 0.5),
    math.floor(c[3] * a + 0.5)
  )
end

local function color(hex)
  local s = style_cache[hex]
  if not s then
    s = { fg = hex, bg = BG_HEX, bold = false }
    style_cache[hex] = s
  end
  return s
end

local W, H

local function new_grid()
  local bg = color(BG_HEX)
  local grid = {}
  for y = 1, H do
    local row = {}
    for x = 1, W do
      row[x] = { glyph = " ", style = bg }
    end
    grid[y] = row
  end
  return grid
end

local function place_text(grid, row, x, text, st)
  if row < 1 or row > H then
    return
  end
  local r = grid[row]
  for i = 1, #text do
    local xx = x + i - 1
    if xx >= 1 and xx <= W then
      r[xx] = { glyph = string.sub(text, i, i), style = st }
    end
  end
end

local function build_rows(grid)
  local rows = {}
  for y = 1, H do
    local segs = {}
    local buf = {}
    local cur
    local function flush()
      if #buf > 0 then
        segs[#segs + 1] = { glyphs = table.concat(buf), style = cur }
        buf = {}
      end
    end
    for x = 1, W do
      local cell = grid[y][x]
      if cell.style ~= cur then
        flush()
        cur = cell.style
      end
      buf[#buf + 1] = cell.glyph
    end
    flush()
    rows[y] = segs
  end
  return rows
end

local function flat_rows(w, h, st)
  local rows = {}
  for y = 1, h do
    rows[y] = { { glyphs = string.rep(" ", w), style = st } }
  end
  return rows
end

local function smoothstep(e0, e1, x)
  local u = (x - e0) / (e1 - e0)
  if u < 0 then
    u = 0
  elseif u > 1 then
    u = 1
  end
  return u * u * (3.0 - 2.0 * u)
end

local function shade_style(r, g, b, f)
  local function q(v)
    if v < 0 then
      v = 0
    elseif v > 1 then
      v = 1
    end
    return math.floor(v * 31 + 0.5) * 255 / 31
  end
  return color(
    string.format("#%02x%02x%02x", math.floor(q(r * f) + 0.5), math.floor(q(g * f) + 0.5), math.floor(q(b * f) + 0.5))
  )
end

local function ramp_glyph(lum)
  if lum < 0 then
    lum = 0
  elseif lum > 1 then
    lum = 1
  end
  local gi = math.floor(lum * (#RAMP - 1) + 0.5) + 1
  return string.sub(RAMP, gi, gi)
end

local function fract(x)
  return x - math.floor(x)
end

local function ball(ux, uy, bx, by, k)
  local dx = ux - bx
  local dy = uy - by
  return k / math.sqrt(dx * dx + dy * dy + 1e-4)
end

-- Fragment shade for isotropic coords (nx, ny); returns r, g, b in [0, 1].
function M.shade(nx, ny, t)
  local v = 0.0
  v = v + ball(nx, ny, math.sin(t * 0.7) * 0.7, math.cos(t * 0.9) * 0.7, 0.35)
  v = v + ball(nx, ny, math.cos(t * 1.1) * 0.8, math.sin(t * 0.6) * 0.8, 0.30)
  v = v + ball(nx, ny, math.sin(t * 0.5 + 2.0) * 0.5, math.cos(t * 0.8 + 1.0) * 0.5, 0.25)
  v = v + ball(nx, ny, math.sin(t * 0.33 + 4.0) * 0.9, math.cos(t * 0.41 + 2.0) * 0.6, 0.22)
  local edge = smoothstep(1.15, 1.25, v)
  local core = smoothstep(1.25, 2.4, v)
  local r = 0.03 + (0.1 - 0.03) * edge
  local g = 0.02 + (0.4 - 0.02) * edge
  local b = 0.06 + (0.9 - 0.06) * edge
  r = r + (0.6 - r) * core
  g = g + (0.9 - g) * core
  b = b + (1.0 - b) * core
  local glow = math.exp(-math.abs(v - 1.2) * 3.0) * 0.8
  r = r + 0.3 * glow
  g = g + 0.7 * glow
  b = b + 1.0 * glow
  local gx = math.abs(fract(nx * 8.0) - 0.5)
  local gy = math.abs(fract(ny * 8.0) - 0.5)
  local gridline = 0.02 * smoothstep(0.48, 0.5, gx > gy and gx or gy)
  return r + gridline, g + gridline, b + gridline
end

function M.render(w, h, t, fade)
  refresh_colors()
  W, H = w, h
  local f = fade or 1.0
  if w < 8 or h < 6 then
    return flat_rows(w, h, color(BG_HEX))
  end
  local grid = new_grid()
  for y = 1, h do
    local row = grid[y]
    local ny = (2 * (y - 0.5) - h) / h
    for x = 1, w do
      local nx = ((x - 0.5) - w / 2) / h
      local r, g, b = M.shade(nx, ny, t)
      row[x] = {
        glyph = ramp_glyph(0.2126 * r * f + 0.7152 * g * f + 0.0722 * b * f),
        style = shade_style(r, g, b, f),
      }
    end
  end
  place_text(grid, H - 1, math.floor((W - 9) / 2) + 1, "metaballs", color(rgb_to_hex(FG, 0.5 * f)))
  place_text(
    grid,
    1,
    W - #("v" .. maki.version().current) + 1,
    "v" .. maki.version().current,
    color(rgb_to_hex(FG, 0.4 * f))
  )
  return build_rows(grid)
end

maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  return M.render(w, h, t, fade)
end)

return M
