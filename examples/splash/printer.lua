-- ASCII printer splash. A printer near the top third prints a sheet that
-- visibly grows downward; when the paper reaches the screen bottom the page
-- "drops" and a fresh short page starts. Deterministic in `t`, so a reframe at
-- the same `t` redraws the same page.

local SPEED = 2.0
local BODY_HEIGHT = 4
local PRINTED_LINE = "all good here"
local PAPER_BG = { 58, 60, 78 }
local BODY_BG = { 90, 90, 110 }
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

local function hex(c)
  return string.format("#%02x%02x%02x", c[1], c[2], c[3])
end

local function dim_rgb(c, a)
  return {
    math.floor(c[1] * a + 0.5),
    math.floor(c[2] * a + 0.5),
    math.floor(c[3] * a + 0.5),
  }
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

-- Visible sheet length for a given `t` (paper rows below the feed slot).
M.sheet_len = function(t, max_len)
  return math.floor((t * SPEED) % (max_len + 1))
end

function M.render(w, h, t, fade)
  refresh_colors()
  W, H = w, h
  local f = fade or 1.0
  if w < 12 or h < 10 then
    return flat_rows(w, h, color(BG_HEX))
  end
  local grid = new_grid()
  local body = color(hex(dim_rgb(BODY_BG, f)))
  local paper = color(hex(dim_rgb(PAPER_BG, f)))
  local ink = color(hex(dim_rgb(ACCENT, f)))
  local fgdim = color(hex(dim_rgb(FG, 0.5 * f)))
  local top = math.floor(h * 0.28) + 1
  local feed = top + BODY_HEIGHT
  local slot_y = feed + 1
  local max_len = h - slot_y
  for x = 1, w do
    if x >= 1 and x <= W then
      grid[top][x] = { glyph = "_", style = body }
      grid[feed][x] = { glyph = "-", style = body }
    end
  end
  for row = top + 1, feed - 1 do
    if row >= 1 and row <= H then
      grid[row][1] = { glyph = "|", style = body }
      grid[row][w] = { glyph = "|", style = body }
    end
  end
  if slot_y <= h then
    grid[slot_y][math.floor(w / 2)] = { glyph = "_", style = body }
  end
  local len = M.sheet_len(t, max_len)
  local last = math.min(h, slot_y + len - 1)
  for row = slot_y, last do
    for x = 1, w do
      grid[row][x] = { glyph = " ", style = paper }
    end
    if row == slot_y then
      place_text(grid, row, math.floor((w - #PRINTED_LINE) / 2) + 1, PRINTED_LINE, ink)
    end
  end
  place_text(grid, H - 1, math.floor((W - 7) / 2) + 1, "printer", fgdim)
  place_text(grid, 1, W - #("v" .. maki.version().current) + 1, "v" .. maki.version().current, fgdim)
  return build_rows(grid)
end

maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  return M.render(w, h, t, fade)
end)

return M