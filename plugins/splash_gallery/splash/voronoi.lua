-- Bundled splash-gallery skin, part of the maki distribution.
-- Activate from init.lua with:   require("splash.voronoi")
-- Requiring self-activates it via maki.api.set_slot("splash.render", ...);
-- the module also returns M with M.render(w, h, t, fade) for custom cyclers.
--
-- Voronoi cells splash. Software port of the WGSL "Voronoi Cells" shader

local TAU = 2.0 * math.pi
local SCALE = 6.0
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

local function h22(px, py)
  local s1 = math.sin(px * 127.1 + py * 311.7) * 43758.5453
  local s2 = math.sin(px * 269.5 + py * 183.3) * 43758.5453
  return s1 - math.floor(s1), s2 - math.floor(s2)
end

-- Fragment shade for pattern coords (ux, uy); returns r, g, b in [0, 1].
function M.shade(ux, uy, t)
  local ipx = math.floor(ux)
  local ipy = math.floor(uy)
  local fpx = ux - ipx
  local fpy = uy - ipy
  local f1 = 8.0
  local f2 = 8.0
  local idx, idy = 0.0, 0.0
  for gy = -1, 1 do
    for gx = -1, 1 do
      local ox, oy = h22(ipx + gx, ipy + gy)
      local ptx = 0.5 + 0.5 * math.sin(t + TAU * ox)
      local pty = 0.5 + 0.5 * math.sin(t + TAU * oy)
      local dx = gx + ptx - fpx
      local dy = gy + pty - fpy
      local d = math.sqrt(dx * dx + dy * dy)
      if d < f1 then
        f2 = f1
        f1 = d
        idx = ipx + gx
        idy = ipy + gy
      elseif d < f2 then
        f2 = d
      end
    end
  end
  local edge = f2 - f1
  local rnd = h22(idx, idy)
  local ph = rnd * TAU + t * 0.5
  local cr = 0.5 + 0.5 * math.cos(ph + 0.0)
  local cg = 0.5 + 0.5 * math.cos(ph + 2.0)
  local cb = 0.5 + 0.5 * math.cos(ph + 4.0)
  local lines = smoothstep(0.0, 0.08, edge)
  local bright = 0.4 + 0.6 * f1
  local rr = (1.0 + (cr * cr - 1.0) * lines) * bright
  local gg = (0.95 + (cg * cg - 0.95) * lines) * bright
  local bb = (0.7 + (cb * cb - 0.7) * lines) * bright
  return rr, gg, bb
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
    local uy = ((y - 0.5) / h) * SCALE + t * 0.1
    for x = 1, w do
      local ux = ((x - 0.5) / (2 * h)) * SCALE + t * 0.2
      local r, g, b = M.shade(ux, uy, t)
      row[x] = {
        glyph = ramp_glyph(0.2126 * r * f + 0.7152 * g * f + 0.0722 * b * f),
        style = shade_style(r, g, b, f),
      }
    end
  end
  place_text(grid, H - 1, math.floor((W - 7) / 2) + 1, "voronoi", color(rgb_to_hex(FG, 0.5 * f)))
  place_text(grid, 1, W - #("v" .. maki.version().current) + 1, "v" .. maki.version().current, color(rgb_to_hex(FG, 0.4 * f)))
  return build_rows(grid)
end

maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  return M.render(w, h, t, fade)
end)

return M
