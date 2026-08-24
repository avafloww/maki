use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use maki_commands::{
    CancellationToken, CommandCompletion, CommandFuture, CompletionContext, CompletionError,
    CompletionItem, CompletionLifecycleEvent, CompletionSessionId, InvocationTargetId,
};

use crate::theme::{ThemesProvider, apply_theme};

pub(crate) struct ModelArgSource {
    models: Arc<ArcSwapOption<Vec<String>>>,
}

impl ModelArgSource {
    pub(crate) fn new(models: Arc<ArcSwapOption<Vec<String>>>) -> Self {
        Self { models }
    }
}

impl CommandCompletion for ModelArgSource {
    fn complete(
        &self,
        _context: CompletionContext,
        _cancellation: CancellationToken,
    ) -> CommandFuture<Result<Vec<CompletionItem>, CompletionError>> {
        let items = self.models.load_full().map_or_else(Vec::new, |specs| {
            specs
                .iter()
                .map(|spec| CompletionItem {
                    label: Arc::from(spec.as_str()),
                    insertion: Arc::from(spec.as_str()),
                    description: None,
                })
                .collect()
        });
        Box::pin(async move { Ok(items) })
    }
}

pub(crate) struct ThemeArgSource {
    provider: Arc<dyn ThemesProvider>,
    previews: Mutex<Vec<ThemePreview>>,
}

struct ThemePreview {
    session: CompletionSessionId,
    target: InvocationTargetId,
    original: String,
    selected: String,
    accepted: bool,
}

impl ThemeArgSource {
    pub(crate) fn new(provider: Arc<dyn ThemesProvider>) -> Self {
        Self {
            provider,
            previews: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn finish(&self, target: InvocationTargetId, commit: bool) {
        let mut previews = self
            .previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(index) = previews
            .iter()
            .rposition(|preview| preview.target == target && preview.accepted)
        else {
            return;
        };
        remove_preview(self.provider.as_ref(), &mut previews, index, commit);
    }
}

fn remove_preview(
    provider: &dyn ThemesProvider,
    previews: &mut Vec<ThemePreview>,
    index: usize,
    commit: bool,
) {
    let was_owner = index + 1 == previews.len();
    let removed = previews.remove(index);
    let restored = if commit {
        removed.selected
    } else {
        removed.original
    };
    if let Some(next) = previews.get_mut(index) {
        next.original = restored.clone();
    }
    if was_owner {
        let theme = previews
            .last()
            .map_or(restored.as_str(), |preview| preview.selected.as_str());
        apply_theme(provider, theme);
    }
}

impl CommandCompletion for ThemeArgSource {
    fn complete(
        &self,
        _context: CompletionContext,
        _cancellation: CancellationToken,
    ) -> CommandFuture<Result<Vec<CompletionItem>, CompletionError>> {
        let items = self
            .provider
            .names()
            .into_iter()
            .map(|name| CompletionItem {
                label: Arc::from(name.as_str()),
                insertion: Arc::from(name),
                description: None,
            })
            .collect();
        Box::pin(async move { Ok(items) })
    }

    fn lifecycle(
        &self,
        context: &CompletionContext,
        event: &CompletionLifecycleEvent,
        _cancellation: &CancellationToken,
    ) -> Result<(), CompletionError> {
        let mut previews = self
            .previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match event {
            CompletionLifecycleEvent::Highlight(item) => {
                let index = previews
                    .iter()
                    .position(|preview| preview.session == context.session_id)
                    .unwrap_or_else(|| {
                        previews.push(ThemePreview {
                            session: context.session_id,
                            target: context.target_id,
                            original: self.provider.current_theme_name(),
                            selected: item.insertion.to_string(),
                            accepted: false,
                        });
                        previews.len() - 1
                    });
                previews[index].selected = item.insertion.to_string();
                if index + 1 == previews.len() {
                    apply_theme(self.provider.as_ref(), &item.insertion);
                }
            }
            CompletionLifecycleEvent::Accept(item) => {
                if let Some(preview) = previews
                    .iter_mut()
                    .find(|preview| preview.session == context.session_id)
                {
                    preview.selected = item.insertion.to_string();
                    preview.accepted = true;
                }
            }
            CompletionLifecycleEvent::Cancel => {
                if let Some(index) = previews
                    .iter()
                    .position(|preview| preview.session == context.session_id)
                {
                    remove_preview(self.provider.as_ref(), &mut previews, index, false);
                }
            }
        }
        Ok(())
    }
}
