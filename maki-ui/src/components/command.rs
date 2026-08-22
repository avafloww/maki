use std::collections::HashSet;
use std::iter;
use std::mem;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use maki_agent::command::CustomCommand;
use maki_agent::{CancelToken, CancelTrigger};
use maki_agent::{McpPromptInfo, McpSnapshotReader};
use maki_lua::{
    CommandArgumentContext, CommandArgumentItem, CommandArgumentLifecycle, EventHandle,
    LuaCommandInfo, LuaCommandReader,
};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Matcher, Nucleo, Utf32String};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::{repaint::Dirty, theme};

const TICK_TIMEOUT_MS: u64 = 10;
/// Note appended to builtin alias rows: `(Alias for /new)`.
const ALIAS_NOTE: &str = " (Alias for ";

pub struct BuiltinCommand {
    pub name: &'static str,
    pub description: &'static str,
    pub max_args: usize,
    pub aliases: &'static [&'static str],
}

pub const BUILTIN_COMMANDS: &[BuiltinCommand] = &[
    BuiltinCommand {
        name: "/tasks",
        description: "Browse and search tasks",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/compact",
        description: "Summarize and compact conversation history",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/new",
        description: "Start a new session",
        max_args: 0,
        aliases: &["/clear"],
    },
    BuiltinCommand {
        name: "/help",
        description: "Show keybindings",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/usage",
        description: "Show token usage breakdown",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/queue",
        description: "Remove items from queue",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/model",
        description: "Switch model",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/theme",
        description: "Switch color theme",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/mcp",
        description: "Configure MCP servers",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/login",
        description: "Authenticate with an LLM provider",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/cd",
        description: "Change working directory",
        max_args: 1,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/btw",
        description: "Ask a quick question (no tools, no history pollution)",
        max_args: usize::MAX,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/yolo",
        description: "Toggle YOLO mode (skip all permission prompts)",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/thinking",
        description: "Toggle extended thinking (off, adaptive, effort level, or budget)",
        max_args: 1,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/fast",
        description: "Toggle Anthropic fast mode (Opus only)",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/workflow",
        description: "Toggle workflow mode (task callable inside code_execution)",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/exit",
        description: "Exit the application",
        max_args: 0,
        aliases: &[],
    },
    BuiltinCommand {
        name: "/reload",
        description: "Reload plugins and config",
        max_args: 0,
        aliases: &[],
    },
];

pub struct ParsedCommand {
    pub name: String,
    pub args: String,
}

pub enum CommandAction {
    Consumed,
    SelectionChanged,
    Execute(ParsedCommand),
    AcceptArgument { text: String, cursor: usize },
    Complete { text: String, cursor: usize },
    Passthrough,
}

#[derive(Clone)]
enum CommandType {
    Builtin(&'static BuiltinCommand),
    Custom(usize),
    McpPrompt(usize),
    Lua(usize),
}

struct CommandItem {
    name: String,
    max_args: usize,
    command_type: CommandType,
}

struct Match {
    display: String,
    command_type: CommandType,
    indices: Vec<u32>,
}

pub struct CommandPalette {
    selected: usize,
    filtered: Vec<Match>,
    custom: Arc<[CustomCommand]>,
    mcp_reader: McpSnapshotReader,
    mcp_prompts: Vec<McpPromptInfo>,
    mcp_generation: u64,
    lua_reader: LuaCommandReader,
    lua_commands: Vec<LuaCommandInfo>,
    lua_generation: u64,
    nucleo: Nucleo<CommandItem>,
    matcher: Matcher,
    current_arg_count: usize,
    argument_items: Vec<ArgumentMatch>,
    argument_range: Option<(usize, usize)>,
    argument_session: u64,
    argument_generation: u64,
    argument_context: Option<CommandArgumentContext>,
    pending_arguments: Option<PendingArguments>,
    pending_cancel: Option<CancelTrigger>,
    lifecycle_cancel: Option<CancelTrigger>,
    accepted_argument_input: Option<String>,
}

struct ArgumentMatch {
    item: CommandArgumentItem,
    indices: Vec<u32>,
}

struct PendingArguments {
    rx: flume::Receiver<Vec<CommandArgumentItem>>,
    generation: u64,
    query: String,
    range: (usize, usize),
}

impl CommandPalette {
    pub fn new(
        custom_commands: Arc<[CustomCommand]>,
        mcp_reader: McpSnapshotReader,
        lua_reader: LuaCommandReader,
    ) -> Self {
        let snap = mcp_reader.load();
        let mcp_generation = snap.generation;
        let prompts = snap.prompts.clone();

        let lua_snap = lua_reader.load();
        let lua_generation = lua_snap.generation;
        let lua_commands = lua_snap.commands.clone();

        let nucleo = Self::build_nucleo(&custom_commands, &prompts, &lua_commands);
        Self {
            selected: 0,
            filtered: Vec::new(),
            custom: custom_commands,
            mcp_reader,
            mcp_prompts: prompts,
            mcp_generation,
            lua_reader,
            lua_commands,
            lua_generation,
            nucleo,
            matcher: Matcher::new(Config::DEFAULT),
            current_arg_count: 0,
            argument_items: Vec::new(),
            argument_range: None,
            argument_session: 0,
            argument_generation: 0,
            argument_context: None,
            pending_arguments: None,
            pending_cancel: None,
            lifecycle_cancel: None,
            accepted_argument_input: None,
        }
    }

    /// Every command the palette knows, in display order. The one place that
    /// enumerates the four sources: matching and name lookup both read it.
    fn items<'a>(
        custom_commands: &'a [CustomCommand],
        mcp_prompts: &'a [McpPromptInfo],
        lua_commands: &'a [LuaCommandInfo],
    ) -> impl Iterator<Item = CommandItem> + 'a {
        let builtins = BUILTIN_COMMANDS.iter().flat_map(|cmd| {
            iter::once(cmd.name)
                .chain(cmd.aliases.iter().copied())
                .map(|name| CommandItem {
                    name: name.to_string(),
                    max_args: cmd.max_args,
                    command_type: CommandType::Builtin(cmd),
                })
        });
        let custom = custom_commands
            .iter()
            .enumerate()
            .map(|(i, cmd)| CommandItem {
                name: cmd.display_name(),
                max_args: if cmd.has_args() { usize::MAX } else { 0 },
                command_type: CommandType::Custom(i),
            });
        let prompts = mcp_prompts.iter().enumerate().map(|(i, p)| CommandItem {
            name: format!("/{}", p.display_name),
            max_args: if p.arguments.is_empty() {
                0
            } else {
                usize::MAX
            },
            command_type: CommandType::McpPrompt(i),
        });
        let lua = lua_commands.iter().enumerate().map(|(i, cmd)| CommandItem {
            name: cmd.name.to_string(),
            max_args: cmd.max_args,
            command_type: CommandType::Lua(i),
        });
        builtins.chain(custom).chain(prompts).chain(lua)
    }

    fn build_nucleo(
        custom_commands: &[CustomCommand],
        mcp_prompts: &[McpPromptInfo],
        lua_commands: &[LuaCommandInfo],
    ) -> Nucleo<CommandItem> {
        let nucleo = Nucleo::new(Config::DEFAULT, Arc::new(|| {}), None, 1);
        let injector = nucleo.injector();

        let overridden: HashSet<String> = HashSet::from_iter(
            lua_commands
                .iter()
                .map(|c| c.name.to_string())
                .chain(custom_commands.iter().map(|c| c.display_name()))
                .chain(mcp_prompts.iter().map(|p| format!("/{}", p.display_name))),
        );

        for item in Self::items(custom_commands, mcp_prompts, lua_commands) {
            if matches!(item.command_type, CommandType::Builtin(_))
                && overridden.contains(&item.name)
            {
                continue;
            }
            injector.push(item, |item, cols| {
                cols[0] = Utf32String::from(item.name.as_str());
            });
        }

        nucleo
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        input: &str,
        handle: &EventHandle,
    ) -> CommandAction {
        if self.accepted_argument_input.as_deref() == Some(input) {
            return if key.code == KeyCode::Enter {
                match self.confirm(input) {
                    Some(cmd) => {
                        self.close(handle);
                        CommandAction::Execute(cmd)
                    }
                    None => CommandAction::Consumed,
                }
            } else {
                CommandAction::Passthrough
            };
        }
        if !self.is_active() {
            return CommandAction::Passthrough;
        }
        match key.code {
            KeyCode::Up => {
                if !self.argument_items.is_empty() {
                    self.selected = self.selected.saturating_sub(1);
                    self.notify_lifecycle(handle, CommandArgumentLifecycle::Highlight);
                    CommandAction::Consumed
                } else {
                    self.move_up();
                    CommandAction::SelectionChanged
                }
            }
            KeyCode::Down => {
                if !self.argument_items.is_empty() {
                    self.selected = (self.selected + 1).min(self.argument_items.len() - 1);
                    self.notify_lifecycle(handle, CommandArgumentLifecycle::Highlight);
                    CommandAction::Consumed
                } else {
                    self.move_down();
                    CommandAction::SelectionChanged
                }
            }
            KeyCode::Esc => {
                self.close(handle);
                CommandAction::Consumed
            }
            KeyCode::Enter => {
                if let Some((range, item)) = self
                    .argument_range
                    .zip(self.argument_items.get(self.selected))
                {
                    let insertion = &item.item.insertion;
                    let text = self.replace_argument(input, insertion);
                    let cursor = range.0 + insertion.len();
                    self.notify_lifecycle(handle, CommandArgumentLifecycle::Accept);
                    self.argument_context = None;
                    self.lifecycle_cancel = None;
                    self.argument_items.clear();
                    self.argument_range = None;
                    self.pending_arguments = None;
                    self.accepted_argument_input = Some(text.clone());
                    CommandAction::AcceptArgument { text, cursor }
                } else {
                    // Not settled: no items, or stale ones still on screen
                    // while the current query is in flight.
                    match self.confirm(input) {
                        Some(cmd) => {
                            self.close(handle);
                            CommandAction::Execute(cmd)
                        }
                        None => CommandAction::Consumed,
                    }
                }
            }
            KeyCode::Tab => {
                if let Some((range, item)) = self
                    .argument_range
                    .zip(self.argument_items.get(self.selected))
                {
                    let insertion = item.item.insertion.clone();
                    self.notify_lifecycle(handle, CommandArgumentLifecycle::Accept);
                    self.argument_context = None;
                    self.lifecycle_cancel = None;
                    let text = self.replace_argument(input, &insertion);
                    self.accepted_argument_input = Some(text.clone());
                    return CommandAction::Complete {
                        text,
                        cursor: range.0 + insertion.len(),
                    };
                }
                if let Some(item) = self.filtered.get(self.selected) {
                    let name = item.display.clone();
                    let text = if self.item_has_args(item) {
                        format!("{name} ")
                    } else {
                        name
                    };
                    CommandAction::Complete {
                        cursor: text.len(),
                        text,
                    }
                } else {
                    CommandAction::Consumed
                }
            }
            _ => CommandAction::Passthrough,
        }
    }

    pub fn is_active(&self) -> bool {
        self.accepted_argument_input.is_none()
            && (!self.filtered.is_empty()
                || !self.argument_items.is_empty()
                || self.argument_context.is_some())
    }

    #[cfg(test)]
    pub(crate) fn argument_generation(&self) -> u64 {
        self.argument_generation
    }

    #[cfg(test)]
    pub(crate) fn set_argument_completion(
        &mut self,
        range: (usize, usize),
        item: CommandArgumentItem,
    ) {
        self.argument_range = Some(range);
        self.argument_items = vec![ArgumentMatch {
            item,
            indices: Vec::new(),
        }];
        self.selected = 0;
    }

    pub fn sync_arguments(&mut self, input: &str, cursor: usize, handle: &EventHandle, mode: &str) {
        self.pending_cancel.take();
        self.argument_generation = self.argument_generation.wrapping_add(1);
        // The previous items stay on screen until the new query lands; the
        // branches below cancel (and clear) them when they cannot survive.
        self.argument_range = None;
        self.pending_arguments = None;
        if self.accepted_argument_input.as_deref() == Some(input) {
            return;
        }
        self.accepted_argument_input = None;
        let command = self
            .filtered
            .get(self.selected)
            .map(|item| item.display.clone());
        let lua = command
            .as_deref()
            .and_then(|command| self.find_lua_command(command))
            .filter(|lua| lua.has_argument_completion)
            .map(|lua| Arc::clone(&lua.plugin));
        let Some((command, plugin, (start, end, arg, index))) = command
            .zip(lua)
            .zip(argument_at_cursor(input, cursor))
            .map(|((command, plugin), argument)| (command, plugin, argument))
        else {
            self.cancel_arguments(handle);
            return;
        };
        let same_session = self
            .argument_context
            .as_ref()
            .is_some_and(|context| context.command.as_ref() == command && context.plugin == plugin);
        if !same_session {
            self.cancel_arguments(handle);
            self.argument_session = self.argument_session.wrapping_add(1);
        }
        let context = CommandArgumentContext {
            command: Arc::from(command.as_str()),
            plugin,
            args: command_args(input).to_string(),
            arg: arg.clone(),
            index,
            mode: mode.to_string(),
            session: self.argument_session,
            generation: self.argument_generation,
        };
        let (cancel, token) = CancelToken::new();
        let Some(rx) = handle.collect_command_argument_items(context.clone(), token) else {
            return;
        };
        self.argument_context = Some(context);
        self.pending_cancel = Some(cancel);
        self.pending_arguments = Some(PendingArguments {
            rx,
            generation: self.argument_generation,
            query: arg,
            range: (start, end),
        });
    }

    pub fn poll_arguments(&mut self, handle: &EventHandle) -> Dirty {
        let Some(pending) = self.pending_arguments.take() else {
            return Dirty::NO;
        };
        let Ok(items) = pending.rx.try_recv() else {
            if !pending.rx.is_disconnected() {
                self.pending_arguments = Some(pending);
            }
            return Dirty::NO;
        };
        if pending.generation != self.argument_generation {
            return Dirty::NO;
        }
        self.argument_items.clear();
        self.selected = 0;
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = nucleo::pattern::Pattern::parse(
            &pending.query,
            CaseMatching::Ignore,
            Normalization::Smart,
        );
        for item in items {
            let mut indices = Vec::new();
            if pattern
                .indices(
                    Utf32String::from(item.label.as_str()).slice(..),
                    &mut matcher,
                    &mut indices,
                )
                .is_none()
            {
                continue;
            }
            self.argument_items.push(ArgumentMatch { item, indices });
        }
        self.argument_range = Some(pending.range);
        self.pending_cancel = None;
        if !self.argument_items.is_empty() {
            self.notify_lifecycle(handle, CommandArgumentLifecycle::Highlight);
        }
        Dirty::YES
    }

    fn notify_lifecycle(&mut self, handle: &EventHandle, event: CommandArgumentLifecycle) {
        let Some(context) = self.argument_context.clone() else {
            return;
        };
        let item = match event {
            CommandArgumentLifecycle::Cancel => None,
            CommandArgumentLifecycle::Highlight | CommandArgumentLifecycle::Accept => self
                .argument_items
                .get(self.selected)
                .map(|item| item.item.clone()),
        };
        self.lifecycle_cancel.take();
        if matches!(event, CommandArgumentLifecycle::Highlight) {
            let (cancel, token) = CancelToken::new();
            handle.command_argument_lifecycle(context, event, item, token);
            self.lifecycle_cancel = Some(cancel);
        } else {
            handle.command_argument_lifecycle(context, event, item, CancelToken::none());
        }
    }

    pub fn cancel_arguments(&mut self, handle: &EventHandle) {
        self.pending_cancel.take();
        self.lifecycle_cancel.take();
        if let Some(context) = self.argument_context.clone() {
            handle.command_argument_lifecycle(
                context,
                CommandArgumentLifecycle::Cancel,
                None,
                CancelToken::none(),
            );
        }
        self.argument_context = None;
        self.argument_items.clear();
        self.argument_range = None;
        self.pending_arguments = None;
    }

    fn replace_argument(&self, input: &str, replacement: &str) -> String {
        let Some((start, end)) = self.argument_range else {
            return input.to_string();
        };
        format!("{}{}{}", &input[..start], replacement, &input[end..])
    }

    pub fn sync(&mut self, input: &str) {
        let mcp_snap = self.mcp_reader.load();
        let lua_snap = self.lua_reader.load();
        if mcp_snap.generation != self.mcp_generation || lua_snap.generation != self.lua_generation
        {
            self.mcp_generation = mcp_snap.generation;
            self.mcp_prompts = mcp_snap.prompts.clone();
            self.lua_generation = lua_snap.generation;
            self.lua_commands = lua_snap.commands.clone();
            self.nucleo = Self::build_nucleo(&self.custom, &self.mcp_prompts, &self.lua_commands);
        }
        let Some(stripped) = input.strip_prefix('/') else {
            self.filtered.clear();
            self.current_arg_count = 0;
            return;
        };

        let parts: Vec<&str> = stripped.split_whitespace().collect();
        let cmd_word = parts.first().copied().unwrap_or(stripped);
        let trailing_space = stripped.ends_with(char::is_whitespace);

        self.current_arg_count = if trailing_space {
            parts.len()
        } else {
            parts.len().saturating_sub(1)
        };

        self.nucleo.pattern.reparse(
            0,
            cmd_word,
            CaseMatching::Ignore,
            Normalization::Smart,
            false,
        );

        self.tick();
    }

    fn tick(&mut self) {
        loop {
            let status = self.nucleo.tick(TICK_TIMEOUT_MS);
            if status.changed {
                self.refresh_matches();
            }
            if !status.running {
                break;
            }
        }
    }

    fn refresh_matches(&mut self) {
        let snapshot = self.nucleo.snapshot();
        let pattern = snapshot.pattern();
        let has_pattern = !pattern.column_pattern(0).atoms.is_empty();

        self.filtered.clear();
        let count = snapshot.matched_item_count();
        for item in snapshot.matched_items(0..count) {
            let cmd_item = &item.data;
            let col = &item.matcher_columns[0];

            if self.current_arg_count > cmd_item.max_args {
                continue;
            }

            let indices = if has_pattern {
                let mut indices_buf = vec![];
                pattern.column_pattern(0).indices(
                    col.slice(..),
                    &mut self.matcher,
                    &mut indices_buf,
                );
                indices_buf
            } else {
                Vec::new()
            };

            self.filtered.push(Match {
                display: cmd_item.name.clone(),
                command_type: cmd_item.command_type.clone(),
                indices,
            });
        }

        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
        // Argument items survive here: sync_arguments follows every sync
        // and clears them when their session ends.
    }

    pub fn close(&mut self, handle: &EventHandle) {
        self.cancel_arguments(handle);
        self.filtered.clear();
        self.argument_items.clear();
        self.argument_range = None;
        self.pending_arguments = None;
        self.accepted_argument_input = None;
        self.current_arg_count = 0;
    }

    pub fn move_up(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.filtered.len() - 1
        } else {
            self.selected - 1
        };
    }

    pub fn move_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        self.selected = if self.selected == self.filtered.len() - 1 {
            0
        } else {
            self.selected + 1
        };
    }

    fn item_has_args(&self, m: &Match) -> bool {
        match &m.command_type {
            CommandType::Builtin(cmd) => cmd.max_args > 0,
            CommandType::Custom(i) => self.custom[*i].has_args(),
            CommandType::McpPrompt(i) => !self.mcp_prompts[*i].arguments.is_empty(),
            CommandType::Lua(i) => self.lua_commands[*i].max_args > 0,
        }
    }

    fn item_description(&self, m: &Match) -> &str {
        match &m.command_type {
            CommandType::Builtin(cmd) => cmd.description,
            CommandType::Custom(i) => &self.custom[*i].description,
            CommandType::McpPrompt(i) => &self.mcp_prompts[*i].description,
            CommandType::Lua(i) => self.lua_commands[*i].description.as_ref(),
        }
    }

    /// Canonical name an alias row points at, if the row is a builtin alias.
    fn alias_target(&self, m: &Match) -> Option<&'static str> {
        match &m.command_type {
            CommandType::Builtin(cmd) if m.display != cmd.name => Some(cmd.name),
            _ => None,
        }
    }

    /// Description as rendered, with the alias note appended for alias rows.
    fn rendered_description(&self, m: &Match) -> String {
        let desc = self.item_description(m);
        match self.alias_target(m) {
            Some(target) => format!("{desc}{ALIAS_NOTE}{target})"),
            None => desc.to_string(),
        }
    }

    /// Description column spans; alias rows highlight the target name.
    fn description_spans(&self, m: &Match, desc_style: Style) -> Vec<Span<'_>> {
        let desc = self.item_description(m);
        let Some(target) = self.alias_target(m) else {
            return vec![Span::styled(desc.to_string(), desc_style)];
        };
        let t = theme::current();
        let target_style = desc_style
            .fg(t.accent.fg.unwrap_or_default())
            .add_modifier(Modifier::BOLD);
        vec![
            Span::styled(desc.to_string(), desc_style),
            Span::styled(ALIAS_NOTE, desc_style),
            Span::styled(target, target_style),
            Span::styled(")", desc_style),
        ]
    }

    pub fn confirm(&self, input: &str) -> Option<ParsedCommand> {
        let item = self.filtered.get(self.selected)?;
        // Aliases dispatch under the canonical name
        let name = match &item.command_type {
            CommandType::Builtin(cmd) => cmd.name.to_string(),
            _ => item.display.clone(),
        };
        let args = input
            .strip_prefix('/')
            .and_then(|s| s.split_once(char::is_whitespace))
            .map(|(_, a)| a.trim())
            .unwrap_or("");
        Some(ParsedCommand {
            name,
            args: args.to_string(),
        })
    }

    /// Name lookup for `maki.api.run_command`, returning the registered
    /// spelling that [`crate::app::App`] dispatches on. Case-insensitive like
    /// typing, but never fuzzy: an alias names one command on purpose, and a
    /// typo should report itself instead of running the closest neighbor.
    pub fn resolve(&self, name: &str) -> Option<String> {
        Self::items(&self.custom, &self.mcp_prompts, &self.lua_commands)
            .map(|item| item.name)
            .find(|n| n.eq_ignore_ascii_case(name))
    }

    pub fn find_custom_command(&self, display_name: &str) -> Option<&CustomCommand> {
        self.custom
            .iter()
            .find(|c| c.display_name() == display_name)
    }

    pub fn find_mcp_prompt(&self, slash_name: &str) -> Option<&McpPromptInfo> {
        let name = slash_name.strip_prefix('/')?;
        self.mcp_prompts.iter().find(|p| p.display_name == name)
    }

    pub fn find_lua_command(&self, name: &str) -> Option<&LuaCommandInfo> {
        self.lua_commands.iter().find(|c| c.name.as_ref() == name)
    }

    pub fn view(&self, frame: &mut Frame, input_area: Rect) -> Option<Rect> {
        let filtered = if self.argument_items.is_empty() {
            &self.filtered
        } else {
            return self.view_arguments(frame, input_area);
        };
        if filtered.is_empty() {
            return None;
        }

        let popup_height = (filtered.len() as u16).min(input_area.y);
        if popup_height == 0 {
            return None;
        }

        const GAP: usize = 2;
        let max_name = filtered
            .iter()
            .map(|item| item.display.len())
            .max()
            .unwrap_or(0);
        let max_desc = filtered
            .iter()
            .map(|item| self.rendered_description(item).len())
            .max()
            .unwrap_or(0);
        const PAD: usize = 1;
        let popup_width = (PAD + max_name + GAP + max_desc + PAD) as u16;

        let popup = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(popup_height),
            width: popup_width.min(input_area.width),
            height: popup_height,
        };

        let t = theme::current();
        let lines: Vec<Line> = filtered
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let name = m.display.clone();
                let selected = i == self.selected;
                let name_pad = max_name - name.len() + GAP;

                if selected {
                    let s = t.item_selected;
                    let highlighted_name = self.build_highlighted_spans(&name, &m.indices, s);
                    let mut spans = vec![Span::styled(" ".repeat(PAD), s)];
                    spans.extend(highlighted_name);
                    spans.push(Span::styled(" ".repeat(name_pad), s));
                    spans.extend(self.description_spans(m, s));
                    spans.push(Span::styled(" ".repeat(PAD), s));
                    Line::from(spans)
                } else {
                    let highlighted_name = self.build_highlighted_spans(&name, &m.indices, t.item);
                    let mut spans = vec![Span::raw(" ".repeat(PAD))];
                    spans.extend(highlighted_name);
                    spans.push(Span::raw(" ".repeat(name_pad)));
                    spans.extend(self.description_spans(m, t.item_desc));
                    spans.push(Span::raw(" ".repeat(PAD)));
                    Line::from(spans)
                }
            })
            .collect();

        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(lines).style(Style::new().bg(t.background)),
            popup,
        );

        Some(popup)
    }

    fn view_arguments(&self, frame: &mut Frame, input_area: Rect) -> Option<Rect> {
        let height = (self.argument_items.len() as u16).min(input_area.y);
        if height == 0 {
            return None;
        }
        let width = self
            .argument_items
            .iter()
            .map(|m| m.item.label.len() + m.item.description.as_deref().map_or(0, |d| d.len() + 2))
            .max()
            .unwrap_or(0) as u16
            + 2;
        let popup = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(height),
            width: width.min(input_area.width),
            height,
        };
        let t = theme::current();
        let lines = self
            .argument_items
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let selected = i == self.selected;
                let style = if selected { t.item_selected } else { t.item };
                let desc = m.item.description.as_deref().unwrap_or("");
                let label = self.build_highlighted_spans(&m.item.label, &m.indices, style);
                let mut spans = vec![Span::raw(" ")];
                spans.extend(label);
                spans.push(Span::styled(format!("  {desc}"), style));
                Line::from(spans)
            })
            .collect::<Vec<_>>();
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(lines).style(Style::new().bg(t.background)),
            popup,
        );
        Some(popup)
    }

    fn build_highlighted_spans(&self, text: &str, indices: &[u32], base: Style) -> Vec<Span<'_>> {
        if indices.is_empty() {
            return vec![Span::styled(text.to_string(), base)];
        }

        let t = theme::current();
        let highlight = base
            .fg(t.accent.fg.unwrap_or_default())
            .add_modifier(Modifier::BOLD);

        let mut spans = Vec::new();
        let mut in_match = false;
        let mut run = String::new();

        for (i, ch) in text.chars().enumerate() {
            let is_match = indices.binary_search(&(i as u32)).is_ok();
            if is_match != in_match && !run.is_empty() {
                spans.push(Span::styled(
                    mem::take(&mut run),
                    if in_match { highlight } else { base },
                ));
            }
            in_match = is_match;
            run.push(ch);
        }

        if !run.is_empty() {
            spans.push(Span::styled(run, if in_match { highlight } else { base }));
        }

        spans
    }
}

fn command_args(input: &str) -> &str {
    input
        .strip_prefix('/')
        .and_then(|input| input.find(char::is_whitespace).map(|i| &input[i + 1..]))
        .unwrap_or("")
}

fn argument_at_cursor(input: &str, cursor: usize) -> Option<(usize, usize, String, usize)> {
    let slash = input.strip_prefix('/')?;
    let command_end = slash.find(char::is_whitespace)? + 1;
    if cursor < command_end || !input.is_char_boundary(cursor) {
        return None;
    }
    let start = input[..cursor]
        .rfind(char::is_whitespace)
        .map_or(command_end, |i| i + 1);
    let end = input[cursor..]
        .find(char::is_whitespace)
        .map_or(input.len(), |i| cursor + i);
    let arg = input[start..end].to_string();
    let index = input[command_end..start].split_whitespace().count();
    Some((start, end, arg, index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use maki_agent::{McpPromptArg, McpSnapshot};
    use test_case::test_case;

    fn empty_snapshot() -> McpSnapshotReader {
        McpSnapshotReader::empty()
    }

    fn synced(input: &str) -> CommandPalette {
        let mut p = CommandPalette::new(Arc::from([]), empty_snapshot(), LuaCommandReader::empty());
        p.sync(input);
        p
    }

    fn synced_with_custom(input: &str, custom: Arc<[CustomCommand]>) -> CommandPalette {
        let mut p = CommandPalette::new(custom, empty_snapshot(), LuaCommandReader::empty());
        p.sync(input);
        p
    }

    fn sample_custom() -> Arc<[CustomCommand]> {
        Arc::from([
            CustomCommand {
                name: "review".into(),
                description: "Code review".into(),
                content: "Review $ARGUMENTS".into(),
                scope: maki_agent::command::CommandScope::Project,
                accepts_args: true,
            },
            CustomCommand {
                name: "fix".into(),
                description: "Quick fix".into(),
                content: "Fix the code".into(),
                scope: maki_agent::command::CommandScope::User,
                accepts_args: false,
            },
        ])
    }

    #[test]
    fn slash_shows_builtins_plus_extras() {
        let builtin_count = synced("/").filtered.len();
        assert!(builtin_count > 0);

        let with_custom = synced_with_custom("/", sample_custom());
        assert_eq!(with_custom.filtered.len(), builtin_count + 2);

        let with_prompts = synced_with_prompts("/");
        assert_eq!(with_prompts.filtered.len(), builtin_count + 2);
    }

    #[test]
    fn lua_command_overrides_builtin_of_same_name() {
        let lua = LuaCommandReader::from_commands(vec![LuaCommandInfo {
            name: Arc::from("/thinking"),
            description: Arc::from("thinking selector"),
            plugin: Arc::from("thinking"),
            max_args: 1,
            has_argument_completion: false,
        }]);
        let mut p = CommandPalette::new(Arc::from([]), empty_snapshot(), lua);
        p.sync("/thinking");
        let count = p
            .filtered
            .iter()
            .filter(|m| m.display == "/thinking")
            .count();
        assert_eq!(
            count, 1,
            "lua command should shadow the builtin of the same name"
        );
    }

    #[test]
    fn close_deactivates() {
        let mut p = synced("/");
        p.close(&EventHandle::disconnected_for_test());
        assert!(!p.is_active());
    }

    #[test_case("/mp", true ; "compact_substring")]
    #[test_case("/ew", true ; "lowercase_substring")]
    #[test_case("/EW", true ; "uppercase_substring")]
    #[test_case("/zzz", false ; "no_match")]
    fn filter_by_substring(input: &str, expect_active: bool) {
        let p = synced(input);
        assert_eq!(p.is_active(), expect_active);
    }

    #[test]
    fn filter_custom_by_substring() {
        let p = synced_with_custom("/review", sample_custom());
        assert!(p.is_active());
        assert_eq!(p.filtered.len(), 1);
        assert!(matches!(p.filtered[0].command_type, CommandType::Custom(0)));
    }

    #[test]
    fn navigation_wraps() {
        let mut p = synced("/");
        p.move_up();
        assert_eq!(p.selected, p.filtered.len() - 1);
        p.move_down();
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn confirm_when_inactive_returns_none() {
        let p = CommandPalette::new(Arc::from([]), empty_snapshot(), LuaCommandReader::empty());
        assert!(p.confirm("").is_none());
    }

    #[test]
    fn sync_clamps_selected() {
        let mut p = synced("/");
        p.selected = 100;
        p.sync("/");
        assert_eq!(p.selected, p.filtered.len() - 1);
    }

    #[test]
    fn sync_filters_on_first_word_only() {
        let p = synced("/cd ~/foo");
        assert!(p.is_active());
        assert_eq!(p.filtered.len(), 1);
        assert_eq!(p.filtered[0].display, "/cd");
    }

    #[test_case("/compact ", false ; "zero_arg_cmd_with_space")]
    #[test_case("/tasks ", false   ; "zero_arg_tasks_with_space")]
    #[test_case("/cd ", true        ; "one_arg_cmd_with_space")]
    #[test_case("/cd ~/foo", true   ; "one_arg_cmd_mid_arg")]
    #[test_case("/cd  ~/foo", true  ; "one_arg_cmd_double_space")]
    #[test_case("/cd ~/foo ", false ; "one_arg_cmd_second_space")]
    #[test_case("/btw hello world", true ; "btw_stays_active_with_many_args")]
    fn sync_respects_nargs(input: &str, expect_active: bool) {
        let p = synced(input);
        assert_eq!(p.is_active(), expect_active);
    }

    #[test]
    fn custom_command_with_args_stays_active() {
        let p = synced_with_custom("/project:review some args", sample_custom());
        assert!(p.is_active());
    }

    #[test]
    fn custom_command_without_args_hides_on_space() {
        let p = synced_with_custom("/user:fix ", sample_custom());
        assert!(!p.is_active());
    }

    #[test_case("/cd", "/cd", ""              ; "no_args")]
    #[test_case("/cd ~/foo", "/cd", "~/foo"   ; "with_args")]
    #[test_case("/CD ~/foo", "/cd", "~/foo"   ; "case_insensitive")]
    #[test_case("/compact", "/compact", ""    ; "other_command")]
    #[test_case("/cmp", "/compact", ""    ; "fuzzy-match-1")]
    #[test_case("/pct", "/compact", ""    ; "fuzzy-match-2")]
    #[test_case("/btw hello world", "/btw", "hello world" ; "btw_multi_word")]
    fn confirm_parses_args(input: &str, expected_name: &str, expected_args: &str) {
        let mut p = CommandPalette::new(Arc::from([]), empty_snapshot(), LuaCommandReader::empty());
        p.sync(input);
        let cmd = p.confirm(input).unwrap();
        assert_eq!(cmd.name, expected_name);
        assert_eq!(cmd.args, expected_args);
    }

    #[test]
    fn builtin_alias_shows_in_palette() {
        let p = synced("/clear");
        assert!(p.is_active());
        assert_eq!(p.filtered.len(), 1);
        assert_eq!(p.filtered[0].display, "/clear");
    }

    #[test]
    fn alias_note_appended_for_alias_rows_only() {
        let cmd = BUILTIN_COMMANDS.iter().find(|c| c.name == "/new").unwrap();

        let p = synced("/clear");
        let m = &p.filtered[0];
        assert_eq!(p.alias_target(m), Some("/new"));
        assert_eq!(
            p.rendered_description(m),
            format!("{}{ALIAS_NOTE}/new)", cmd.description)
        );

        let p = synced("/new");
        let m = p.filtered.iter().find(|m| m.display == "/new").unwrap();
        assert_eq!(p.alias_target(m), None);
        assert_eq!(p.rendered_description(m), cmd.description);
    }

    #[test]
    fn confirm_alias_dispatches_canonical_name() {
        let p = synced("/clear");
        let cmd = p.confirm("/clear").unwrap();
        assert_eq!(cmd.name, "/new");
        assert_eq!(cmd.args, "");
    }

    #[test]
    fn builtin_and_alias_listed_under_slash() {
        let p = synced("/");
        assert!(p.filtered.iter().any(|m| m.display == "/new"));
        assert!(p.filtered.iter().any(|m| m.display == "/clear"));
    }

    #[test]
    fn confirm_custom_command() {
        let custom = sample_custom();
        let mut p = CommandPalette::new(custom, empty_snapshot(), LuaCommandReader::empty());
        p.sync("/project:review");
        assert!(p.is_active());
        let cmd = p.confirm("/project:review some-file.rs").unwrap();
        assert_eq!(cmd.name, "/project:review");
        assert_eq!(cmd.args, "some-file.rs");
    }

    #[test]
    fn find_custom_command_lookup() {
        let custom = sample_custom();
        let p = CommandPalette::new(custom, empty_snapshot(), LuaCommandReader::empty());
        let found = p.find_custom_command("/project:review");
        assert!(found.is_some());
        assert_eq!(found.unwrap().content, "Review $ARGUMENTS");
        assert!(p.find_custom_command("/nonexistent").is_none());
    }

    fn sample_prompts() -> McpSnapshotReader {
        McpSnapshotReader::from_snapshot(McpSnapshot {
            infos: vec![],
            prompts: vec![
                McpPromptInfo {
                    display_name: "myserver:code-review".into(),
                    qualified_name: "myserver/code-review".into(),
                    description: "Review code changes".into(),
                    arguments: vec![McpPromptArg {
                        name: "diff".into(),
                        description: "The diff".into(),
                        required: true,
                    }],
                },
                McpPromptInfo {
                    display_name: "myserver:summarize".into(),
                    qualified_name: "myserver/summarize".into(),
                    description: "Summarize text".into(),
                    arguments: vec![],
                },
            ],
            pids: vec![],
            generation: 0,
        })
    }

    fn synced_with_prompts(input: &str) -> CommandPalette {
        let mut p = CommandPalette::new(Arc::from([]), sample_prompts(), LuaCommandReader::empty());
        p.sync(input);
        p
    }

    #[test]
    fn filter_mcp_prompt_by_substring() {
        let p = synced_with_prompts("/code");
        assert!(p.is_active());
        assert_eq!(p.filtered.len(), 1);
        assert!(matches!(
            p.filtered[0].command_type,
            CommandType::McpPrompt(0)
        ));
    }

    #[test]
    fn mcp_prompt_with_args_stays_active() {
        let p = synced_with_prompts("/myserver:code-review some diff");
        assert!(p.is_active());
    }

    #[test]
    fn mcp_prompt_without_args_hides_on_space() {
        let p = synced_with_prompts("/myserver:summarize ");
        assert!(
            !p.filtered
                .iter()
                .any(|f| matches!(f.command_type, CommandType::McpPrompt(1)))
        );
    }

    #[test]
    fn find_mcp_prompt_lookup() {
        let p = synced_with_prompts("/");
        let found = p.find_mcp_prompt("/myserver:code-review");
        assert!(found.is_some());
        assert_eq!(found.unwrap().qualified_name, "myserver/code-review");
        assert!(p.find_mcp_prompt("/nonexistent").is_none());
    }

    #[test]
    fn confirm_mcp_prompt_parses_args() {
        let input = "/myserver:code-review my-diff-content";
        let mut p = synced_with_prompts(input);
        p.selected = p
            .filtered
            .iter()
            .position(|f| matches!(f.command_type, CommandType::McpPrompt(0)))
            .unwrap();
        let cmd = p.confirm(input).unwrap();
        assert_eq!(cmd.name, "/myserver:code-review");
        assert_eq!(cmd.args, "my-diff-content");
    }

    #[test]
    fn mcp_update_clears_old_prompts() {
        let reader = sample_prompts();
        let mut p = CommandPalette::new(Arc::from([]), reader, LuaCommandReader::empty());

        p.sync("/");
        let initial_count = p
            .filtered
            .iter()
            .filter(|f| matches!(f.command_type, CommandType::McpPrompt(_)))
            .count();
        assert_eq!(initial_count, 2, "Should have 2 MCP prompts initially");

        let updated_reader = McpSnapshotReader::from_snapshot(McpSnapshot {
            infos: vec![],
            prompts: vec![McpPromptInfo {
                display_name: "myserver:new-prompt".into(),
                qualified_name: "myserver/new-prompt".into(),
                description: "A new prompt".into(),
                arguments: vec![],
            }],
            pids: vec![],
            generation: 1,
        });

        p.mcp_reader = updated_reader;
        p.sync("/");

        let updated_count = p
            .filtered
            .iter()
            .filter(|f| matches!(f.command_type, CommandType::McpPrompt(_)))
            .count();
        assert_eq!(
            updated_count, 1,
            "Should have only 1 MCP prompt after update"
        );

        assert!(!p.filtered.is_empty(), "Should have filtered results");
        let prompt = &p
            .filtered
            .iter()
            .find(|f| matches!(f.command_type, CommandType::McpPrompt(_)))
            .expect("Should have at least one MCP prompt");
        match &prompt.command_type {
            CommandType::McpPrompt(i) => {
                assert_eq!(p.mcp_prompts[*i].display_name, "myserver:new-prompt");
            }
            _ => panic!("Should have MCP prompt"),
        }
    }

    #[test_case("/cmp", "/compact" ; "compact_fuzzy")]
    #[test_case("/new", "/new" ; "new_exact")]
    #[test_case("/tsk", "/tasks" ; "tasks_fuzzy")]
    fn nucleo_highlights_matching_indices(input: &str, expected_cmd: &str) {
        let p = synced(input);
        assert!(p.is_active(), "Input '{}' should activate palette", input);
        // Find the expected match
        let matched = p
            .filtered
            .iter()
            .find(|m| m.display == expected_cmd)
            .unwrap_or_else(|| panic!("Should find {} for input {}", expected_cmd, input));
        // Should have some highlight indices
        assert!(
            !matched.indices.is_empty(),
            "Match should have highlight indices"
        );
    }

    fn sample_lua_commands() -> LuaCommandReader {
        LuaCommandReader::from_commands(vec![
            LuaCommandInfo {
                name: Arc::from("/memory"),
                description: Arc::from("View memory files"),
                plugin: Arc::from("memory"),
                max_args: 0,
                has_argument_completion: false,
            },
            LuaCommandInfo {
                name: Arc::from("/deploy"),
                description: Arc::from("Deploy the project"),
                plugin: Arc::from("deploy_plugin"),
                max_args: 0,
                has_argument_completion: false,
            },
        ])
    }

    fn synced_with_nargs(input: &str, max_args: usize) -> CommandPalette {
        let reader = LuaCommandReader::from_commands(vec![LuaCommandInfo {
            name: Arc::from("/rename"),
            description: Arc::from("Rename the current session"),
            plugin: Arc::from("sessions"),
            max_args,
            has_argument_completion: false,
        }]);
        let mut p = CommandPalette::new(Arc::from([]), empty_snapshot(), reader);
        p.sync(input);
        p
    }

    #[test_case("/rename", usize::MAX, true           ; "nargs_plus_no_args")]
    #[test_case("/rename ", usize::MAX, true          ; "nargs_plus_trailing_space")]
    #[test_case("/rename my title", usize::MAX, true  ; "nargs_plus_multi_word")]
    #[test_case("/rename title", 1, true              ; "nargs_one_single_word")]
    #[test_case("/rename my title", 1, false          ; "nargs_one_too_many")]
    #[test_case("/rename", 0, true                    ; "nargs_zero_no_args")]
    #[test_case("/rename title", 0, false             ; "nargs_zero_with_arg")]
    fn lua_command_respects_nargs(input: &str, max_args: usize, expect_active: bool) {
        assert_eq!(
            synced_with_nargs(input, max_args).is_active(),
            expect_active
        );
    }

    #[test]
    fn argument_at_cursor_selects_middle_argument() {
        assert_eq!(
            argument_at_cursor("/rename first second third", 16),
            Some((14, 20, "second".to_string(), 1))
        );
    }

    #[test]
    fn argument_at_cursor_handles_multibyte_text() {
        let input = "/rename αβ second";
        let cursor = input.find('β').unwrap();
        assert_eq!(
            argument_at_cursor(input, cursor),
            Some((8, 12, "αβ".to_string(), 0))
        );
    }

    #[test]
    fn confirm_lua_command_keeps_multi_word_args() {
        let input = "/rename my new title";
        let cmd = synced_with_nargs(input, usize::MAX).confirm(input).unwrap();
        assert_eq!(cmd.name, "/rename");
        assert_eq!(cmd.args, "my new title");
    }

    fn synced_with_lua(input: &str) -> CommandPalette {
        let mut p = CommandPalette::new(Arc::from([]), empty_snapshot(), sample_lua_commands());
        p.sync(input);
        p
    }

    #[test]
    fn lua_commands_appear_in_unfiltered_list() {
        let p = synced_with_lua("/");
        let lua_count = p
            .filtered
            .iter()
            .filter(|f| matches!(f.command_type, CommandType::Lua(_)))
            .count();
        assert_eq!(lua_count, 2);
    }

    #[test]
    fn lua_command_filtered_by_substring() {
        let p = synced_with_lua("/mem");
        assert!(p.is_active());
        let found = p
            .filtered
            .iter()
            .any(|f| matches!(f.command_type, CommandType::Lua(_)) && f.display == "/memory");
        assert!(found);
    }

    #[test]
    fn find_lua_command_returns_matching_entry() {
        let p = synced_with_lua("/");
        let found = p.find_lua_command("/memory");
        assert!(found.is_some());
        assert_eq!(found.unwrap().plugin.as_ref(), "memory");
        assert!(p.find_lua_command("/nonexistent").is_none());
    }

    #[test]
    fn confirm_lua_command_parses_args() {
        let mut p = CommandPalette::new(Arc::from([]), empty_snapshot(), sample_lua_commands());
        p.sync("/memory");
        let cmd = p.confirm("/memory some-arg").unwrap();
        assert_eq!(cmd.name, "/memory");
        assert_eq!(cmd.args, "some-arg");
    }

    #[test_case("/deploy", "" ; "no_arguments")]
    #[test_case("/deploy ", "" ; "empty_argument")]
    #[test_case("/deploy  staging west ", " staging west " ; "raw_whitespace")]
    fn command_args_excludes_slash_command(input: &str, expected: &str) {
        assert_eq!(command_args(input), expected);
    }

    #[test]
    fn navigation_requires_argument_completion_resync() {
        let lua = LuaCommandReader::from_commands(vec![
            LuaCommandInfo {
                name: Arc::from("/alpha"),
                description: Arc::from("alpha"),
                plugin: Arc::from("plugin"),
                max_args: 1,
                has_argument_completion: true,
            },
            LuaCommandInfo {
                name: Arc::from("/beta"),
                description: Arc::from("beta"),
                plugin: Arc::from("plugin"),
                max_args: 1,
                has_argument_completion: true,
            },
        ]);
        let mut palette = CommandPalette::new(Arc::from([]), empty_snapshot(), lua);
        palette.sync("/ ");
        let handle = EventHandle::disconnected_for_test();

        assert!(matches!(
            palette.handle_key(KeyEvent::from(KeyCode::Down), "/ ", &handle),
            CommandAction::SelectionChanged
        ));
    }

    #[test]
    fn delayed_argument_response_is_rejected_after_navigation_resync() {
        let lua = LuaCommandReader::from_commands(vec![
            LuaCommandInfo {
                name: Arc::from("/alpha"),
                description: Arc::from("alpha"),
                plugin: Arc::from("plugin"),
                max_args: 1,
                has_argument_completion: true,
            },
            LuaCommandInfo {
                name: Arc::from("/beta"),
                description: Arc::from("beta"),
                plugin: Arc::from("plugin"),
                max_args: 1,
                has_argument_completion: true,
            },
        ]);
        let mut palette = CommandPalette::new(Arc::from([]), empty_snapshot(), lua);
        palette.sync("/ ");
        let handle = EventHandle::disconnected_for_test();
        let (tx, rx) = flume::bounded(1);
        palette.pending_arguments = Some(PendingArguments {
            rx,
            generation: palette.argument_generation,
            query: String::new(),
            range: (2, 2),
        });
        assert!(matches!(
            palette.handle_key(KeyEvent::from(KeyCode::Down), "/ ", &handle),
            CommandAction::SelectionChanged
        ));
        tx.send(vec![CommandArgumentItem {
            label: "stale".into(),
            insertion: "stale".into(),
            description: None,
        }])
        .unwrap();
        palette.sync_arguments("/ ", 2, &handle, "build");

        assert_eq!(palette.poll_arguments(&handle), Dirty::NO);
        assert!(palette.argument_items.is_empty());
    }

    #[test]
    fn enter_accepts_argument_then_executes_completed_command() {
        let mut palette = synced_with_nargs("/rename draft", usize::MAX);
        palette.set_argument_completion(
            (8, 13),
            CommandArgumentItem {
                label: "final".into(),
                insertion: "final".into(),
                description: None,
            },
        );

        let handle = EventHandle::disconnected_for_test();
        assert!(matches!(
            palette.handle_key(KeyEvent::from(KeyCode::Enter), "/rename draft", &handle),
            CommandAction::AcceptArgument { text, cursor }
                if text == "/rename final" && cursor == 13
        ));
        assert!(!palette.is_active());
        assert!(matches!(
            palette.handle_key(KeyEvent::from(KeyCode::Enter), "/rename final", &handle),
            CommandAction::Execute(ParsedCommand { name, args })
                if name == "/rename" && args == "final"
        ));
    }

    fn settled_rename_palette() -> (
        CommandPalette,
        EventHandle,
        maki_lua::test_support::RequestProbe,
        CommandArgumentItem,
    ) {
        let lua = LuaCommandReader::from_commands(vec![LuaCommandInfo {
            name: Arc::from("/rename"),
            description: Arc::from("Rename the current session"),
            plugin: Arc::from("sessions"),
            max_args: usize::MAX,
            has_argument_completion: true,
        }]);
        let mut palette = CommandPalette::new(Arc::from([]), empty_snapshot(), lua);
        let (handle, probe) = maki_lua::test_support::probed_event_handle();
        let final_item = CommandArgumentItem {
            label: "final".into(),
            insertion: "final".into(),
            description: None,
        };
        palette.sync("/rename final");
        palette.sync_arguments("/rename final", 13, &handle, "build");
        probe
            .try_finish_command_arguments(vec![final_item.clone()])
            .unwrap();
        let _ = palette.poll_arguments(&handle);
        (palette, handle, probe, final_item)
    }

    #[test]
    fn stale_argument_items_stay_until_new_result_lands() {
        let (mut palette, handle, probe, _) = settled_rename_palette();

        // Keystroke: the app re-syncs the command list and re-queries.
        palette.sync("/rename final");
        palette.sync_arguments("/rename final", 13, &handle, "build");
        // The previous items stay on screen while the new query is in flight.
        assert!(matches!(
            palette.handle_key(KeyEvent::from(KeyCode::Down), "/rename final", &handle),
            CommandAction::Consumed
        ));

        let finale = CommandArgumentItem {
            label: "finale".into(),
            insertion: "finale".into(),
            description: None,
        };
        probe.try_finish_command_arguments(vec![finale]).unwrap();
        let _ = palette.poll_arguments(&handle);
        // The new result replaces the stale item: accepting now inserts it.
        assert!(matches!(
            palette.handle_key(KeyEvent::from(KeyCode::Enter), "/rename final", &handle),
            CommandAction::AcceptArgument { text, .. } if text == "/rename finale"
        ));
    }

    #[test]
    fn enter_while_argument_query_in_flight_executes_input() {
        let (mut palette, handle, _, _) = settled_rename_palette();
        palette.sync("/rename final");
        palette.sync_arguments("/rename final", 13, &handle, "build");

        assert!(matches!(
            palette.handle_key(KeyEvent::from(KeyCode::Enter), "/rename final", &handle),
            CommandAction::Execute(ParsedCommand { name, args })
                if name == "/rename" && args == "final"
        ));
        assert!(!palette.is_active());
    }

    #[test]
    fn tab_completes_selected_argument_in_place() {
        let mut palette = synced_with_nargs("/rename one draft three", usize::MAX);
        palette.argument_items.push(ArgumentMatch {
            item: CommandArgumentItem {
                label: "final".into(),
                insertion: "final".into(),
                description: None,
            },
            indices: Vec::new(),
        });
        palette.argument_range = Some((12, 17));

        let handle = EventHandle::disconnected_for_test();
        assert!(matches!(
            palette.handle_key(KeyEvent::from(KeyCode::Tab), "/rename one draft three", &handle),
            CommandAction::Complete { text, cursor }
                if text == "/rename one final three" && cursor == 17
        ));
    }

    #[test]
    fn zero_match_keeps_lifecycle_until_recovery_and_close_cancels_once() {
        let lua = LuaCommandReader::from_commands(vec![LuaCommandInfo {
            name: Arc::from("/deploy"),
            description: Arc::from("Deploy"),
            plugin: Arc::from("deploy"),
            max_args: 1,
            has_argument_completion: true,
        }]);
        let mut palette = CommandPalette::new(Arc::from([]), empty_snapshot(), lua);
        let (handle, probe) = maki_lua::test_support::probed_event_handle();
        let items = vec![CommandArgumentItem {
            label: "alpha".into(),
            insertion: "alpha".into(),
            description: None,
        }];

        palette.sync("/deploy a");
        palette.sync_arguments("/deploy a", 9, &handle, "build");
        probe.try_finish_command_arguments(items.clone()).unwrap();
        let _ = palette.poll_arguments(&handle);
        assert_eq!(
            probe.try_finish_command_argument_lifecycle(),
            Some(("highlight", Some("alpha".into()), true))
        );

        palette.sync_arguments("/deploy z", 9, &handle, "build");
        probe.try_finish_command_arguments(items.clone()).unwrap();
        let _ = palette.poll_arguments(&handle);
        assert!(palette.is_active());
        assert!(probe.try_finish_command_argument_lifecycle().is_none());

        palette.sync_arguments("/deploy a", 9, &handle, "build");
        probe.try_finish_command_arguments(items).unwrap();
        let _ = palette.poll_arguments(&handle);
        assert_eq!(
            probe.try_finish_command_argument_lifecycle(),
            Some(("highlight", Some("alpha".into()), true))
        );
        palette.close(&handle);
        assert_eq!(
            probe.try_finish_command_argument_lifecycle(),
            Some(("cancel", None, true))
        );
        palette.close(&handle);
        assert!(probe.try_finish_command_argument_lifecycle().is_none());
    }

    #[test]
    fn lua_commands_update_on_generation_change() {
        let (writer, reader) = maki_lua::test_support::lua_command_writer_pair();
        writer.publish(vec![LuaCommandInfo {
            name: Arc::from("/old"),
            description: Arc::from("old command"),
            plugin: Arc::from("p"),
            max_args: 0,
            has_argument_completion: false,
        }]);
        let mut p = CommandPalette::new(Arc::from([]), empty_snapshot(), reader);
        p.sync("/");
        let initial_lua = p
            .filtered
            .iter()
            .filter(|f| matches!(f.command_type, CommandType::Lua(_)))
            .count();
        assert_eq!(initial_lua, 1);

        writer.publish(vec![
            LuaCommandInfo {
                name: Arc::from("/new1"),
                description: Arc::from("new"),
                plugin: Arc::from("p"),
                max_args: 0,
                has_argument_completion: false,
            },
            LuaCommandInfo {
                name: Arc::from("/new2"),
                description: Arc::from("new2"),
                plugin: Arc::from("p"),
                max_args: 0,
                has_argument_completion: false,
            },
        ]);
        p.sync("/");
        let updated_lua = p
            .filtered
            .iter()
            .filter(|f| matches!(f.command_type, CommandType::Lua(_)))
            .count();
        assert_eq!(updated_lua, 2);
        assert!(p.find_lua_command("/old").is_none());
        assert!(p.find_lua_command("/new1").is_some());
    }
}
