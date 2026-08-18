-- Shooting-comets splash. K comets streak along fixed diagonals (deterministic
-- per-index direction, speed, and offset, no per-frame randomness). Each head
-- is `*` with a `+`, `:`, `·` trail that fades behind it as it loops.

local TAU = 2.0 * math.pi
local TRAIL = 6
local ANGLES = { -0.4, -0.1, 0.2, 0.5 }
local SPEEDS = { 6, 7, 8, 9 }
local OFFS = { 20, 26, 32, 38 }
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

-- Head advance (`u`) for comet `i` at time `t`, looping across the screen.
M.progress = function(i, t, w)
  local vx = math.cos(ANGLES[i])
  local span = (w + 2 * TRAIL) / vx + 2
  return (t * SPEEDS[i] + OFFS[i]) % span
end

function M.render(w, h, t, fade)
  refresh_colors()
  W, H = w, h
  local f = fade or 1.0
  if w < 10 or h < 8 then
    return flat_rows(w, h, color(BG_HEX))
  end
  local grid = new_grid()
  local head = color(rgb_to_hex({ 255, 255, 255 }, f))
  local t1 = color(rgb_to_hex(ACCENT, f))
  local t2 = color(rgb_to_hex(ACCENT, 0.6 * f))
  local t3 = color(rgb_to_hex(ACCENT, 0.35 * f))
  for i = 1, #ANGLES do
    local vx = math.cos(ANGLES[i])
    local vy = math.sin(ANGLES[i])
    local starty = math.floor(h / 2) + (i - 2) * 3
    local adv = M.progress(i, t, w)
    for k = TRAIL, 0, -1 do
      local bx = math.floor(-TRAIL + (adv - k) * vx + 0.5)
      local by = math.floor(starty + (adv - k) * vy + 0.5)
      if bx >= 1 and bx <= w and by >= 1 and by <= h then
        local st, ch
        if k == 0 then
          st, ch = head, "*"
        elseif k == 1 then
          st, ch = t1, "+"
        elseif k == 2 then
          st, ch = t2, ":"
        else
          st, ch = t3, "·"
        end
        grid[by][bx] = { glyph = ch, style = st }
      end
    end
  end
  place_text(grid, H - 1, math.floor((W - 6) / 2) + 1, "comets", color(rgb_to_hex(FG, 0.5 * f)))
  place_text(grid, 1, W - #("v" .. maki.version().current) + 1, "v" .. maki.version().current, color(rgb_to_hex(FG, 0.4 * f)))
  return build_rows(grid)
end

maki.api.set_slot("splash.render", function(prev, w, h, t, fade)
  return M.render(w, h, t, fade)
end)

return M