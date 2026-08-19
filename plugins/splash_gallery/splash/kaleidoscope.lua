-- Bundled splash-gallery skin, part of the maki distribution.
-- Require from init.lua with:   local skin = require("splash.kaleidoscope")
-- The module returns M with M.render(w, h, t, fade) and does not activate itself.
--
-- Kaleidoscope splash. Software port of the WGSL "Kaleidoscope" shader from

local TAU = 2.0 * math.pi
local SEGMENTS = 10.0
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

local function atan2(y, x)
  if x > 0 then
    return math.atan(y / x)
  elseif x < 0 and y >= 0 then
    return math.atan(y / x) + math.pi
  elseif x < 0 then
    return math.atan(y / x) - math.pi
  elseif y > 0 then
    return math.pi / 2
  elseif y < 0 then
    return -math.pi / 2
  end
  return 0
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

-- 5-bit-per-channel quantized style, keeps the style cache bounded.
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

-- Fragment shade: returns r, g, b in [0, 1] for isotropic coords (nx, ny).
function M.shade(nx, ny, t)
  local tt = t * 0.25
  local a = atan2(ny, nx)
  local r = math.sqrt(nx * nx + ny * ny)
  local seg = TAU / SEGMENTS
  local sa = math.abs((a % (2.0 * seg)) - seg) + tt * 0.4
  local ox = math.cos(sa) * r - (0.3 + 0.2 * math.sin(t * 0.3))
  local oy = math.sin(sa) * r
  local qx, qy = ox * 3.0, oy * 3.0
  local ar, ag, ab = 0.0, 0.0, 0.0
  for i = 0, 4 do
    local d = qx * qx + qy * qy
    if d < 0.15 then
      d = 0.15
    end
    qx = math.abs(qx) / d
    qy = math.abs(qy) / d
    qx = qx * 1.9 - (0.9 + 0.3 * math.sin(tt + i))
    qy = qy * 1.9 - 0.7
    local s = qx * 0.4 + qy * 0.8 + t + i
    ar = (ar + 0.5 + 0.5 * math.cos(s + 0.0)) * 0.85
    ag = (ag + 0.5 + 0.5 * math.cos(s + 2.1)) * 0.85
    ab = (ab + 0.5 + 0.5 * math.cos(s + 4.3)) * 0.85
  end
  ar = ar / 5.0
  ag = ag / 5.0
  ab = ab / 5.0
  local edge = smoothstep(1.6, 0.2, r * 0.5)
  return ar ^ 1.6 * edge, ag ^ 1.6 * edge, ab ^ 1.6 * edge
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
