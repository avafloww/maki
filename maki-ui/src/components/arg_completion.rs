use std::sync::Arc;

use arc_swap::ArcSwapOption;
use maki_agent::CancelToken;
use maki_lua::{
    CommandArgumentContext, CommandArgumentItem, CommandArgumentLifecycle, EventHandle,
};

use crate::theme::{ThemesProvider, apply_theme};

/// Which live source serves the palette's current argument session.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SourceKind {
    Model,
    Theme,
    Lua,
}

/// Feeds the command palette's argument list. Synchronous sources fill the
/// receiver they return so the palette's shared `poll_arguments` flow works
/// for every source.
pub(crate) trait ArgumentSource: Send {
    /// None when there is no live item list (e.g. model discovery not done yet).
    fn collect(
        &mut self,
        ctx: &CommandArgumentContext,
        token: CancelToken,
    ) -> Option<flume::Receiver<Vec<CommandArgumentItem>>>;

    fn lifecycle(
        &mut self,
        ctx: &CommandArgumentContext,
        event: CommandArgumentLifecycle,
        item: Option<&CommandArgumentItem>,
        token: CancelToken,
    );
}

/// Hand synchronous items over on a one-slot channel.
fn sync_items(
    items: Vec<CommandArgumentItem>,
) -> Option<flume::Receiver<Vec<CommandArgumentItem>>> {
    let (tx, rx) = flume::bounded(1);
    let _ = tx.send(items);
    Some(rx)
}

pub(crate) struct ModelArgSource {
    models: Arc<ArcSwapOption<Vec<String>>>,
}

impl ModelArgSource {
    pub(crate) fn new(models: Arc<ArcSwapOption<Vec<String>>>) -> Self {
        Self { models }
    }
}

impl ArgumentSource for ModelArgSource {
    fn collect(
        &mut self,
        _ctx: &CommandArgumentContext,
        _token: CancelToken,
    ) -> Option<flume::Receiver<Vec<CommandArgumentItem>>> {
        let specs = self.models.load_full()?;
        sync_items(
            specs
                .iter()
                .map(|spec| CommandArgumentItem {
                    label: spec.clone(),
                    insertion: spec.clone(),
                    description: None,
                })
                .collect(),
        )
    }

    fn lifecycle(
        &mut self,
        _ctx: &CommandArgumentContext,
        _event: CommandArgumentLifecycle,
        _item: Option<&CommandArgumentItem>,
        _token: CancelToken,
    ) {
    }
}

pub(crate) struct ThemeArgSource {
    provider: Arc<dyn ThemesProvider>,
    /// Original name at the first preview, restored on Cancel.
    previewed: Option<String>,
}

impl ThemeArgSource {
    pub(crate) fn new(provider: Arc<dyn ThemesProvider>) -> Self {
        Self {
            provider,
            previewed: None,
        }
    }
}

impl ArgumentSource for ThemeArgSource {
    fn collect(
        &mut self,
        _ctx: &CommandArgumentContext,
        _token: CancelToken,
    ) -> Option<flume::Receiver<Vec<CommandArgumentItem>>> {
        sync_items(
            self.provider
                .names()
                .into_iter()
                .map(|name| CommandArgumentItem {
                    label: name.clone(),
                    insertion: name,
                    description: None,
                })
                .collect(),
        )
    }

    fn lifecycle(
        &mut self,
        _ctx: &CommandArgumentContext,
        event: CommandArgumentLifecycle,
        item: Option<&CommandArgumentItem>,
        _token: CancelToken,
    ) {
        match event {
            CommandArgumentLifecycle::Highlight => {
                if self.previewed.is_none() {
                    self.previewed = Some(self.provider.current_theme_name());
                }
                if let Some(item) = item {
                    apply_theme(self.provider.as_ref(), &item.insertion);
                }
            }
            // The baseline is consumed by the submitted command, mirroring
            // ThemePicker::Select.
            CommandArgumentLifecycle::Accept => {
                self.previewed = None;
            }
            CommandArgumentLifecycle::Cancel => {
                if let Some(name) = self.previewed.take() {
                    apply_theme(self.provider.as_ref(), &name);
                }
            }
        }
    }
}

pub(crate) struct LuaArgumentSource {
    handle: EventHandle,
}

impl LuaArgumentSource {
    pub(crate) fn new(handle: EventHandle) -> Self {
        Self { handle }
    }
}

impl ArgumentSource for LuaArgumentSource {
    fn collect(
        &mut self,
        ctx: &CommandArgumentContext,
        token: CancelToken,
    ) -> Option<flume::Receiver<Vec<CommandArgumentItem>>> {
        self.handle
            .collect_command_argument_items(ctx.clone(), token)
    }

    fn lifecycle(
        &mut self,
        ctx: &CommandArgumentContext,
        event: CommandArgumentLifecycle,
        item: Option<&CommandArgumentItem>,
        token: CancelToken,
    ) {
        self.handle
            .command_argument_lifecycle(ctx.clone(), event, item.cloned(), token)
    }
}
