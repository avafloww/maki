-- Spinning five-pointed star splash. Drop-in `splash.render` override.
-- One full turn every 5 s (1/5 turn per second). Pure and pull-driven: no
-- blocking maki calls, only the theme colors and maki.version() it is handed.

local TAU = 2.0 * math.pi
local TAU5 = TAU / 5.0
local PERIOD_S = 5.0
local CHORDS = { 3, 4, 5, 1, 2 }
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

-- Angle of vertex zero. Folded over the 5 s period so a full turn is exact and
-- the rasterization is bit-identical each loop.
M.vertex_angle = function(t)
  return TAU5 * (t % PERIOD_S)
end

local function draw_line(grid, x0, y0, x1, y1, st)
  local dx = math.abs(x1 - x0)
  local dy = -math.abs(y1 - y0)
  local sx = x0 < x1 and 1 or -1
  local sy = y0 < y1 and 1 or -1
  local err = dx + dy
  while true do
    if y0 >= 1 and y0 <= H and x0 >= 1 and x0 <= W then
      grid[y0][x0] = { glyph = "*", style = st }
    end
    if x0 == x1 and y0 == y1 then
      return
    end
    local e2 = 2 * err
    if e2 >= dy then
      err = err + dy
      x0 = x0 + sx
    end
    if e2 <= dx then
      err = err + dx
      y0 = y0 + sy
    end
  end
end

function M.render(w, h, t, fade)
  refresh_colors()
  W, H = w, h
  local f = fade or 1.0
  if w < 8 or h < 7 or math.floor(math.min(w, h) / 2) - 3 < 2 then
    return flat_rows(w, h, color(BG_HEX))
  end
  local grid = new_grid()
  local cx = math.floor(w / 2) + 1
  local cy = math.floor(h / 2) + 1
  local R = math.floor(math.min(w, h) / 2) - 3
  local base = M.vertex_angle(t) - math.pi / 2
  local vertices = {}
  for i = 0, 4 do
    local ang = base + i * TAU5
    vertices[i + 1] = {
      math.floor(cx + R * math.cos(ang) + 0.5),
      math.floor(cy + R * math.sin(ang) + 0.5),
    }
  end
  local acc = color(rgb_to_hex(ACCENT, f))
  local bright = color(rgb_to_hex({ 255, 255, 255 }, f))
  for i = 1, 5 do
    local s = vertices[i]
    local e = vertices[CHORDS[i]]
    draw_line(grid, s[1], s[2], e[1], e[2], acc)
  end
  local head = vertices[1]
  if head[1] >= 1 and head[1] <= W and head[2] >= 1 and head[2] <= H then
    grid[head[2]][head[1]] = { glyph = "*", style = bright }
  end
  place_text(grid, H - 1, math.floor((W - 9) / 2) + 1, "pentagram", color(rgb_to_hex(FG, 0.5 * f)))
  place_text(grid, 1, W - #("v" .. maki.version().current) + 1, "v" .. maki.version().current, color(rgb_to_hex(FG, 0.4 * f)))
  return build_rows(grid)
end

maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  return M.render(w, h, t, fade)
end)

return M