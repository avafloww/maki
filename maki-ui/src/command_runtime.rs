use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use maki_agent::{McpPromptInfo, command::CustomCommand};
use maki_commands::{
    ArgumentArity, BUILTIN_COMMANDS, CommandBehavior, CommandClassification, CommandCompletion,
    CommandDocs, CommandError, CommandFuture, CommandInvocation, CommandRegistry, CommandSpec,
    CompletionContext, CompletionError, CompletionItem, CompletionLifecycleEvent,
    CompletionSessionId, InvocationLifecycle, InvocationTargetId, Producer, ProducerPrecedence,
    Registration,
};
use maki_lua::{
    CommandArgumentContext, CommandArgumentItem, CommandArgumentLifecycle, EventHandle,
    LuaCommandInfo,
};

use crate::components::arg_completion::{ModelArgSource, ThemeArgSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinRoute {
    Tasks,
    Compact,
    New,
    Help,
    Usage,
    Queue,
    Model,
    Theme,
    Mcp,
    Login,
    Cd,
    Btw,
    Yolo,
    Thinking,
    Fast,
    Workflow,
    Exit,
    Reload,
}

impl BuiltinRoute {
    fn from_name(name: &str) -> Self {
        match name {
            "/tasks" => Self::Tasks,
            "/compact" => Self::Compact,
            "/new" => Self::New,
            "/help" => Self::Help,
            "/usage" => Self::Usage,
            "/queue" => Self::Queue,
            "/model" => Self::Model,
            "/theme" => Self::Theme,
            "/mcp" => Self::Mcp,
            "/login" => Self::Login,
            "/cd" => Self::Cd,
            "/btw" => Self::Btw,
            "/yolo" => Self::Yolo,
            "/thinking" => Self::Thinking,
            "/fast" => Self::Fast,
            "/workflow" => Self::Workflow,
            "/exit" => Self::Exit,
            "/reload" => Self::Reload,
            _ => unreachable!("builtin metadata and route enum diverged"),
        }
    }
}

#[derive(Clone)]
pub(crate) enum CommandRoute {
    Builtin(BuiltinRoute),
    Custom(CustomCommand),
    Mcp(McpPromptInfo),
    Lua { plugin: Arc<str>, name: Arc<str> },
}

pub(crate) struct RoutedCommand {
    pub target: InvocationTargetId,
    pub route: CommandRoute,
    pub arguments: String,
    pub depth: usize,
    pub lifecycle: InvocationLifecycle,
}

#[derive(Clone)]
struct RouteBehavior {
    route: CommandRoute,
    command_tx: flume::Sender<RoutedCommand>,
}

impl CommandBehavior for RouteBehavior {
    fn execute(&self, invocation: CommandInvocation) -> CommandFuture<Result<(), CommandError>> {
        let result = self.command_tx.send(RoutedCommand {
            target: invocation.target_id,
            route: self.route.clone(),
            arguments: invocation.arguments.to_string(),
            depth: invocation.depth,
            lifecycle: invocation.lifecycle,
        });
        Box::pin(async move { result.map_err(|_| CommandError::StaleTarget) })
    }
}

struct LuaCompletion {
    handle: EventHandle,
    plugin: Arc<str>,
    sessions: Mutex<HashMap<CompletionSessionId, LuaCompletionSession>>,
}

struct LuaCompletionSession {
    legacy_id: u64,
    _trigger: maki_agent::CancelTrigger,
}

impl CommandCompletion for LuaCompletion {
    fn complete(
        &self,
        context: CompletionContext,
        cancellation: maki_commands::CancellationToken,
    ) -> CommandFuture<Result<Vec<CompletionItem>, CompletionError>> {
        let (trigger, cancel) = maki_agent::CancelToken::new();
        let legacy = self.legacy_context(&context, trigger);
        let Some(rx) = self
            .handle
            .collect_command_argument_items(legacy, cancel.clone())
        else {
            return Box::pin(async { Ok(Vec::new()) });
        };
        Box::pin(async move {
            let items = cancel.race(rx.recv_async()).await.ok().and_then(Result::ok);
            if cancellation.is_cancelled() {
                return Ok(Vec::new());
            }
            Ok(items
                .unwrap_or_default()
                .into_iter()
                .map(completion_item)
                .collect())
        })
    }

    fn lifecycle(
        &self,
        context: &CompletionContext,
        event: &CompletionLifecycleEvent,
        _cancellation: &maki_commands::CancellationToken,
    ) -> Result<(), CompletionError> {
        let (trigger, cancel) = maki_agent::CancelToken::new();
        let legacy = self.legacy_context(context, trigger);
        let (event, item) = legacy_lifecycle(event);
        self.handle
            .command_argument_lifecycle(legacy, event, item, cancel);
        Ok(())
    }
}

impl LuaCompletion {
    fn legacy_context(
        &self,
        context: &CompletionContext,
        trigger: maki_agent::CancelTrigger,
    ) -> CommandArgumentContext {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let next_id = sessions.len() as u64 + 1;
        let legacy_id = sessions
            .get(&context.session_id)
            .map_or(next_id, |session| session.legacy_id);
        sessions.insert(
            context.session_id,
            LuaCompletionSession {
                legacy_id,
                _trigger: trigger,
            },
        );
        CommandArgumentContext {
            command: Arc::clone(&context.invoked_name),
            plugin: Arc::clone(&self.plugin),
            args: context.arguments.to_string(),
            arg: context.argument.to_string(),
            index: context.argument_index,
            mode: context.mode.to_string(),
            session: legacy_id,
            generation: 0,
        }
    }
}

fn completion_item(item: CommandArgumentItem) -> CompletionItem {
    CompletionItem {
        label: Arc::from(item.label),
        insertion: Arc::from(item.insertion),
        description: item.description.map(Arc::from),
    }
}

fn legacy_lifecycle(
    event: &CompletionLifecycleEvent,
) -> (CommandArgumentLifecycle, Option<CommandArgumentItem>) {
    match event {
        CompletionLifecycleEvent::Highlight(item) => {
            (CommandArgumentLifecycle::Highlight, Some(legacy_item(item)))
        }
        CompletionLifecycleEvent::Accept(item) => {
            (CommandArgumentLifecycle::Accept, Some(legacy_item(item)))
        }
        CompletionLifecycleEvent::Cancel => (CommandArgumentLifecycle::Cancel, None),
    }
}

fn legacy_item(item: &CompletionItem) -> CommandArgumentItem {
    CommandArgumentItem {
        label: item.label.to_string(),
        insertion: item.insertion.to_string(),
        description: item.description.as_ref().map(ToString::to_string),
    }
}

pub(crate) struct CommandRuntime {
    pub registry: CommandRegistry,
    application: Producer,
    mcp: Producer,
    plugins: Producer,
    command_tx: flume::Sender<RoutedCommand>,
    lua_event_handle: EventHandle,
    theme_completion: Arc<ThemeArgSource>,
    #[cfg(test)]
    command_rx: flume::Receiver<RoutedCommand>,
}

impl CommandRuntime {
    #[cfg(test)]
    pub fn new_for_test(
        custom_commands: &[CustomCommand],
        mcp_prompts: &[McpPromptInfo],
        lua_commands: &[LuaCommandInfo],
        model_completion: Arc<ModelArgSource>,
        theme_completion: Arc<ThemeArgSource>,
        lua_event_handle: EventHandle,
    ) -> Self {
        Self::new(
            custom_commands,
            mcp_prompts,
            lua_commands,
            model_completion,
            theme_completion,
            lua_event_handle,
        )
        .0
    }

    pub fn new(
        custom_commands: &[CustomCommand],
        mcp_prompts: &[McpPromptInfo],
        lua_commands: &[LuaCommandInfo],
        model_completion: Arc<ModelArgSource>,
        theme_completion: Arc<ThemeArgSource>,
        lua_event_handle: EventHandle,
    ) -> (Self, flume::Receiver<RoutedCommand>) {
        let registry = CommandRegistry::new();
        let (command_tx, command_rx) = flume::unbounded();
        let builtin = registry.create_producer(ProducerPrecedence::Builtin);
        builtin
            .replace(
                BUILTIN_COMMANDS
                    .iter()
                    .map(|command| Registration {
                        spec: CommandSpec {
                            name: Arc::from(command.name),
                            aliases: command.aliases.iter().copied().map(Arc::from).collect(),
                            arguments: if command.max_args == usize::MAX {
                                ArgumentArity::unbounded(0)
                            } else {
                                ArgumentArity::bounded(0, command.max_args)
                            },
                            docs: CommandDocs {
                                summary: Arc::from(command.description),
                                argument_hint: None,
                            },
                        },
                        behavior: Arc::new(RouteBehavior {
                            route: CommandRoute::Builtin(BuiltinRoute::from_name(command.name)),
                            command_tx: command_tx.clone(),
                        }),
                        completion: match BuiltinRoute::from_name(command.name) {
                            BuiltinRoute::Model => {
                                Some(Arc::clone(&model_completion) as Arc<dyn CommandCompletion>)
                            }
                            BuiltinRoute::Theme => {
                                Some(Arc::clone(&theme_completion) as Arc<dyn CommandCompletion>)
                            }
                            _ => None,
                        },
                    })
                    .collect(),
            )
            .expect("static builtin registrations are valid");
        let application = registry.create_producer(ProducerPrecedence::Application);
        let mcp = registry.create_producer(ProducerPrecedence::Mcp);
        let plugins = registry.create_producer(ProducerPrecedence::Plugin);
        let runtime = Self {
            registry,
            application,
            mcp,
            plugins,
            command_tx,
            lua_event_handle,
            theme_completion,
            #[cfg(test)]
            command_rx: command_rx.clone(),
        };
        runtime.replace_custom(custom_commands);
        runtime.replace_mcp(mcp_prompts);
        runtime.replace_lua(lua_commands);
        (runtime, command_rx)
    }

    #[cfg(test)]
    pub fn try_recv_for_test(&self) -> Option<RoutedCommand> {
        self.command_rx.try_recv().ok()
    }

    pub(crate) fn finish_theme_preview(&self, target: InvocationTargetId, commit: bool) {
        self.theme_completion.finish(target, commit);
    }

    fn replace_custom(&self, commands: &[CustomCommand]) {
        self.application
            .replace(
                commands
                    .iter()
                    .cloned()
                    .map(|command| {
                        let name = command.display_name();
                        let description = command.description.clone();
                        bridge_registration(
                            name,
                            if command.has_args() { None } else { Some(0) },
                            &description,
                            CommandRoute::Custom(command),
                            &self.command_tx,
                            None,
                        )
                    })
                    .collect(),
            )
            .expect("custom command metadata is valid");
    }

    pub fn replace_mcp(&self, prompts: &[McpPromptInfo]) {
        self.mcp
            .replace(
                prompts
                    .iter()
                    .cloned()
                    .map(|prompt| {
                        let description = prompt.description.clone();
                        bridge_registration(
                            format!("/{}", prompt.display_name),
                            None,
                            &description,
                            CommandRoute::Mcp(prompt),
                            &self.command_tx,
                            None,
                        )
                    })
                    .collect(),
            )
            .expect("MCP command metadata is valid");
    }

    pub fn replace_lua(&self, commands: &[LuaCommandInfo]) {
        self.plugins
            .replace(
                commands
                    .iter()
                    .map(|command| {
                        let completion = command.has_argument_completion.then(|| {
                            Arc::new(LuaCompletion {
                                handle: self.lua_event_handle.clone(),
                                plugin: Arc::clone(&command.plugin),
                                sessions: Mutex::new(HashMap::new()),
                            }) as Arc<dyn CommandCompletion>
                        });
                        bridge_registration(
                            Arc::clone(&command.name),
                            (command.max_args != usize::MAX).then_some(command.max_args),
                            command.description.as_ref(),
                            CommandRoute::Lua {
                                plugin: Arc::clone(&command.plugin),
                                name: Arc::clone(&command.name),
                            },
                            &self.command_tx,
                            completion,
                        )
                    })
                    .collect(),
            )
            .expect("Lua command metadata is valid");
    }
}

fn bridge_registration(
    name: impl Into<Arc<str>>,
    max_args: Option<usize>,
    description: &str,
    route: CommandRoute,
    command_tx: &flume::Sender<RoutedCommand>,
    completion: Option<Arc<dyn CommandCompletion>>,
) -> Registration {
    Registration {
        spec: CommandSpec {
            name: name.into(),
            aliases: Arc::from([]),
            arguments: max_args
                .map(|max| ArgumentArity::bounded(0, max))
                .unwrap_or_else(|| ArgumentArity::unbounded(0)),
            docs: CommandDocs {
                summary: Arc::from(description),
                argument_hint: None,
            },
        },
        behavior: Arc::new(RouteBehavior {
            route,
            command_tx: command_tx.clone(),
        }),
        completion,
    }
}

pub(crate) fn complete(command: &RoutedCommand, classification: CommandClassification) {
    command.lifecycle.transition(classification);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::ArcSwapOption;
    use maki_commands::{CommandClassification, CommandError, InputDispatch};

    use super::{CommandRuntime, RoutedCommand};
    use crate::{
        components::arg_completion::{ModelArgSource, ThemeArgSource},
        theme::ThemesProvider,
    };

    fn runtime() -> (CommandRuntime, flume::Receiver<RoutedCommand>) {
        runtime_with_themes(Arc::new(crate::theme::InMemoryThemesProvider::bundled()))
    }

    fn runtime_with_themes(
        provider: Arc<crate::theme::InMemoryThemesProvider>,
    ) -> (CommandRuntime, flume::Receiver<RoutedCommand>) {
        CommandRuntime::new(
            &[],
            &[],
            &[],
            Arc::new(ModelArgSource::new(Arc::new(ArcSwapOption::empty()))),
            Arc::new(ThemeArgSource::new(provider)),
            maki_lua::EventHandle::disconnected_for_test(),
        )
    }

    #[test]
    fn routed_command_preserves_target_and_terminal_lifecycle() {
        let (runtime, rx) = runtime();
        let target = runtime.registry.create_target();
        let InputDispatch::Dispatched(dispatch) =
            smol::block_on(runtime.registry.dispatch_input("/help", 0, target)).unwrap()
        else {
            panic!("expected dispatch");
        };
        let routed = rx.recv().unwrap();
        assert_eq!(routed.target, target);
        super::complete(&routed, CommandClassification::Completed);
        assert_eq!(
            smol::block_on(dispatch.classification()),
            CommandClassification::Completed
        );
    }

    fn theme_context(
        runtime: &CommandRuntime,
        target: maki_commands::InvocationTargetId,
        theme: &str,
    ) -> (
        maki_commands::CompletionSession,
        maki_commands::CompletionCandidate,
    ) {
        let session = runtime
            .registry
            .open_completion(runtime.registry.resolve("/theme").unwrap(), target)
            .unwrap();
        let maki_commands::CompletionResult::Items(mut items) = smol::block_on(session.complete(
            Arc::from(theme),
            Arc::from(theme),
            0,
            Arc::from("build"),
        )) else {
            panic!("expected theme items");
        };
        let candidate = items
            .drain(..)
            .find(|item| item.item().insertion.as_ref() == theme)
            .unwrap();
        (session, candidate)
    }

    #[test]
    fn overlapping_theme_previews_restore_deterministically() {
        let _guard = crate::theme::theme_test_guard();
        let provider = Arc::new(crate::theme::InMemoryThemesProvider::bundled());
        let baseline = provider.current_theme_name();
        let (runtime, _) = runtime_with_themes(Arc::clone(&provider));
        let first_target = runtime.registry.create_target();
        let second_target = runtime.registry.create_target();
        let (first, first_item) = theme_context(&runtime, first_target, "dracula");
        let (second, second_item) = theme_context(&runtime, second_target, "tokyonight");

        first.highlight(&first_item).unwrap();
        second.highlight(&second_item).unwrap();
        first.cancel().unwrap();
        assert_eq!(
            **crate::theme::current(),
            provider.load("tokyonight").unwrap()
        );
        second.cancel().unwrap();
        assert_eq!(**crate::theme::current(), provider.load(&baseline).unwrap());
    }

    #[test]
    fn dropped_target_is_classified_stale() {
        let (runtime, rx) = runtime();
        let target = runtime.registry.create_target();
        let InputDispatch::Dispatched(dispatch) =
            smol::block_on(runtime.registry.dispatch_input("/help", 0, target)).unwrap()
        else {
            panic!("expected dispatch");
        };
        let routed = rx.recv().unwrap();
        routed
            .lifecycle
            .transition(CommandClassification::Failed(CommandError::StaleTarget));
        assert_eq!(
            smol::block_on(dispatch.classification()),
            CommandClassification::Failed(CommandError::StaleTarget)
        );
    }
}
