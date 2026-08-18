mod api;
pub mod docs;
pub mod docs_render;
mod error;
pub mod language;
mod loader;
pub(crate) mod plugin_permissions;
mod runtime;
mod splash;

pub use api::keymap::{KeymapEntry, KeymapReader, KeymapSnapshot};
pub use api::options::{OptionSpec, OptionType, PluginOptionSpecs};
pub use api::util::command::{
    Anchor, Axis, Border, BuiltinAction, Dimension, Edge, FloatConfig, FloatConfigPatch,
    HintReader, HintSnapshot, LuaCommandInfo, LuaCommandReader, SessionReply, SessionRequest,
    Split, TitlePos, UiAction, WinCommand, WinEvent, WinView,
};
pub use docs::{DocKind, FnDoc, ModuleDoc, ParamDoc, api_docs};
pub use error::PluginError;
pub use loader::{EventHandle, PluginHost, TestCompletionBackend};
pub use plugin_permissions::{Permission, PluginPermissions};
pub use runtime::{KILL_GRACE, RestoreItem, WARM_TOOL_CAP};
pub use splash::{
    SPLASH_PULL_TIMEOUT, SplashFrame, SplashPull, SplashRow, SplashStyle, VersionInfo,
};

pub use api::completion::{AtToken, CompletionCtx, ItemSpec, at_is_token_start, parse_at_tokens};

pub mod test_support {
    use crate::api::keymap::{KeymapEntry, KeymapWriter};
    use crate::api::util::command::{
        HintEntries, HintReader, HintWriter, LuaCommandInfo, LuaCommandReader, LuaCommandWriter,
    };
    use crate::{EventHandle, KeymapReader, PluginHost, TestCompletionBackend};

    pub struct LuaCommandWriterHandle(LuaCommandWriter);

    impl LuaCommandWriterHandle {
        pub fn publish(&self, commands: Vec<LuaCommandInfo>) {
            self.0.publish(commands);
        }
    }

    pub fn lua_command_writer_pair() -> (LuaCommandWriterHandle, LuaCommandReader) {
        let (writer, reader) = LuaCommandWriter::new();
        (LuaCommandWriterHandle(writer), reader)
    }

    /// Stands in for the Lua thread publishing a plugin's status hints.
    pub struct HintWriterHandle(HintWriter);

    impl HintWriterHandle {
        pub fn publish(&self, entries: HintEntries) {
            self.0.publish(entries);
        }
    }

    pub fn hint_writer_pair() -> (HintWriterHandle, HintReader) {
        let (writer, reader) = HintWriter::new();
        (HintWriterHandle(writer), reader)
    }

    /// Observes which requests an [`crate::EventHandle`] sends, without a
    /// running plugin host.
    pub struct RequestProbe(flume::Receiver<crate::runtime::Request>);

    impl RequestProbe {
        /// Next request as `(kind, clicks)`: `"click"` carries no clicks,
        /// `"click_fallback"` and `"restore"` carry their restore item's.
        pub fn try_recv(&self) -> Option<(&'static str, Vec<usize>)> {
            use crate::runtime::Request;
            Some(match self.0.try_recv().ok()? {
                Request::ClickTool { fallback: None, .. } => ("click", Vec::new()),
                Request::ClickTool {
                    fallback: Some(fb), ..
                } => ("click_fallback", fb.item.clicks),
                Request::RestoreToolAsync { item, .. } => ("restore", item.clicks),
                _ => ("other", Vec::new()),
            })
        }

        /// Next dispatched slash command as `(command, args, depth)`, skipping
        /// other requests.
        pub fn try_recv_command(&self) -> Option<(String, String, u8)> {
            use crate::runtime::Request;
            while let Ok(req) = self.0.try_recv() {
                if let Request::RunCommand {
                    command,
                    args,
                    depth,
                    ..
                } = req
                {
                    return Some((command.to_string(), args, depth));
                }
            }
            None
        }

        /// Next fired autocmd as `(event, data)`, skipping other requests.
        pub fn try_recv_autocmd(&self) -> Option<(String, serde_json::Value)> {
            use crate::runtime::Request;
            while let Ok(req) = self.0.try_recv() {
                if let Request::FireAutocmd { event, data } = req {
                    return Some((event, data));
                }
            }
            None
        }
    }

    pub fn probed_event_handle() -> (crate::EventHandle, RequestProbe) {
        let (tx, rx) = flume::unbounded();
        (crate::EventHandle::probed_for_test(tx), RequestProbe(rx))
    }

    /// Boots a real `PluginHost` (background Lua thread) pre-loading the given
    /// builtins, returning a live `EventHandle` plus a guard that keeps the
    /// host alive until it drops. Real-host tests use this instead of the
    /// disconnected/probed handles so end-to-end Lua behavior (e.g. pulling a
    /// `splash.render` frame) can be exercised.
    pub fn spawn_host_for_tests(plugins: &[&str]) -> (crate::EventHandle, PluginHostGuard) {
        use maki_config::PluginsConfig;
        use std::collections::HashMap;
        use std::sync::Arc;
        let reg: Arc<maki_agent::tools::ToolRegistry> =
            Arc::new(maki_agent::tools::ToolRegistry::new());
        let mut host = crate::PluginHost::new(Arc::clone(&reg)).unwrap();
        let cfg = PluginsConfig {
            enabled: true,
            names: plugins.iter().map(|s| s.to_string()).collect(),
            opts: HashMap::new(),
        };
        host.load_builtins(&cfg).unwrap();
        let handle = host.event_handle();
        (handle, PluginHostGuard { host })
    }

    /// Keeps a booted [`PluginHost`] alive (its `Drop` joins the Lua thread).
    pub struct PluginHostGuard {
        host: PluginHost,
    }

    impl PluginHostGuard {
        pub fn host(&self) -> &PluginHost {
            &self.host
        }
    }

    impl Drop for PluginHostGuard {
        fn drop(&mut self) {}
    }

    pub fn keymap_reader_with(entries: Vec<KeymapEntry>) -> KeymapReader {
        let (writer, reader) = KeymapWriter::new();
        writer.publish(entries);
        reader
    }

    /// An `EventHandle` backed by an in-memory completion/expander store, plus
    /// the store handle so tests can seed sources and expanders. Use this for
    /// `@`-completion and submit-expansion tests that run without a plugin host.
    pub fn event_handle_with_completion() -> (EventHandle, std::sync::Arc<TestCompletionBackend>) {
        let backend = std::sync::Arc::new(TestCompletionBackend::new());
        let handle = EventHandle::with_completion_for_test(std::sync::Arc::clone(&backend));
        (handle, backend)
    }
}
