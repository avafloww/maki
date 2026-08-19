-- Bundled splash-gallery skin, part of the maki distribution.
-- Activate from init.lua with:   require("splash.caustics")
-- Requiring self-activates it via maki.api.set_slot("splash.render", ...);
-- the module also returns M with M.render(w, h, t, fade) for custom cyclers.
--
-- Caustics splash. Software port of the WGSL "Caustics" shader from

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
  return color(string.format("#%02x%02x%02x", math.floor(q(r * f) + 0.5), math.floor(q(g * f) + 0.5), math.floor(q(b * f) + 0.5)))
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

-- Fragment shade for isotropic coords (nx, ny); returns r, g, b in [0, 1].
function M.shade(nx, ny, t)
  local ux = nx * 2.0
  local uy = ny * 2.0
  local tt = t * 0.6
  local qx, qy = ux, uy
  local c = 0.0
  for i = 1, 4 do
    local fi = tonumber(i)
    local wx = M.n2(qx * fi + tt * 0.3, qy * fi + tt * 0.3)
    local wy = M.n2(qx * fi - tt * 0.2, qy * fi - tt * 0.2)
    qx = qx + wx * 0.7
    qy = qy + wy * 0.7
    local wv = math.abs(math.sin((qx + qy) * (2.0 + fi) - tt))
    c = c + (1.0 - wv) ^ 8 / fi
  end
  local deep_r, deep_g, deep_b = 0.0, 0.08, 0.18
  local lite_r, lite_g, lite_b = 0.4, 0.95, 1.1
  local vig = 1.0 - 0.35 * math.sqrt(ux * ux + uy * uy) * 0.5
  return (deep_r + lite_r * c * 0.9) * vig,
    (deep_g + lite_g * c * 0.9) * vig,
    (deep_b + lite_b * c * 0.9) * vig
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
  place_text(grid, H - 1, math.floor((W - 8) / 2) + 1, "caustics", color(rgb_to_hex(FG, 0.5 * f)))
  place_text(grid, 1, W - #("v" .. maki.version().current) + 1, "v" .. maki.version().current, color(rgb_to_hex(FG, 0.4 * f)))
  return build_rows(grid)
end

maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  return M.render(w, h, t, fade)
end)

return M
