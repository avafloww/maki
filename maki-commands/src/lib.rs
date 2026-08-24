//! Frontend-neutral contracts for slash commands.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;

pub type CommandFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: Arc<str>,
    pub aliases: Arc<[Arc<str>]>,
    pub arguments: ArgumentArity,
    pub docs: CommandDocs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgumentArity {
    pub min: usize,
    pub max: Option<usize>,
}

impl ArgumentArity {
    pub const NONE: Self = Self::exactly(0);
    pub const OPTIONAL: Self = Self::bounded(0, 1);
    pub const ONE: Self = Self::exactly(1);
    pub const ANY: Self = Self::unbounded(0);
    pub const ONE_OR_MORE: Self = Self::unbounded(1);

    pub const fn exactly(count: usize) -> Self {
        Self {
            min: count,
            max: Some(count),
        }
    }

    pub const fn bounded(min: usize, max: usize) -> Self {
        Self {
            min,
            max: Some(max),
        }
    }

    pub const fn unbounded(min: usize) -> Self {
        Self { min, max: None }
    }

    pub fn accepts(self, count: usize) -> bool {
        count >= self.min && self.max.is_none_or(|max| count <= max)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandDocs {
    pub summary: Arc<str>,
    pub argument_hint: Option<Arc<str>>,
}

pub struct Registration {
    pub spec: CommandSpec,
    pub behavior: Arc<dyn CommandBehavior>,
    pub completion: Option<Arc<dyn CommandCompletion>>,
}

impl fmt::Debug for Registration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Registration")
            .field("spec", &self.spec)
            .field("behavior", &"dyn CommandBehavior")
            .field(
                "completion",
                &self.completion.as_ref().map(|_| "dyn CommandCompletion"),
            )
            .finish()
    }
}

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(u64);

        #[allow(dead_code)]
        impl $name {
            pub(crate) const fn new(value: u64) -> Self {
                Self(value)
            }
        }
    };
}

opaque_id!(ProducerId);
opaque_id!(CommandId);
opaque_id!(CompletionSessionId);
opaque_id!(InvocationTargetId);

struct RegistrationRecord {
    producer_id: ProducerId,
    command_id: CommandId,
    registration: Registration,
}

#[derive(Clone)]
pub struct ResolvedCommand {
    record: Arc<RegistrationRecord>,
    invoked_name: Arc<str>,
}

impl ResolvedCommand {
    pub fn producer_id(&self) -> ProducerId {
        self.record.producer_id
    }

    pub fn command_id(&self) -> CommandId {
        self.record.command_id
    }

    pub fn spec(&self) -> &CommandSpec {
        &self.record.registration.spec
    }

    pub fn behavior(&self) -> &Arc<dyn CommandBehavior> {
        &self.record.registration.behavior
    }

    pub fn completion(&self) -> Option<&Arc<dyn CommandCompletion>> {
        self.record.registration.completion.as_ref()
    }

    pub fn invoked_name(&self) -> &str {
        &self.invoked_name
    }
}

impl fmt::Debug for ResolvedCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedCommand")
            .field("command_id", &self.record.command_id)
            .field("spec", &self.record.registration.spec)
            .field("invoked_name", &self.invoked_name)
            .finish_non_exhaustive()
    }
}

pub trait CommandBehavior: Send + Sync + 'static {
    fn execute(&self, invocation: CommandInvocation) -> CommandFuture<Result<(), CommandError>>;
}

#[derive(Clone)]
pub struct CommandInvocation {
    pub command_id: CommandId,
    pub canonical_name: Arc<str>,
    pub invoked_name: Arc<str>,
    pub arguments: Arc<str>,
    pub depth: usize,
    pub target_id: InvocationTargetId,
    pub dispatcher: InvocationDispatcher,
    pub lifecycle: InvocationLifecycle,
}

impl fmt::Debug for CommandInvocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandInvocation")
            .field("command_id", &self.command_id)
            .field("canonical_name", &self.canonical_name)
            .field("invoked_name", &self.invoked_name)
            .field("arguments", &self.arguments)
            .field("depth", &self.depth)
            .field("target_id", &self.target_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct InvocationDispatcher(Arc<dyn DispatchCommands>);

impl InvocationDispatcher {
    pub fn new(dispatcher: Arc<dyn DispatchCommands>) -> Self {
        Self(dispatcher)
    }

    pub fn dispatch(
        &self,
        request: DispatchRequest,
    ) -> CommandFuture<Result<CommandDispatch, CommandError>> {
        self.0.dispatch(request)
    }
}

pub trait DispatchCommands: Send + Sync + 'static {
    fn dispatch(
        &self,
        request: DispatchRequest,
    ) -> CommandFuture<Result<CommandDispatch, CommandError>>;
}

#[derive(Clone)]
pub struct DispatchRequest {
    pub input: Arc<str>,
    pub depth: usize,
    pub target_id: InvocationTargetId,
    pub lifecycle: InvocationLifecycle,
}

pub struct CommandDispatch {
    classification: CommandFuture<CommandClassification>,
}

impl CommandDispatch {
    pub fn new(classification: CommandFuture<CommandClassification>) -> Self {
        Self { classification }
    }

    pub fn classification(self) -> CommandFuture<CommandClassification> {
        self.classification
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandClassification {
    Completed,
    AgentTurnAccepted,
    Failed(CommandError),
}

#[derive(Clone)]
pub struct InvocationLifecycle(Arc<dyn ClassifyInvocation>);

impl InvocationLifecycle {
    pub fn new(lifecycle: Arc<dyn ClassifyInvocation>) -> Self {
        Self(lifecycle)
    }

    pub fn transition(&self, classification: CommandClassification) -> bool {
        self.0.transition(classification)
    }
}

pub trait ClassifyInvocation: Send + Sync + 'static {
    fn transition(&self, classification: CommandClassification) -> bool;
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RegistrationError {
    #[error("command name is invalid: {0}")]
    InvalidName(Arc<str>),
    #[error("command alias is invalid: {0}")]
    InvalidAlias(Arc<str>),
    #[error("command argument range {min}..={max} is invalid")]
    InvalidArgumentArity { min: usize, max: usize },
    #[error("command spelling is registered more than once: {0}")]
    DuplicateSpelling(Arc<str>),
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ResolutionError {
    #[error("unknown command: {0}")]
    UnknownCommand(Arc<str>),
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CommandError {
    #[error("invalid arguments for {command}: expected {expected}, got {actual}")]
    InvalidArguments {
        command: Arc<str>,
        expected: ArgumentArity,
        actual: usize,
    },
    #[error("command is not supported by this frontend: {0}")]
    UnsupportedFrontend(Arc<str>),
    #[error("the command target is no longer available")]
    StaleTarget,
    #[error("maximum command recursion depth exceeded")]
    MaximumDepth,
    #[error("command failed: {0}")]
    Producer(Arc<str>),
}

impl fmt::Display for ArgumentArity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.max {
            Some(max) if max == self.min => write!(formatter, "{}", self.min),
            Some(max) => write!(formatter, "{}..={max}", self.min),
            None => write!(formatter, "{} or more", self.min),
        }
    }
}

pub trait CommandCompletion: Send + Sync + 'static {
    fn complete(
        &self,
        context: CompletionContext,
        cancellation: CancellationToken,
    ) -> CommandFuture<Result<Vec<CompletionItem>, CompletionError>>;

    fn lifecycle(
        &self,
        _context: CompletionContext,
        _event: CompletionLifecycleEvent,
        _cancellation: CancellationToken,
    ) -> CommandFuture<Result<(), CompletionError>> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionContext {
    pub command_id: CommandId,
    pub canonical_name: Arc<str>,
    pub invoked_name: Arc<str>,
    pub arguments: Arc<str>,
    pub argument: Arc<str>,
    pub argument_index: usize,
    pub mode: Arc<str>,
    pub target_id: InvocationTargetId,
    pub session_id: CompletionSessionId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: Arc<str>,
    pub insertion: Arc<str>,
    pub description: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionLifecycleEvent {
    Highlight(CompletionItem),
    Accept(CompletionItem),
    Cancel,
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    #[allow(dead_code)]
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Debug, Clone)]
pub struct CompletionSession {
    id: CompletionSessionId,
    command: ResolvedCommand,
    target_id: InvocationTargetId,
}

impl CompletionSession {
    pub fn id(&self) -> CompletionSessionId {
        self.id
    }

    pub fn command(&self) -> &ResolvedCommand {
        &self.command
    }

    pub fn target_id(&self) -> InvocationTargetId {
        self.target_id
    }
}

#[derive(Debug, Clone)]
pub struct PendingCompletionRequest {
    pub context: CompletionContext,
    pub cancellation: CancellationToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionResult {
    Items(Vec<CompletionItem>),
    Stale,
    Cancelled,
    Failed(CompletionError),
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CompletionError {
    #[error("completion provider failed: {0}")]
    Producer(Arc<str>),
    #[error("the completion target is no longer available")]
    StaleTarget,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        ArgumentArity, CancellationToken, CommandBehavior, CommandCompletion, CommandDocs,
        CommandFuture, CommandInvocation, CommandSpec, CompletionContext, CompletionItem,
        Registration,
    };

    struct Behavior;

    impl CommandBehavior for Behavior {
        fn execute(
            &self,
            _invocation: CommandInvocation,
        ) -> CommandFuture<Result<(), super::CommandError>> {
            Box::pin(async { Ok(()) })
        }
    }

    struct Completion;

    impl CommandCompletion for Completion {
        fn complete(
            &self,
            _context: CompletionContext,
            _cancellation: CancellationToken,
        ) -> CommandFuture<Result<Vec<CompletionItem>, super::CompletionError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[test]
    fn standard_arities_accept_expected_counts() {
        assert!(ArgumentArity::NONE.accepts(0));
        assert!(!ArgumentArity::NONE.accepts(1));
        assert!(ArgumentArity::OPTIONAL.accepts(0));
        assert!(ArgumentArity::OPTIONAL.accepts(1));
        assert!(!ArgumentArity::OPTIONAL.accepts(2));
        assert!(!ArgumentArity::ONE_OR_MORE.accepts(0));
        assert!(ArgumentArity::ONE_OR_MORE.accepts(2));
    }

    #[test]
    fn registration_contains_behavior_and_completion() {
        let registration = Registration {
            spec: CommandSpec {
                name: Arc::from("/test"),
                aliases: Arc::from([Arc::from("/alias")]),
                arguments: ArgumentArity::ANY,
                docs: CommandDocs {
                    summary: Arc::from("Test command"),
                    argument_hint: Some(Arc::from("[value]")),
                },
            },
            behavior: Arc::new(Behavior),
            completion: Some(Arc::new(Completion)),
        };

        assert_eq!(registration.spec.name.as_ref(), "/test");
        assert!(registration.completion.is_some());
    }

    #[test]
    fn cancellation_is_shared_and_idempotent() {
        let token = CancellationToken::default();
        let observer = token.clone();
        token.cancel();
        token.cancel();
        assert!(observer.is_cancelled());
    }

    #[test]
    fn opaque_ids_are_distinct_types() {
        let producer = super::ProducerId::new(1);
        let command = super::CommandId::new(1);
        assert_eq!(producer, super::ProducerId::new(1));
        assert_eq!(command, super::CommandId::new(1));
    }
}
