-- Perspective tunnel splash. Concentric axis-aligned rectangles recede to the
-- screen center; nearer rings are denser/brighter, and the ring pattern
-- travels inward-over-time, so the tunnel reads as drifting toward the viewer.

local TAU = 2.0 * math.pi
local SPEED = 2.0
local SPACING = 4.0
local SYMS = { " ", ".", ":", "+", "*" }
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

local function color(hexstr)
  local s = style_cache[hexstr]
  if not s then
    s = { fg = hexstr, bg = BG_HEX, bold = false }
    style_cache[hexstr] = s
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

M.chebyshev = function(x, y, cx, cy)
  return math.max(math.abs(x - cx), math.abs(y - cy))
end

function M.render(w, h, t, fade)
  refresh_colors()
  W, H = w, h
  local f = fade or 1.0
  if w < 8 or h < 6 then
    return flat_rows(w, h, color(BG_HEX))
  end
  local grid = new_grid()
  local cx = math.floor(w / 2) + 1
  local cy = math.floor(h / 2) + 1
  local m_max = math.max(math.floor(w / 2), math.floor(h / 2))
  local bright = color(rgb_to_hex(ACCENT, f))
  local dim = color(rgb_to_hex(ACCENT, 0.45 * f))
  local phase = t * SPEED
  for y = 1, h do
    for x = 1, w do
      local m = M.chebyshev(x, y, cx, cy)
      local depth = 1.0 - m / m_max
      local ring = (m + phase) % SPACING / SPACING
      local val = depth * 0.9 + (1.0 - ring) * 0.3 * depth
      local level = math.floor(val * 5.0)
      if level < 0 then
        level = 0
      elseif level > 4 then
        level = 4
      end
      local st = level >= 3 and bright or dim
      grid[y][x] = { glyph = SYMS[level + 1], style = st }
    end
  end
  place_text(grid, H - 1, math.floor((W - 6) / 2) + 1, "tunnel", color(rgb_to_hex(FG, 0.5 * f)))
  place_text(grid, 1, W - #("v" .. maki.version().current) + 1, "v" .. maki.version().current, color(rgb_to_hex(FG, 0.4 * f)))
  return build_rows(grid)
end

maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  return M.render(w, h, t, fade)
end)

return M