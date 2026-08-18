-- Flowers rising bottom-to-top splash. Drop-in `splash.render` override.
-- Deterministic: fixed flower columns that sway by +-1 cell, each rising on a
-- slow sawtooth as `t` grows and wrapping back to the bottom.

local TAU = 2.0 * math.pi
local SPEED = 1.5
local PERIOD = 60.0
local BLOOM = { "(", "{", "o", "}", ")" }
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

local BG, FG, ACCENT, GREEN, BG_HEX
local style_cache = {}

local function refresh_colors()
  BG = theme_or("background", { 40, 42, 54 })
  FG = theme_or("foreground", { 248, 248, 242 })
  ACCENT = theme_or("accent", { 255, 184, 108 })
  GREEN = theme_or("green", { 80, 250, 123 })
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

M.bloom_row = function(t, phase)
  return H - ((t * SPEED + phase) % PERIOD)
end

function M.render(w, h, t, fade)
  refresh_colors()
  W, H = w, h
  local f = fade or 1.0
  if w < 10 or h < 8 then
    return flat_rows(w, h, color(BG_HEX))
  end
  local grid = new_grid()
  local acc = color(rgb_to_hex(ACCENT, f))
  local grn = color(rgb_to_hex(GREEN, f))
  local n = math.max(1, math.min(4, math.floor(w / 16)))
  for i = 0, n - 1 do
    local x0 = math.floor((i + 1) * w / (n + 1)) + 1
    local sway = math.floor(math.sin(t * 1.4 + i * 1.7))
    local sx = x0 + sway
    local yy = math.floor(M.bloom_row(t, i * 11.0))
    for k = -2, 2 do
      local px = sx + k
      if yy >= 1 and yy <= h and px >= 1 and px <= w then
        grid[yy][px] = { glyph = BLOOM[k + 3], style = acc }
      end
    end
    for row = math.max(yy + 1, 1), h do
      if sx >= 1 and sx <= W then
        grid[row][sx] = { glyph = "|", style = grn }
      end
    end
    local leaf_y = math.min(h, yy + math.floor(h * 0.35) + 1)
    local lx = sx - 1
    if leaf_y >= 1 and lx >= 1 and lx <= w then
      grid[leaf_y][lx] = { glyph = "/", style = grn }
    end
  end
  place_text(grid, H - 1, math.floor((W - 7) / 2) + 1, "flowers", color(rgb_to_hex(FG, 0.5 * f)))
  place_text(grid, 1, W - #("v" .. maki.version().current) + 1, "v" .. maki.version().current, color(rgb_to_hex(FG, 0.4 * f)))
  return build_rows(grid)
end

maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  return M.render(w, h, t, fade)
end)

return M