use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};
use maki_agent::skills::SkillInfo;
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Matcher, Nucleo, Utf32Str};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use tracing::warn;
use unicode_width::UnicodeWidthChar;

use crate::repaint::{Cadence, Dirty};
use crate::text_buffer::TextBuffer;
use crate::theme;

const WALKER_CRASHED_MSG: &str = "File scanner crashed";
const COL_GAP: usize = 2;
const PENDING_DEBOUNCE_MS: u128 = 100;
const MAX_MATERIALIZED: u32 = 640;

/// Byte range of the `@`-token under the cursor (including its leading `@`),
/// or `None` when the most recent `@` does not begin a token (e.g. `foo@bar`).
pub fn at_token_range(line: &str, cursor_chars: usize) -> Option<(usize, usize)> {
    let cursor_byte = TextBuffer::char_to_byte(line, cursor_chars);
    let before = &line[..cursor_byte];
    let bytes = before.as_bytes();
    let mut i = before.len();
    while i > 0 {
        i -= 1;
        if bytes[i] != b'@' {
            continue;
        }
        let token_start = before[..i]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        if token_start {
            return Some((i, cursor_byte));
        }
    }
    None
}

#[derive(Debug, Clone)]
pub enum CompletionItem {
    File { path: String },
    Skill { name: String, description: String },
}

impl CompletionItem {
    /// Text that replaces the whole `@`-token (including its leading `@`).
    pub(crate) fn replacement(&self) -> String {
        match self {
            CompletionItem::File { path } => format!("@{path}"),
            CompletionItem::Skill { name, .. } => format!("@skill:{name}"),
        }
    }

    fn display(&self) -> String {
        match self {
            CompletionItem::File { path } => path.clone(),
            CompletionItem::Skill { name, description } => {
                if description.is_empty() {
                    format!("skill:{name}")
                } else {
                    format!("skill:{name}  {description}")
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    item: CompletionItem,
    indices: Vec<u32>,
}

#[derive(Debug)]
pub enum CompletionAction {
    Consumed,
    Select(CompletionItem),
    Close,
    Passthrough,
}

struct Session {
    nucleo: Nucleo<()>,
    matcher: Matcher,
    skills: Vec<SkillInfo>,
    file_matches: Vec<Candidate>,
    skill_matches: Vec<Candidate>,
    matches: Vec<Candidate>,

    selected: usize,
    /// Grid layout: columns used, and scroll/viewport in whole rows.
    cols: usize,
    scroll_offset: usize,
    viewport_height: usize,

    cancel: Arc<AtomicBool>,
    done_rx: flume::Receiver<()>,
    started_at: Instant,

    walking: bool,
    matching: bool,
    visible: bool,

    skills_only: bool,
    token_byte_range: (usize, usize),
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct FileCompletionMenu {
    session: Option<Session>,
}

impl FileCompletionMenu {
    pub fn new() -> Self {
        Self { session: None }
    }

    pub fn open(
        &mut self,
        cwd: &str,
        skills: Vec<SkillInfo>,
        query: &str,
        token_byte_range: (usize, usize),
    ) {
        self.close();

        let Some((nucleo, done_rx, cancel_clone)) = super::file_picker::spawn_file_walker(cwd)
        else {
            return;
        };

        let session = Session {
            nucleo,
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            skills,
            file_matches: Vec::new(),
            skill_matches: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            cols: 1,
            scroll_offset: 0,
            viewport_height: 0,
            cancel: cancel_clone,
            done_rx,
            started_at: Instant::now(),
            walking: true,
            matching: false,
            visible: false,
            skills_only: false,
            token_byte_range,
        };
        self.session = Some(session);
        self.sync_query(query);
    }

    pub fn close(&mut self) {
        self.session = None;
    }

    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    #[cfg(test)]
    pub fn has_selectable(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.visible && !s.matches.is_empty())
    }

    pub fn token_byte_range(&self) -> (usize, usize) {
        self.session.as_ref().map_or((0, 0), |s| s.token_byte_range)
    }

    pub fn set_token_byte_range(&mut self, range: (usize, usize)) {
        if let Some(s) = &mut self.session {
            s.token_byte_range = range;
        }
    }

    pub fn sync_query(&mut self, query: &str) {
        let Some(s) = &mut self.session else {
            return;
        };
        s.skills_only = query.starts_with("skill:");
        let file_query = if s.skills_only { "" } else { query };
        s.nucleo.pattern.reparse(
            0,
            file_query,
            CaseMatching::Smart,
            Normalization::Smart,
            false,
        );
        s.selected = 0;
        s.scroll_offset = 0;

        let skill_needle: &str = if s.skills_only {
            query.strip_prefix("skill:").unwrap_or("")
        } else {
            query
        };
        s.skill_matches = match_skills(&mut s.matcher, &s.skills, skill_needle);
        rebuild_combined(s);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> CompletionAction {
        let Some(s) = &mut self.session else {
            return CompletionAction::Close;
        };

        match key.code {
            KeyCode::Esc => return CompletionAction::Close,
            KeyCode::Enter | KeyCode::Tab => {
                if !s.visible {
                    return CompletionAction::Passthrough;
                }
                return match s.matches.get(s.selected).map(|c| c.item.clone()) {
                    Some(item) => CompletionAction::Select(item),
                    None => CompletionAction::Passthrough,
                };
            }
            KeyCode::Up => move_selection(s, -1),
            KeyCode::Down => move_selection(s, 1),
            _ if super::is_ctrl(&key) => return CompletionAction::Consumed,
            _ => return CompletionAction::Passthrough,
        }
        CompletionAction::Consumed
    }

    pub fn cadence(&self) -> Cadence {
        let Some(s) = self.session.as_ref() else {
            return Cadence::IDLE;
        };
        Cadence::any([
            Cadence::when(s.visible && s.walking, Cadence::SPINNER),
            Cadence::when(s.matching && !s.walking, Cadence::PENDING),
        ])
    }

    pub fn tick(&mut self) -> (Dirty, Option<String>) {
        let Some(s) = &mut self.session else {
            return (Dirty::NO, None);
        };

        let status = s.nucleo.tick(0);
        s.matching = status.running;
        let mut dirty = Dirty::from(status.changed);

        if s.walking {
            match s.done_rx.try_recv() {
                Ok(()) => {
                    s.walking = false;
                    dirty = Dirty::YES;
                }
                Err(flume::TryRecvError::Disconnected) => {
                    warn!("{WALKER_CRASHED_MSG}: walker thread panicked");
                    self.session = None;
                    return (Dirty::YES, Some(WALKER_CRASHED_MSG.into()));
                }
                Err(flume::TryRecvError::Empty) => {}
            }
        }

        if !s.visible {
            let has_files = s.nucleo.injector().injected_items() > 0;
            let has_skills = !s.skill_matches.is_empty();
            let debounce_elapsed = s.started_at.elapsed().as_millis() >= PENDING_DEBOUNCE_MS;

            if has_files || has_skills || (s.walking && debounce_elapsed) {
                s.visible = true;
                dirty = Dirty::YES;
            }
        }

        if status.changed
            && let Some(s) = self.session.as_mut()
        {
            refresh_file_matches(s);
            rebuild_combined(s);
            clamp_selection(s);
        }

        (dirty, None)
    }

    pub fn view(&mut self, frame: &mut Frame, input_area: Rect) -> Option<Rect> {
        let s = match &mut self.session {
            Some(s) if s.visible && !s.matches.is_empty() => s,
            _ => return None,
        };

        let len = s.matches.len();
        // Cap taken from the screen height: the popup is a compact overlay, not
        // a full-height list.
        let max_height = ((frame.area().height as u32 * 30 / 100) as u16).max(2);
        let avail = max_height.saturating_sub(1) as usize;
        if avail == 0 || input_area.y == 0 {
            return None;
        }

        let cols = if len <= avail {
            1
        } else if len <= avail.saturating_mul(2) {
            2
        } else {
            len.min(3)
        };
        s.cols = cols;
        let total_rows = len.div_ceil(cols);
        let view_rows = avail.min(total_rows);
        s.viewport_height = view_rows;
        ensure_visible(s);

        let budget = (input_area.width as usize).saturating_sub(COL_GAP * (cols - 1)) / cols;
        let col_widths: Vec<usize> = (0..cols)
            .map(|j| {
                s.matches
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i % cols == j)
                    .map(|(_, c)| c.item.display().chars().count())
                    .max()
                    .unwrap_or(0)
                    .min(budget)
            })
            .collect();
        let total_width = col_widths.iter().sum::<usize>() + COL_GAP * (cols - 1);
        let popup_height = (view_rows as u16 + 1).min(max_height);
        let popup = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(popup_height),
            width: total_width.clamp(1, input_area.width.max(1) as usize) as u16,
            height: popup_height,
        };

        let t = theme::current();
        let lines = build_grid(s, view_rows, cols, &col_widths, &t);

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::TOP)
            .style(Style::new().bg(t.background));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        frame.render_widget(Paragraph::new(lines), inner);

        Some(popup)
    }
}

fn match_skills(matcher: &mut Matcher, skills: &[SkillInfo], needle: &str) -> Vec<Candidate> {
    if skills.is_empty() {
        return Vec::new();
    }
    let mut needle_buf = Vec::new();
    let needle_utf32 = Utf32Str::new(needle, &mut needle_buf);
    let mut indices = Vec::new();
    let mut out = Vec::new();
    for skill in skills {
        let mut hay_buf = Vec::new();
        let hay = Utf32Str::new(skill.name.as_str(), &mut hay_buf);
        indices.clear();
        let matched = matcher.fuzzy_indices(hay, needle_utf32, &mut indices);
        if matched.is_some() {
            out.push(Candidate {
                item: CompletionItem::Skill {
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                },
                indices: mem::take(&mut indices),
            });
        }
    }
    out
}

fn refresh_file_matches(s: &mut Session) {
    let snapshot = s.nucleo.snapshot();
    let count = snapshot.matched_item_count().min(MAX_MATERIALIZED);

    s.file_matches.clear();

    let pattern = snapshot.pattern();
    let has_pattern = !pattern.column_pattern(0).atoms.is_empty();
    let mut indices_buf = Vec::new();

    for item in snapshot.matched_items(0..count) {
        let col = &item.matcher_columns[0];
        let path = col.to_string();

        let indices = if has_pattern {
            indices_buf.clear();
            pattern
                .column_pattern(0)
                .indices(col.slice(..), &mut s.matcher, &mut indices_buf);
            mem::take(&mut indices_buf)
        } else {
            Vec::new()
        };

        s.file_matches.push(Candidate {
            item: CompletionItem::File { path },
            indices,
        });
    }
}

fn rebuild_combined(s: &mut Session) {
    s.matches.clear();
    if s.skills_only {
        s.matches.extend(s.skill_matches.iter().cloned());
        return;
    }
    s.matches.extend(s.file_matches.iter().cloned());
    s.matches.extend(s.skill_matches.iter().cloned());
}

fn move_selection(s: &mut Session, rows: isize) {
    if s.matches.is_empty() {
        return;
    }
    let cols = s.cols.max(1);
    let last = s.matches.len() - 1;
    let col = (s.selected % cols).min(last);
    let last_row = last / cols;
    let row = ((s.selected / cols) as isize + rows).clamp(0, last_row as isize) as usize;
    s.selected = (row * cols + col).min(last);
    ensure_visible(s);
}

fn clamp_selection(s: &mut Session) {
    if s.matches.is_empty() {
        s.selected = 0;
        s.scroll_offset = 0;
    } else {
        s.selected = s.selected.min(s.matches.len() - 1);
        ensure_visible(s);
    }
}

fn ensure_visible(s: &mut Session) {
    let cols = s.cols.max(1);
    let total_rows = s.matches.len().div_ceil(cols);
    let vh = s.viewport_height.max(1);

    if total_rows > vh {
        s.scroll_offset = s.scroll_offset.min(total_rows - vh);
    } else {
        s.scroll_offset = 0;
    }

    let row = s.selected / cols;
    if row < s.scroll_offset {
        s.scroll_offset = row;
    } else if row >= s.scroll_offset + vh {
        s.scroll_offset = row + 1 - vh;
    }
}

fn build_grid<'a>(
    s: &Session,
    view_rows: usize,
    cols: usize,
    col_widths: &[usize],
    t: &'a theme::Theme,
) -> Vec<Line<'a>> {
    let len = s.matches.len();
    let mut lines = Vec::with_capacity(view_rows);

    for r in 0..view_rows {
        let row = s.scroll_offset + r;
        let mut spans = Vec::new();
        for (j, width) in col_widths.iter().enumerate() {
            let idx = row * cols + j;
            if idx < len {
                spans.extend(cell_line(&s.matches[idx], *width, idx == s.selected, t).spans);
            } else {
                spans.push(Span::raw(" ".repeat(*width)));
            }
            if j + 1 < cols {
                spans.push(Span::raw(" ".repeat(COL_GAP)));
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn cell_line<'a>(c: &Candidate, width: usize, selected: bool, t: &'a theme::Theme) -> Line<'a> {
    let base = if selected { t.item_selected } else { t.item };
    let text = c.item.display();
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut used = 0usize;

    match &c.item {
        CompletionItem::File { .. } => {
            let hl = base
                .fg(t.accent.fg.unwrap_or_default())
                .add_modifier(Modifier::BOLD);
            let mut in_match = false;
            let mut run = String::new();
            for (i, ch) in text.chars().enumerate() {
                let cw = ch.width().unwrap_or(0);
                if used + cw > width {
                    break;
                }
                used += cw;
                let is_match = c.indices.binary_search(&(i as u32)).is_ok();
                if is_match != in_match && !run.is_empty() {
                    spans.push(Span::styled(
                        mem::take(&mut run),
                        if in_match { hl } else { base },
                    ));
                }
                in_match = is_match;
                run.push(ch);
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, if in_match { hl } else { base }));
            }
        }
        CompletionItem::Skill { .. } => {
            let run: String = text.chars().take(width).collect();
            used = run.chars().count();
            spans.push(Span::styled(run, base));
        }
    }

    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers};
    use nucleo::{Config, Nucleo};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    use crate::text_buffer::TextBuffer;

    use super::*;
    use test_case::test_case;

    fn skill(name: &str) -> SkillInfo {
        SkillInfo {
            name: name.into(),
            description: format!("desc {name}"),
        }
    }

    fn session_with_skills(skills: Vec<SkillInfo>) -> FileCompletionMenu {
        let nucleo = Nucleo::new(Config::DEFAULT.match_paths(), Arc::new(|| {}), None, 1);
        let (_, done_rx) = flume::bounded(1);
        let mut menu = FileCompletionMenu::new();
        menu.session = Some(Session {
            nucleo,
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            skills,
            file_matches: Vec::new(),
            skill_matches: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            cols: 1,
            scroll_offset: 0,
            viewport_height: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            done_rx,
            started_at: Instant::now(),
            walking: true,
            matching: false,
            visible: false,
            skills_only: false,
            token_byte_range: (0, 0),
        });
        menu
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test_case("@", 1        => Some((0, 1))  ; "bare_at")]
    #[test_case("@src", 4      => Some((0, 4))  ; "query_no_prefix")]
    #[test_case(" @src", 5     => Some((1, 5))  ; "space_prefixed")]
    #[test_case("prefix @src", 11 => Some((7, 11)) ; "word_then_token")]
    #[test_case("foo@bar", 7   => None          ; "mid_word_at_rejected")]
    #[test_case("@src tail", 3 => Some((0, 3))  ; "cursor_mid_token")]
    fn at_token_cases(line: &str, cursor: usize) -> Option<(usize, usize)> {
        at_token_range(line, cursor)
    }

    #[test]
    fn insertion_replaces_token_keeps_single_at() {
        let mut buf = TextBuffer::new("foo @xyz".into());
        let range = at_token_range(&buf.lines()[0], 8).unwrap();
        let item = CompletionItem::File {
            path: "docs/read me.md".into(),
        };
        buf.replace_range_on_current_line(range.0, range.1, &item.replacement());
        assert_eq!(buf.value(), "foo @docs/read me.md");
    }

    #[test]
    fn skill_replacement_uses_skill_prefix() {
        let mut buf = TextBuffer::new("foo @".into());
        let range = at_token_range(&buf.lines()[0], 5).unwrap();
        let item = CompletionItem::Skill {
            name: "review".into(),
            description: String::new(),
        };
        buf.replace_range_on_current_line(range.0, range.1, &item.replacement());
        assert_eq!(buf.value(), "foo @skill:review");
    }

    #[test]
    fn cursor_lands_after_insertion() {
        let mut buf = TextBuffer::new("foo @xyz".into());
        let range = at_token_range(&buf.lines()[0], 8).unwrap();
        assert_eq!(range, (4, 8)); // `foo @xyz` -> token is `@xyz`
        let item = CompletionItem::File {
            path: "main.rs".into(),
        };
        buf.replace_range_on_current_line(range.0, range.1, &item.replacement());
        assert_eq!(buf.value(), "foo @main.rs");
        assert_eq!(buf.x(), 12); // cursor just past the inserted `@main.rs`
    }

    #[test]
    fn plain_query_matches_skills_and_files_mode() {
        let mut menu = session_with_skills(vec![skill("review"), skill("tests")]);
        menu.sync_query("rev");
        let s = menu.session.as_ref().unwrap();
        assert!(!s.skills_only);
        assert_eq!(
            s.skill_matches
                .iter()
                .map(|c| c.item_display_name())
                .collect::<Vec<_>>(),
            vec!["review".to_string()]
        );
    }

    #[test]
    fn skill_prefix_filters_to_skills_only() {
        let mut menu = session_with_skills(vec![skill("review"), skill("tests")]);
        menu.sync_query("skill:");
        let s = menu.session.as_ref().unwrap();
        assert!(s.skills_only);
        assert_eq!(s.skill_matches.len(), 2);

        menu.sync_query("skill:t");
        let s = menu.session.as_ref().unwrap();
        let names = s
            .skill_matches
            .iter()
            .map(|c| c.item_display_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["tests".to_string()]);
    }

    #[test]
    fn skills_complete_without_prefix() {
        let mut menu = session_with_skills(vec![skill("review")]);
        menu.sync_query("rev");
        let s = menu.session.as_ref().unwrap();
        let offered: Vec<_> = s.matches.iter().map(|c| c.item_display_name()).collect();
        assert!(offered.contains(&"review".to_string()));
    }

    fn menu_with_matches(count: usize) -> FileCompletionMenu {
        let mut menu = session_with_skills(Vec::new());
        let s = menu.session.as_mut().unwrap();
        s.matches = (0..count)
            .map(|i| Candidate {
                item: CompletionItem::File {
                    path: format!("file{i}"),
                },
                indices: Vec::new(),
            })
            .collect();
        menu
    }

    #[test_case(0, -5, 0    ; "clamps_at_start")]
    #[test_case(4, 5, 4     ; "clamps_at_end")]
    #[test_case(2, 1, 3     ; "moves_down")]
    #[test_case(2, -1, 1    ; "moves_up")]
    fn move_selection_behavior(start: usize, delta: isize, expected: usize) {
        let mut menu = menu_with_matches(5);
        let s = menu.session.as_mut().unwrap();
        s.viewport_height = 10;
        s.selected = start;
        move_selection(s, delta);
        assert_eq!(s.selected, expected);
    }

    #[test]
    fn enter_returns_select() {
        let mut menu = menu_with_matches(3);
        menu.session.as_mut().unwrap().visible = true;
        match menu.handle_key(key(KeyCode::Enter)) {
            CompletionAction::Select(CompletionItem::File { path }) => assert_eq!(path, "file0"),
            other => panic!("expected Select, got {other:?}"),
        }
    }

    #[test]
    fn esc_returns_close() {
        let mut menu = menu_with_matches(3);
        menu.session.as_mut().unwrap().visible = true;
        assert!(matches!(
            menu.handle_key(key(KeyCode::Esc)),
            CompletionAction::Close
        ));
    }

    #[test]
    fn other_keys_passthrough_and_updown_consumed() {
        let mut menu = menu_with_matches(3);
        menu.session.as_mut().unwrap().visible = true;
        assert!(matches!(
            menu.handle_key(key(KeyCode::Char('a'))),
            CompletionAction::Passthrough
        ));
        let sel = menu.session.as_ref().unwrap().selected;
        assert!(matches!(
            menu.handle_key(key(KeyCode::Down)),
            CompletionAction::Consumed
        ));
        assert_eq!(menu.session.as_ref().unwrap().selected, sel + 1);
    }

    #[test]
    fn view_popup_above_input_area() {
        let mut menu = menu_with_matches(3);
        let s = menu.session.as_mut().unwrap();
        s.visible = true;
        s.walking = false;
        s.started_at = Instant::now() - std::time::Duration::from_secs(1);
        let backend = TestBackend::new(40, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let input_area = Rect {
            x: 0,
            y: 10,
            width: 40,
            height: 3,
        };
        terminal
            .draw(|frame| {
                let rect = menu.view(frame, input_area).unwrap();
                assert_eq!(rect.y, 10 - rect.height);
            })
            .unwrap();
    }

    impl Candidate {
        fn item_display_name(&self) -> String {
            match &self.item {
                CompletionItem::File { path } => path.clone(),
                CompletionItem::Skill { name, .. } => name.clone(),
            }
        }
    }
}
