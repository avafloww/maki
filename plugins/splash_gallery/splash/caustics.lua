-- Bundled splash-gallery skin, part of the maki distribution.
-- Require from init.lua with:   local skin = require("splash.caustics")
-- The module returns M with M.render(w, h, t, fade) and does not activate itself.
--
-- Caustics splash. Software port of the WGSL "Caustics" shader from

local RAMP = " .:-=+*#%@"
local SAMPLE_STEP = 4
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

local function flat_rows(w, h, st)
  local rows = {}
  for y = 1, h do
    rows[y] = { { glyphs = string.rep(" ", w), style = st } }
  end
  return rows
end

local function quantize(v)
  if v < 0 then
    v = 0
  elseif v > 1 then
    v = 1
  end
  return math.floor(math.floor(v * 31 + 0.5) * 255 / 31 + 0.5)
end

local function shade_style(r, g, b, f)
  local qr = quantize(r * f)
  local qg = quantize(g * f)
  local qb = quantize(b * f)
  local key = qr * 65536 + qg * 256 + qb
  local st = style_cache[key]
  if not st then
    st = { fg = string.format("#%02x%02x%02x", qr, qg, qb), bg = BG_HEX, bold = false }
    style_cache[key] = st
  end
  return st
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
    local fi = i
    local wx = M.n2(qx * fi + tt * 0.3, qy * fi + tt * 0.3)
    local wy = M.n2(qx * fi - tt * 0.2, qy * fi - tt * 0.2)
    qx = qx + wx * 0.7
    qy = qy + wy * 0.7
    local wv = 1.0 - math.abs(math.sin((qx + qy) * (2.0 + fi) - tt))
    wv = wv * wv
    wv = wv * wv
    c = c + wv * wv / fi
  end
  local deep_r, deep_g, deep_b = 0.0, 0.08, 0.18
  local lite_r, lite_g, lite_b = 0.4, 0.95, 1.1
  local vig = 1.0 - 0.35 * math.sqrt(ux * ux + uy * uy) * 0.5
  return (deep_r + lite_r * c * 0.9) * vig, (deep_g + lite_g * c * 0.9) * vig, (deep_b + lite_b * c * 0.9) * vig
end

function M.render(w, h, t, fade)
  refresh_colors()
  local f = fade or 1.0
  if w < 8 or h < 6 then
    return flat_rows(w, h, color(BG_HEX))
  end
  local version = "v" .. maki.version().current
  local version_x = w - #version + 1
  local version_style = color(rgb_to_hex(FG, 0.4 * f))
  local rows = {}
  local x_scale = 1 / h
  local sample_r, sample_g, sample_b = {}, {}, {}
  local sample_columns = math.floor((w - 1) / SAMPLE_STEP) + 2
  local sample_rows = math.floor((h - 1) / SAMPLE_STEP) + 2
  for sample_y = 1, sample_rows do
    sample_r[sample_y], sample_g[sample_y], sample_b[sample_y] = {}, {}, {}
    local y = 1 + (sample_y - 1) * SAMPLE_STEP
    local ny = (2 * (y - 0.5) - h) / h
    for sample_x = 1, sample_columns do
      local x = 1 + (sample_x - 1) * SAMPLE_STEP
      local r, g, b = M.shade(((x - 0.5) - w / 2) * x_scale, ny, t)
      sample_r[sample_y][sample_x] = r
      sample_g[sample_y][sample_x] = g
      sample_b[sample_y][sample_x] = b
    end
  end
  for y = 1, h do
    local grid_y = (y - 1) / SAMPLE_STEP
    local sample_y = math.floor(grid_y) + 1
    local fy = grid_y - math.floor(grid_y)
    local glyphs = {}
    local segs = {}
    local current_style
    local run_start = 1
    for x = 1, w do
      local glyph, style
      if y == 1 and x >= version_x then
        glyph = string.sub(version, x - version_x + 1, x - version_x + 1)
        style = version_style
      else
        local grid_x = (x - 1) / SAMPLE_STEP
        local sample_x = math.floor(grid_x) + 1
        local fx = grid_x - math.floor(grid_x)
        local r0 = sample_r[sample_y][sample_x] * (1 - fx) + sample_r[sample_y][sample_x + 1] * fx
        local g0 = sample_g[sample_y][sample_x] * (1 - fx) + sample_g[sample_y][sample_x + 1] * fx
        local b0 = sample_b[sample_y][sample_x] * (1 - fx) + sample_b[sample_y][sample_x + 1] * fx
        local r1 = sample_r[sample_y + 1][sample_x] * (1 - fx) + sample_r[sample_y + 1][sample_x + 1] * fx
        local g1 = sample_g[sample_y + 1][sample_x] * (1 - fx) + sample_g[sample_y + 1][sample_x + 1] * fx
        local b1 = sample_b[sample_y + 1][sample_x] * (1 - fx) + sample_b[sample_y + 1][sample_x + 1] * fx
        local r = r0 * (1 - fy) + r1 * fy
        local g = g0 * (1 - fy) + g1 * fy
        local b = b0 * (1 - fy) + b1 * fy
        glyph = ramp_glyph((0.2126 * r + 0.7152 * g + 0.0722 * b) * f)
        style = shade_style(r, g, b, f)
      end
      glyphs[x] = glyph
      if style ~= current_style then
        if current_style then
          segs[#segs + 1] = { glyphs = table.concat(glyphs, "", run_start, x - 1), style = current_style }
        end
        current_style = style
        run_start = x
      end
    end
    segs[#segs + 1] = { glyphs = table.concat(glyphs, "", run_start, w), style = current_style }
    rows[y] = segs
  end
  return rows
end

return M
