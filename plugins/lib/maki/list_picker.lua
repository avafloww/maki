local TextInput = require("maki.text_input")

local ListPicker = {}
ListPicker.__index = ListPicker

local DETAIL_RIGHT_PAD = 2
local NO_MATCHES_LABEL = "  (no matches)"

local function split_words(query)
  local words = {}
  for w in (query or ""):lower():gmatch("%S+") do
    words[#words + 1] = w
  end
  return words
end

-- Returns matched byte positions and a lower-is-better score. Compact matches
-- sort before scattered ones; ties retain the source order.
local function fuzzy_match(text, word)
  local positions = {}
  local start = 1
  local hay = text:lower()
  for i = 1, #word do
    local position = hay:find(word:sub(i, i), start, true)
    if not position then
      return nil
    end
    positions[#positions + 1] = position
    start = position + 1
  end
  local gaps = positions[#positions] - positions[1] + 1 - #word
  return positions, gaps * 1000 + positions[1]
end

local function match_ranges(label, words)
  local ranges = {}
  for _, word in ipairs(words) do
    local positions = fuzzy_match(label, word)
    if positions then
      for _, position in ipairs(positions) do
        ranges[#ranges + 1] = { position, position }
      end
    end
  end
  table.sort(ranges, function(a, b)
    return a[1] < b[1]
  end)
  local merged = {}
  for _, range in ipairs(ranges) do
    local last = merged[#merged]
    if last and range[1] <= last[2] + 1 then
      last[2] = math.max(last[2], range[2])
    else
      merged[#merged + 1] = range
    end
  end
  return merged
end

local function highlight_spans(label, words, base, match_style)
  local ranges = match_ranges(label, words)
  if #ranges == 0 then
    return { { label, base } }
  end
  local spans, pos = {}, 1
  for _, r in ipairs(ranges) do
    if r[1] > pos then
      spans[#spans + 1] = { label:sub(pos, r[1] - 1), base }
    end
    spans[#spans + 1] = { label:sub(r[1], r[2]), match_style }
    pos = r[2] + 1
  end
  if pos <= #label then
    spans[#spans + 1] = { label:sub(pos), base }
  end
  return spans
end

local function item_label(item)
  return type(item) == "string" and item or item.label
end

local function item_section(item)
  return type(item) == "table" and item.section or nil
end

local function next_section(item, prev)
  local s = item_section(item)
  if s and s ~= prev then
    return s
  end
  return nil
end

local function section_rows(items)
  local n = 0
  local prev = nil
  for _, item in ipairs(items) do
    local s = next_section(item, prev)
    if s then
      n = n + 1
      prev = s
    end
  end
  if n == 0 then
    return 0
  end
  return item_section(items[1]) and 2 * n - 1 or 2 * n
end

local function filter_items(items, query)
  local words = split_words(query)
  if #words == 0 then
    local indices = {}
    for i = 1, #items do
      indices[i] = i
    end
    return items, indices
  end
  local matches = {}
  for index, item in ipairs(items) do
    local section = item_section(item)
    local label = item_label(item)
    local score = 0
    for _, word in ipairs(words) do
      local _, label_score = fuzzy_match(label, word)
      local section_score = nil
      if section then
        _, section_score = fuzzy_match(section, word)
      end
      local word_score = math.min(label_score or math.huge, section_score or math.huge)
      if word_score == math.huge then
        score = nil
        break
      end
      score = score + word_score
    end
    if score then
      matches[#matches + 1] = { item = item, index = index, score = score }
    end
  end
  local groups = {}
  for _, match in ipairs(matches) do
    local section = item_section(match.item)
    local group = groups[#groups]
    if not group or group.section ~= section then
      group = { section = section, matches = {} }
      groups[#groups + 1] = group
    end
    group.matches[#group.matches + 1] = match
  end
  local filtered, indices = {}, {}
  for _, group in ipairs(groups) do
    table.sort(group.matches, function(a, b)
      if a.score ~= b.score then
        return a.score < b.score
      end
      return a.index < b.index
    end)
    for _, match in ipairs(group.matches) do
      filtered[#filtered + 1] = match.item
      indices[#indices + 1] = match.index
    end
  end
  return filtered, indices
end

local function render_lines(items, selected, width, query)
  width = width or 80
  local words = split_words(query)
  local lines = {}
  local item_lines = {}
  local prev_section = nil
  for i, item in ipairs(items) do
    local label = item_label(item)
    local detail = type(item) == "table" and item.detail or nil
    local section = next_section(item, prev_section)
    local is_sel = (i == selected)
    local style = is_sel and "selected" or "item"
    local detail_style = is_sel and "selected" or "dim"
    local match_style = is_sel and "match_selected" or "match"

    if section then
      if #lines > 0 then
        lines[#lines + 1] = {}
      end
      local header = { { "  " .. section, "keybind_section" } }
      local section_detail = type(item) == "table" and item.section_detail or nil
      if section_detail then
        header[#header + 1] = { " " .. section_detail, "dim" }
      end
      lines[#lines + 1] = header
      prev_section = section
    end

    item_lines[i] = #lines + 1

    local spans = highlight_spans(label, words, style, match_style)
    if spans[1][2] == style then
      spans[1][1] = "  " .. spans[1][1]
    else
      table.insert(spans, 1, { "  ", style })
    end

    if detail then
      local pad = width - 2 - #label - #detail - DETAIL_RIGHT_PAD
      if pad < 1 then
        pad = 1
      end
      spans[#spans + 1] = { string.rep(" ", pad), style }
      spans[#spans + 1] = { detail, detail_style }
      spans[#spans + 1] = { string.rep(" ", DETAIL_RIGHT_PAD), style }
    else
      local trail = width - 2 - #label
      if trail > 0 then
        spans[#spans + 1] = { string.rep(" ", trail), style }
      end
    end

    lines[#lines + 1] = spans
  end
  return lines, item_lines
end

-- Open a fuzzy-filter picker in a floating window and block until the user
-- decides. {items} is a list of strings or { label, detail? } tables. {opts}:
-- title, footer, cursor (initial index), submit_keys (extra submit keys
-- besides enter), on_change(item, index), notify_initial (call on_change for
-- the initial item), on_timeout(), and timeout_ms.
-- Returns { type = "choice"|"delete", index } or { type = "close" }.
function ListPicker.open(items, opts)
  opts = opts or {}
  local submit_keys = { enter = true }
  if opts.submit_keys then
    for _, k in ipairs(opts.submit_keys) do
      submit_keys[k] = true
    end
  end
  local width
  local input = TextInput.new()
  local filtered, original_indices = filter_items(items, "")

  local cursor = math.max(math.min(opts.cursor or 1, #filtered), 1)
  local item_lines = {}

  local function build_lines()
    local content
    if #filtered == 0 then
      content = { { { NO_MATCHES_LABEL, "dim" } } }
      item_lines = {}
    else
      content, item_lines = render_lines(filtered, cursor, width, input:value())
    end
    local r = input:render("\xe2\x9d\xaf ")
    for _, ln in ipairs(r.lines) do
      content[#content + 1] = ln
    end
    return content
  end

  local buf = maki.ui.buf()

  local border_chrome = 2
  local content_h = #items + section_rows(items) + 1
  local total_h = content_h + border_chrome

  local win = maki.ui.open_win(buf, {
    title = opts.title,
    footer = opts.footer,
    height = total_h,
    reserved_bottom = 1,
  })

  width = win.width
  local height = win.height
  local confirming = nil

  local function move_cursor(to, notify, previous_index)
    previous_index = previous_index or original_indices[cursor]
    if #filtered > 0 then
      cursor = math.max(math.min(to, #filtered), 1)
    end
    buf:set_lines(build_lines())
    if item_lines[cursor] then
      win:set_cursor(item_lines[cursor])
      if notify and opts.on_change and original_indices[cursor] ~= previous_index then
        opts.on_change(filtered[cursor], original_indices[cursor])
      end
    end
    confirming = nil
  end

  local function page_size()
    return math.max(height - 2, 1)
  end

  buf:set_lines(build_lines())
  if #filtered > 0 then
    move_cursor(cursor)
    if opts.notify_initial and opts.on_change then
      opts.on_change(filtered[cursor], original_indices[cursor])
    end
  end

  while true do
    local ev = win:recv(opts.timeout_ms)
    if not ev or ev.type == "close" then
      return { type = "close" }
    end

    if ev.type == "timeout" then
      if opts.on_timeout then
        opts.on_timeout()
      end
      buf:set_lines(build_lines())
      if item_lines[cursor] then
        win:set_cursor(item_lines[cursor])
      end
    elseif ev.type == "resize" then
      width = ev.width
      height = ev.height
      move_cursor(cursor)
    elseif ev.type == "key" then
      if ev.key == "up" then
        move_cursor((cursor - 2) % math.max(#filtered, 1) + 1, true)
      elseif ev.key == "down" then
        move_cursor(cursor % math.max(#filtered, 1) + 1, true)
      elseif ev.key == "pageup" then
        move_cursor(cursor - page_size(), true)
      elseif ev.key == "pagedown" then
        move_cursor(cursor + page_size(), true)
      elseif ev.key == "esc" or ev.key == "ctrl+c" then
        win:close()
        return { type = "close" }
      elseif ev.key == "ctrl+d" then
        if #filtered > 0 then
          if confirming == cursor then
            win:close()
            return { type = "delete", index = original_indices[cursor] }
          else
            confirming = cursor
            maki.ui.flash("Press Ctrl+D again to delete")
          end
        end
      elseif submit_keys[ev.key] then
        if #filtered > 0 then
          win:close()
          return { type = "choice", index = original_indices[cursor] }
        end
      else
        local result = input:handle_key(ev.key)
        if result == TextInput.Result.CHANGED then
          local previous_index = original_indices[cursor]
          filtered, original_indices = filter_items(items, input:value())
          move_cursor(1, true, previous_index)
        elseif result == TextInput.Result.MOVED then
          move_cursor(cursor, true)
        end
      end
    end
  end
end

return ListPicker
