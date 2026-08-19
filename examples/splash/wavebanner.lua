-- Wave-banner splash. Kinetic typography: each letter sits in its own column
-- and bobs with a sine that is phase-shifted across x, so a traveling wave
-- ripples through the word. Opaque segments keep the letters readable.

local MESSAGE = "WAVE"
local AMP = 4
local FREQ = 0.7
local SPEED = 2.0
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

-- Vertical offset (signed, around `base`) for the letter in column `x` at `t`.
M.wave = function(x, t)
  return math.floor(AMP * math.sin(x * FREQ + t * SPEED) + 0.5)
end

function M.render(w, h, t, fade)
  refresh_colors()
  W, H = w, h
  local f = fade or 1.0
  if w < #MESSAGE + 4 or h < 2 * AMP + 4 then
    return flat_rows(w, h, color(BG_HEX))
  end
  local grid = new_grid()
  local acc = color(rgb_to_hex(ACCENT, f))
  local dim = color(rgb_to_hex(FG, 0.4 * f))
  local startx = math.floor((w - #MESSAGE) / 2) + 1
  local base = math.floor(h / 2) + 1
  for li = 0, #MESSAGE - 1 do
    local x = startx + li
    local y = base + M.wave(x, t)
    if y >= 1 and y <= h and x >= 1 and x <= w then
      grid[y][x] = { glyph = string.sub(MESSAGE, li + 1, li + 1), style = acc }
    end
  end
  local tagline = "make it simple"
  place_text(grid, base + AMP + 2, math.floor((w - #tagline) / 2) + 1, tagline, dim)
  place_text(grid, H - 1, math.floor((W - 10) / 2) + 1, "wavebanner", color(rgb_to_hex(FG, 0.5 * f)))
  place_text(grid, 1, W - #("v" .. maki.version().current) + 1, "v" .. maki.version().current, dim)
  return build_rows(grid)
end

maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  return M.render(w, h, t, fade)
end)

return M