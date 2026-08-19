-- Bundled splash-gallery skin, part of the maki distribution.
-- Require from init.lua with:   local skin = require("splash.aurora")
-- The module returns M with M.render(w, h, t, fade) and does not activate itself.
--
-- Aurora splash. Software port of the WGSL "Aurora" shader from

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

local function h21(px, py)
  local s = math.sin(px * 127.1 + py * 311.7) * 43758.5453
  return s - math.floor(s)
end

-- Smooth value noise.
function M.n2(px, py)
  local ix = math.floor(px)
  local iy = math.floor(py)
  local fx = px - ix
  local fy = py - iy
  local ux = fx * fx * (3.0 - 2.0 * fx)
  local uy = fy * fy * (3.0 - 2.0 * fy)
  local a = h21(ix, iy)
  local b = h21(ix + 1, iy)
  local c = h21(ix, iy + 1)
  local d = h21(ix + 1, iy + 1)
  return a + (b - a) * ux + (c - a) * uy + (a - b - c + d) * ux * uy
end

-- Per-column band parameters (center, spread, color, shimmer).
function M.column_bands(ux, t)
  local tt = t * 0.5
  local yc = {}
  local spread = {}
  local cr = {}
  local cg = {}
  local cb = {}
  local shimmer = {}
  for i = 0, 4 do
    local fi = tonumber(i)
    local sx = ux * 3.0 + fi * 1.7
    local wave = M.n2(sx * 1.2 + tt * 0.7, fi * 3.1) * 0.5 + M.n2(sx * 0.4 - tt * 0.3, fi * 7.7) * 0.5
    yc[i + 1] = 0.2 + wave * 0.45 + fi * 0.07
    spread[i + 1] = 10.0 + fi * 6.0
    local hue = 0.45 + 0.25 * math.sin(fi * 1.3 + tt * 0.2)
    cr[i + 1] = hue ^ 2 * 0.9
    cg[i + 1] = 0.9
    cb[i + 1] = (1.0 - hue) ^ 1.5
    shimmer[i + 1] = 0.25 + 0.25 * M.n2(sx * 5.0, tt)
  end
  return { yc = yc, spread = spread, cr = cr, cg = cg, cb = cb, shimmer = shimmer }
end

-- Fragment shade for normalized screen coords (ux, uy in [0, 1], y down).
function M.shade(ux, uy, t, cols)
  if not cols then
    cols = M.column_bands(ux, t)
  end
  local r, g, b = 0.0, 0.0, 0.0
  for i = 1, 5 do
    local band = math.exp(-math.abs(uy - cols.yc[i]) * cols.spread[i]) * cols.shimmer[i]
    r = r + cols.cr[i] * band
    g = g + cols.cg[i] * band
    b = b + cols.cb[i] * band
  end
  local sky = 1.0 - uy
  return r + 0.03 * sky, g + 0.03 * sky, b + 0.07 * sky
end

function M.render(w, h, t, fade)
  refresh_colors()
  W, H = w, h
  local f = fade or 1.0
  if w < 8 or h < 6 then
    return flat_rows(w, h, color(BG_HEX))
  end
  local grid = new_grid()
  for x = 1, w do
    local ux = (x - 0.5) / w
    local cols = M.column_bands(ux, t)
    for y = 1, h do
      local r, g, b = M.shade(ux, (y - 0.5) / h, t, cols)
      grid[y][x] = {
        glyph = ramp_glyph(0.2126 * r * f + 0.7152 * g * f + 0.0722 * b * f),
        style = shade_style(r, g, b, f),
      }
    end
  end
  place_text(
    grid,
    1,
    W - #("v" .. maki.version().current) + 1,
    "v" .. maki.version().current,
    color(rgb_to_hex(FG, 0.4 * f))
  )
  return build_rows(grid)
end

return M
