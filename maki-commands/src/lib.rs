//! Frontend-neutral contracts for slash commands.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Poll, Waker};

use thiserror::Error;

pub type CommandFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

pub const MAX_COMMAND_DEPTH: usize = 8;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProducerPrecedence {
    Plugin,
    Mcp,
    Application,
    Builtin,
}

#[derive(Clone)]
pub struct CommandRegistry(Arc<RegistryInner>);

struct RegistryInner {
    state: Mutex<RegistryState>,
}

struct RegistryState {
    next_id: u64,
    generation: u64,
    producers: Vec<ProducerSlot>,
    winners: HashMap<String, Winner>,
    projection: Arc<[ResolvedCommand]>,
}

struct ProducerSlot {
    id: ProducerId,
    precedence: ProducerPrecedence,
    creation_order: u64,
    records: Vec<Arc<RegistrationRecord>>,
}

#[derive(Clone)]
struct Winner {
    record: Arc<RegistrationRecord>,
    canonical: bool,
    precedence: ProducerPrecedence,
    creation_order: u64,
}

#[derive(Debug, Clone)]
pub struct RegistrySnapshot {
    generation: u64,
    commands: Arc<[ResolvedCommand]>,
}

impl RegistrySnapshot {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn commands(&self) -> &[ResolvedCommand] {
        &self.commands
    }
}

impl fmt::Debug for InputDispatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotCommand => formatter.write_str("NotCommand"),
            Self::UnknownCommandInput => formatter.write_str("UnknownCommandInput"),
            Self::Dispatched(_) => formatter.write_str("Dispatched(..)"),
        }
    }
}

pub enum InputDispatch {
    NotCommand,
    UnknownCommandInput,
    Dispatched(CommandDispatch),
}

#[derive(Clone)]
pub struct Producer {
    registry: Weak<RegistryInner>,
    id: ProducerId,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self(Arc::new(RegistryInner {
            state: Mutex::new(RegistryState {
                next_id: 1,
                generation: 0,
                producers: Vec::new(),
                winners: HashMap::new(),
                projection: Arc::from([]),
            }),
        }))
    }

    pub fn create_producer(&self, precedence: ProducerPrecedence) -> Producer {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let id = ProducerId::new(state.take_id());
        let creation_order = state.producers.len() as u64;
        state.producers.push(ProducerSlot {
            id,
            precedence,
            creation_order,
            records: Vec::new(),
        });
        Producer {
            registry: Arc::downgrade(&self.0),
            id,
        }
    }

    pub fn create_target(&self) -> InvocationTargetId {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        InvocationTargetId::new(state.take_id())
    }

    pub fn resolve(&self, spelling: &str) -> Result<ResolvedCommand, ResolutionError> {
        let normalized = normalize(spelling);
        let state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state
            .winners
            .get(&normalized)
            .map(|winner| ResolvedCommand {
                record: Arc::clone(&winner.record),
                invoked_name: Arc::from(spelling),
            })
            .ok_or_else(|| ResolutionError::UnknownCommand(Arc::from(spelling)))
    }

    pub fn snapshot(&self) -> RegistrySnapshot {
        let state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        RegistrySnapshot {
            generation: state.generation,
            commands: Arc::clone(&state.projection),
        }
    }

    pub fn dispatch_input(
        &self,
        input: &str,
        depth: usize,
        target_id: InvocationTargetId,
    ) -> CommandFuture<Result<InputDispatch, CommandError>> {
        let Some(parsed) = ParsedInput::parse(input) else {
            return Box::pin(async { Ok(InputDispatch::NotCommand) });
        };
        let Ok(command) = self.resolve(parsed.name) else {
            return Box::pin(async { Ok(InputDispatch::UnknownCommandInput) });
        };
        if depth > MAX_COMMAND_DEPTH {
            return Box::pin(async { Err(CommandError::MaximumDepth) });
        }
        let arguments: Arc<str> = Arc::from(parsed.arguments);
        let count = arguments.split_whitespace().count();
        if !command.spec().arguments.accepts(count) {
            return Box::pin(async move {
                Err(CommandError::InvalidArguments {
                    command: Arc::clone(&command.spec().name),
                    expected: command.spec().arguments,
                    actual: count,
                })
            });
        }

        let registry = self.clone();
        Box::pin(async move {
            let (lifecycle, classification) = classification_channel();
            let invocation = command.invocation(
                arguments,
                depth,
                target_id,
                InvocationDispatcher::new(Arc::new(registry)),
                lifecycle.clone(),
            );
            if let Err(error) = command.behavior().execute(invocation).await {
                lifecycle.transition(CommandClassification::Failed(error.clone()));
                return Err(error);
            }
            Ok(InputDispatch::Dispatched(CommandDispatch::new(
                classification,
            )))
        })
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DispatchCommands for CommandRegistry {
    fn dispatch(
        &self,
        request: DispatchRequest,
    ) -> CommandFuture<Result<CommandDispatch, CommandError>> {
        let registry = self.clone();
        Box::pin(async move {
            let Some(parsed) = ParsedInput::parse(&request.input) else {
                return Err(CommandError::Producer(Arc::from(
                    "input is not a slash command",
                )));
            };
            let command = registry
                .resolve(parsed.name)
                .map_err(|_| CommandError::UnknownCommand(Arc::from(parsed.name)))?;
            registry
                .dispatch_resolved(
                    command,
                    parsed.arguments,
                    request.depth,
                    request.target_id,
                    request.lifecycle,
                )
                .await
        })
    }
}

impl CommandRegistry {
    pub fn dispatch_resolved(
        &self,
        command: ResolvedCommand,
        arguments: &str,
        depth: usize,
        target_id: InvocationTargetId,
        lifecycle: InvocationLifecycle,
    ) -> CommandFuture<Result<CommandDispatch, CommandError>> {
        let count = arguments.split_whitespace().count();
        if depth > MAX_COMMAND_DEPTH {
            lifecycle.transition(CommandClassification::Failed(CommandError::MaximumDepth));
            return Box::pin(async { Err(CommandError::MaximumDepth) });
        }
        if !command.spec().arguments.accepts(count) {
            let error = CommandError::InvalidArguments {
                command: Arc::clone(&command.spec().name),
                expected: command.spec().arguments,
                actual: count,
            };
            lifecycle.transition(CommandClassification::Failed(error.clone()));
            return Box::pin(async move { Err(error) });
        }
        let registry = self.clone();
        let arguments: Arc<str> = Arc::from(arguments);
        Box::pin(async move {
            let invocation = command.invocation(
                arguments,
                depth,
                target_id,
                InvocationDispatcher::new(Arc::new(registry)),
                lifecycle.clone(),
            );
            command
                .behavior()
                .execute(invocation)
                .await
                .inspect_err(|error| {
                    lifecycle.transition(CommandClassification::Failed(error.clone()));
                })?;
            Ok(CommandDispatch::new(lifecycle.classification()))
        })
    }
}

impl Producer {
    pub fn id(&self) -> ProducerId {
        self.id
    }

    pub fn replace(&self, registrations: Vec<Registration>) -> Result<(), RegistrationError> {
        let validated = validate_registrations(registrations)?;
        let registry = self
            .registry
            .upgrade()
            .ok_or(RegistrationError::StaleProducer)?;
        let mut state = registry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let position = state
            .producers
            .iter()
            .position(|producer| producer.id == self.id)
            .ok_or(RegistrationError::StaleProducer)?;
        let records = validated
            .into_iter()
            .map(|registration| {
                Arc::new(RegistrationRecord {
                    producer_id: self.id,
                    command_id: CommandId::new(state.take_id()),
                    registration,
                })
            })
            .collect();
        state.producers[position].records = records;
        state.generation += 1;
        state.rebuild();
        Ok(())
    }

    pub fn remove(&self) -> bool {
        let Some(registry) = self.registry.upgrade() else {
            return false;
        };
        let mut state = registry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(position) = state
            .producers
            .iter()
            .position(|producer| producer.id == self.id)
        else {
            return false;
        };
        state.producers.remove(position);
        state.generation += 1;
        state.rebuild();
        true
    }
}

impl RegistryState {
    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn rebuild(&mut self) {
        let mut winners = HashMap::new();
        for producer in &self.producers {
            for record in &producer.records {
                let spec = &record.registration.spec;
                insert_winner(
                    &mut winners,
                    record,
                    &spec.name,
                    true,
                    producer.precedence,
                    producer.creation_order,
                );
                for alias in spec.aliases.iter() {
                    insert_winner(
                        &mut winners,
                        record,
                        alias,
                        false,
                        producer.precedence,
                        producer.creation_order,
                    );
                }
            }
        }
        let mut producers = self.producers.iter().collect::<Vec<_>>();
        producers.sort_by_key(|producer| (producer.precedence, producer.creation_order));
        let mut projection = Vec::new();
        for producer in producers {
            for record in &producer.records {
                for spelling in std::iter::once(&record.registration.spec.name)
                    .chain(record.registration.spec.aliases.iter())
                {
                    if winners
                        .get(&normalize(spelling))
                        .is_some_and(|winner| winner.record.command_id == record.command_id)
                    {
                        projection.push(ResolvedCommand {
                            record: Arc::clone(record),
                            invoked_name: Arc::clone(spelling),
                        });
                    }
                }
            }
        }
        self.winners = winners;
        self.projection = projection.into();
    }
}

fn insert_winner(
    winners: &mut HashMap<String, Winner>,
    record: &Arc<RegistrationRecord>,
    spelling: &Arc<str>,
    canonical: bool,
    precedence: ProducerPrecedence,
    creation_order: u64,
) {
    let candidate = Winner {
        record: Arc::clone(record),
        canonical,
        precedence,
        creation_order,
    };
    let key = normalize(spelling);
    match winners.get(&key) {
        Some(current) if !candidate.precedes(current) => {}
        _ => {
            winners.insert(key, candidate);
        }
    }
}

impl Winner {
    fn precedes(&self, other: &Self) -> bool {
        (self.precedence, !self.canonical, self.creation_order)
            < (other.precedence, !other.canonical, other.creation_order)
    }
}

fn validate_registrations(
    registrations: Vec<Registration>,
) -> Result<Vec<Registration>, RegistrationError> {
    let mut spellings = HashSet::new();
    for registration in &registrations {
        validate_spelling(&registration.spec.name)
            .map_err(|_| RegistrationError::InvalidName(Arc::clone(&registration.spec.name)))?;
        if registration
            .spec
            .arguments
            .max
            .is_some_and(|max| registration.spec.arguments.min > max)
        {
            return Err(RegistrationError::InvalidArgumentArity {
                min: registration.spec.arguments.min,
                max: registration.spec.arguments.max.unwrap_or_default(),
            });
        }
        for (spelling, alias) in std::iter::once((&registration.spec.name, false))
            .chain(registration.spec.aliases.iter().map(|alias| (alias, true)))
        {
            if validate_spelling(spelling).is_err() {
                return Err(if alias {
                    RegistrationError::InvalidAlias(Arc::clone(spelling))
                } else {
                    RegistrationError::InvalidName(Arc::clone(spelling))
                });
            }
            if !spellings.insert(normalize(spelling)) {
                return Err(RegistrationError::DuplicateSpelling(Arc::clone(spelling)));
            }
        }
    }
    Ok(registrations)
}

fn validate_spelling(spelling: &str) -> Result<(), ()> {
    if spelling.len() > 1 && spelling.starts_with('/') && !spelling.chars().any(char::is_whitespace)
    {
        Ok(())
    } else {
        Err(())
    }
}

fn normalize(spelling: &str) -> String {
    spelling.to_ascii_lowercase()
}

struct ParsedInput<'a> {
    name: &'a str,
    arguments: &'a str,
}

impl<'a> ParsedInput<'a> {
    fn parse(input: &'a str) -> Option<Self> {
        let trimmed = input.trim_start();
        let name_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        let name = &trimmed[..name_end];
        name.starts_with('/').then(|| Self {
            name,
            arguments: trimmed[name_end..].trim(),
        })
    }
}

#[derive(Clone)]
pub struct ResolvedCommand {
    record: Arc<RegistrationRecord>,
    invoked_name: Arc<str>,
}

impl ResolvedCommand {
    fn invocation(
        &self,
        arguments: Arc<str>,
        depth: usize,
        target_id: InvocationTargetId,
        dispatcher: InvocationDispatcher,
        lifecycle: InvocationLifecycle,
    ) -> CommandInvocation {
        CommandInvocation {
            command_id: self.command_id(),
            canonical_name: Arc::clone(&self.spec().name),
            invoked_name: Arc::clone(&self.invoked_name),
            arguments,
            depth,
            target_id,
            dispatcher,
            lifecycle,
        }
    }

    pub fn producer_id(&self) -> ProducerId {
        self.record.producer_id
    }

    pub fn command_id(&self) -> CommandId {
        self.record.command_id
    }

    pub fn spec(&self) -> &CommandSpec {
        &self.record.registration.spec
    }

    pub fn behavior(&self) -> Arc<dyn CommandBehavior> {
        Arc::clone(&self.record.registration.behavior)
    }

    pub fn completion(&self) -> Option<Arc<dyn CommandCompletion>> {
        self.record.registration.completion.clone()
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

struct ClassificationState {
    classification: Mutex<Option<CommandClassification>>,
    waker: Mutex<Option<Waker>>,
}

impl ClassifyInvocation for ClassificationState {
    fn transition(&self, classification: CommandClassification) -> bool {
        let mut current = self
            .classification
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if current.is_some() {
            return false;
        }
        *current = Some(classification);
        drop(current);
        if let Some(waker) = self
            .waker
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            waker.wake();
        }
        true
    }

    fn poll_classification(&self, waker: &Waker) -> Poll<CommandClassification> {
        if let Some(classification) = self
            .classification
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            return Poll::Ready(classification);
        }
        *self.waker.lock().unwrap_or_else(|error| error.into_inner()) = Some(waker.clone());
        Poll::Pending
    }
}

fn classification_channel() -> (InvocationLifecycle, CommandFuture<CommandClassification>) {
    let lifecycle = InvocationLifecycle(Arc::new(ClassificationState {
        classification: Mutex::new(None),
        waker: Mutex::new(None),
    }));
    let classification = lifecycle.classification();
    (lifecycle, classification)
}

impl InvocationLifecycle {
    pub fn new(lifecycle: Arc<dyn ClassifyInvocation>) -> Self {
        Self(lifecycle)
    }

    pub fn transition(&self, classification: CommandClassification) -> bool {
        self.0.transition(classification)
    }

    pub fn classification(&self) -> CommandFuture<CommandClassification> {
        let lifecycle = self.clone();
        Box::pin(poll_fn(move |context| {
            lifecycle.0.poll_classification(context.waker())
        }))
    }
}

pub trait ClassifyInvocation: Send + Sync + 'static {
    fn transition(&self, classification: CommandClassification) -> bool;
    fn poll_classification(&self, waker: &Waker) -> Poll<CommandClassification>;
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RegistrationError {
    #[error("the producer is no longer registered")]
    StaleProducer,
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
    #[error("unknown command: {0}")]
    UnknownCommand(Arc<str>),
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

    fn registration(name: &str, aliases: &[&str], arity: ArgumentArity) -> Registration {
        Registration {
            spec: CommandSpec {
                name: Arc::from(name),
                aliases: aliases.iter().map(|alias| Arc::from(*alias)).collect(),
                arguments: arity,
                docs: CommandDocs {
                    summary: Arc::from("test"),
                    argument_hint: None,
                },
            },
            behavior: Arc::new(Behavior),
            completion: None,
        }
    }

    #[test]
    fn replacement_is_atomic() {
        let registry = super::CommandRegistry::new();
        let producer = registry.create_producer(super::ProducerPrecedence::Builtin);
        producer
            .replace(vec![registration("/old", &[], ArgumentArity::ANY)])
            .unwrap();
        let generation = registry.snapshot().generation();

        producer
            .replace(vec![
                registration("/new", &[], ArgumentArity::ANY),
                registration("/other", &[], ArgumentArity::ANY),
            ])
            .unwrap();

        assert_eq!(registry.snapshot().generation(), generation + 1);
        assert!(registry.resolve("/old").is_err());
        assert!(registry.resolve("/new").is_ok());
        assert!(registry.resolve("/other").is_ok());
    }

    #[test]
    fn invalid_replacement_preserves_generation() {
        let registry = super::CommandRegistry::new();
        let producer = registry.create_producer(super::ProducerPrecedence::Builtin);
        producer
            .replace(vec![registration("/valid", &[], ArgumentArity::ANY)])
            .unwrap();
        let before = registry.snapshot();

        assert!(
            producer
                .replace(vec![registration("invalid", &[], ArgumentArity::ANY)])
                .is_err()
        );

        assert_eq!(registry.snapshot().generation(), before.generation());
        assert!(registry.resolve("/valid").is_ok());
    }

    #[test]
    fn duplicate_normalized_spelling_is_rejected() {
        let registry = super::CommandRegistry::new();
        let producer = registry.create_producer(super::ProducerPrecedence::Builtin);
        let error = producer
            .replace(vec![registration("/test", &["/TEST"], ArgumentArity::ANY)])
            .unwrap_err();
        assert!(matches!(
            error,
            super::RegistrationError::DuplicateSpelling(_)
        ));
    }

    #[test]
    fn precedence_matrix_uses_one_winner() {
        let registry = super::CommandRegistry::new();
        let builtin = registry.create_producer(super::ProducerPrecedence::Builtin);
        let application = registry.create_producer(super::ProducerPrecedence::Application);
        let mcp = registry.create_producer(super::ProducerPrecedence::Mcp);
        let plugin = registry.create_producer(super::ProducerPrecedence::Plugin);
        for producer in [&builtin, &application, &mcp, &plugin] {
            producer
                .replace(vec![registration("/same", &[], ArgumentArity::ANY)])
                .unwrap();
        }
        assert_eq!(
            registry.resolve("/same").unwrap().producer_id(),
            plugin.id()
        );
    }

    #[test]
    fn canonical_beats_alias_at_equal_precedence() {
        let registry = super::CommandRegistry::new();
        let alias = registry.create_producer(super::ProducerPrecedence::Application);
        let canonical = registry.create_producer(super::ProducerPrecedence::Application);
        alias
            .replace(vec![registration("/other", &["/same"], ArgumentArity::ANY)])
            .unwrap();
        canonical
            .replace(vec![registration("/same", &[], ArgumentArity::ANY)])
            .unwrap();
        assert_eq!(
            registry.resolve("/same").unwrap().producer_id(),
            canonical.id()
        );
    }

    #[test]
    fn creation_order_breaks_equal_ties() {
        let registry = super::CommandRegistry::new();
        let first = registry.create_producer(super::ProducerPrecedence::Application);
        let second = registry.create_producer(super::ProducerPrecedence::Application);
        first
            .replace(vec![registration("/same", &[], ArgumentArity::ANY)])
            .unwrap();
        second
            .replace(vec![registration("/same", &[], ArgumentArity::ANY)])
            .unwrap();
        assert_eq!(registry.resolve("/same").unwrap().producer_id(), first.id());
    }

    #[test]
    fn palette_and_resolve_share_winners() {
        let registry = super::CommandRegistry::new();
        let builtin = registry.create_producer(super::ProducerPrecedence::Builtin);
        let plugin = registry.create_producer(super::ProducerPrecedence::Plugin);
        builtin
            .replace(vec![registration("/same", &[], ArgumentArity::ANY)])
            .unwrap();
        plugin
            .replace(vec![registration("/same", &[], ArgumentArity::ANY)])
            .unwrap();
        let resolved = registry.resolve("/same").unwrap();
        let snapshot = registry.snapshot();
        assert_eq!(snapshot.commands().len(), 1);
        assert_eq!(snapshot.commands()[0].command_id(), resolved.command_id());
    }

    #[test]
    fn snapshot_contains_each_winning_spelling_in_deterministic_order() {
        let registry = super::CommandRegistry::new();
        let builtin = registry.create_producer(super::ProducerPrecedence::Builtin);
        let plugin = registry.create_producer(super::ProducerPrecedence::Plugin);
        builtin
            .replace(vec![registration(
                "/builtin",
                &["/shared", "/builtin-alias"],
                ArgumentArity::ANY,
            )])
            .unwrap();
        plugin
            .replace(vec![
                registration("/plugin", &["/shared", "/plugin-alias"], ArgumentArity::ANY),
                registration("/second", &["/second-alias"], ArgumentArity::ANY),
            ])
            .unwrap();

        let snapshot = registry.snapshot();
        let spellings = snapshot
            .commands()
            .iter()
            .map(|command| command.invoked_name())
            .collect::<Vec<_>>();
        assert_eq!(
            spellings,
            [
                "/plugin",
                "/shared",
                "/plugin-alias",
                "/second",
                "/second-alias",
                "/builtin",
                "/builtin-alias",
            ]
        );
        assert!(
            snapshot
                .commands()
                .iter()
                .filter(|command| command.invoked_name() == "/shared")
                .all(|command| command.producer_id() == plugin.id())
        );
    }

    #[test]
    fn removing_winner_restores_colliding_spelling() {
        let registry = super::CommandRegistry::new();
        let builtin = registry.create_producer(super::ProducerPrecedence::Builtin);
        let plugin = registry.create_producer(super::ProducerPrecedence::Plugin);
        builtin
            .replace(vec![registration("/shared", &[], ArgumentArity::ANY)])
            .unwrap();
        plugin
            .replace(vec![registration(
                "/plugin",
                &["/shared"],
                ArgumentArity::ANY,
            )])
            .unwrap();

        assert_eq!(
            registry.resolve("/shared").unwrap().producer_id(),
            plugin.id()
        );
        assert!(plugin.remove());

        let resolved = registry.resolve("/shared").unwrap();
        assert_eq!(resolved.producer_id(), builtin.id());
        assert_eq!(
            registry
                .snapshot()
                .commands()
                .iter()
                .map(|command| command.invoked_name())
                .collect::<Vec<_>>(),
            ["/shared"]
        );
    }

    #[test]
    fn owned_resolution_executes_after_replacement() {
        let registry = super::CommandRegistry::new();
        let producer = registry.create_producer(super::ProducerPrecedence::Builtin);
        producer
            .replace(vec![registration("/old", &[], ArgumentArity::ANY)])
            .unwrap();
        let resolved = registry.resolve("/old").unwrap();
        producer.replace(Vec::new()).unwrap();
        let invocation = CommandInvocation {
            command_id: resolved.command_id(),
            canonical_name: Arc::clone(&resolved.spec().name),
            invoked_name: Arc::from("/old"),
            arguments: Arc::from(""),
            depth: 0,
            target_id: registry.create_target(),
            dispatcher: super::InvocationDispatcher::new(Arc::new(registry)),
            lifecycle: super::classification_channel().0,
        };
        futures_lite::future::block_on(resolved.behavior().execute(invocation)).unwrap();
    }

    #[test]
    fn removed_command_is_not_resolved_again() {
        let registry = super::CommandRegistry::new();
        let producer = registry.create_producer(super::ProducerPrecedence::Builtin);
        producer
            .replace(vec![registration("/gone", &[], ArgumentArity::ANY)])
            .unwrap();
        assert!(producer.remove());
        assert!(registry.resolve("/gone").is_err());
        assert!(!producer.remove());
    }

    #[test]
    fn input_parsing_preserves_remainder_and_validates_arity() {
        struct Capture(Arc<std::sync::Mutex<Option<Arc<str>>>>);
        impl CommandBehavior for Capture {
            fn execute(
                &self,
                invocation: CommandInvocation,
            ) -> CommandFuture<Result<(), super::CommandError>> {
                *self.0.lock().unwrap() = Some(invocation.arguments);
                invocation
                    .lifecycle
                    .transition(super::CommandClassification::Completed);
                Box::pin(async { Ok(()) })
            }
        }
        let captured = Arc::new(std::sync::Mutex::new(None));
        let registry = super::CommandRegistry::new();
        let producer = registry.create_producer(super::ProducerPrecedence::Builtin);
        let mut command = registration("/run", &[], ArgumentArity::ONE);
        command.behavior = Arc::new(Capture(Arc::clone(&captured)));
        producer.replace(vec![command]).unwrap();
        let target = registry.create_target();

        let dispatched =
            futures_lite::future::block_on(registry.dispatch_input("  /RUN   value  ", 0, target))
                .unwrap();
        assert!(matches!(dispatched, super::InputDispatch::Dispatched(_)));
        assert_eq!(captured.lock().unwrap().as_deref(), Some("value"));
        let error =
            futures_lite::future::block_on(registry.dispatch_input("/run one two", 0, target))
                .unwrap_err();
        assert!(matches!(
            error,
            super::CommandError::InvalidArguments { actual: 2, .. }
        ));
    }

    #[test]
    fn unknown_and_non_command_inputs_are_distinct() {
        let registry = super::CommandRegistry::new();
        let target = registry.create_target();
        assert!(matches!(
            futures_lite::future::block_on(registry.dispatch_input("hello", 0, target)).unwrap(),
            super::InputDispatch::NotCommand
        ));
        assert!(matches!(
            futures_lite::future::block_on(registry.dispatch_input("/unknown text", 0, target))
                .unwrap(),
            super::InputDispatch::UnknownCommandInput
        ));
    }

    #[test]
    fn depth_limit_is_enforced_for_known_commands() {
        let registry = super::CommandRegistry::new();
        let producer = registry.create_producer(super::ProducerPrecedence::Builtin);
        producer
            .replace(vec![registration("/run", &[], ArgumentArity::ANY)])
            .unwrap();
        let target = registry.create_target();
        let error = futures_lite::future::block_on(registry.dispatch_input(
            "/run",
            super::MAX_COMMAND_DEPTH + 1,
            target,
        ))
        .unwrap_err();
        assert_eq!(error, super::CommandError::MaximumDepth);
    }
}
