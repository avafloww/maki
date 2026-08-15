//! Registry of agent modes: built-in `build` and `plan` plus plugin-defined
//! modes and full overrides of the built-ins. The UI, Lua host, CLI, SDK and
//! ACP all share one `Arc<ModeRegistry>`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::prompt::PLAN_PROMPT;

/// Identity of an agent mode. Custom modes are keyed by a snake_case name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModeId {
    Build,
    Plan,
    Custom(Arc<str>),
}

impl ModeId {
    /// Stable registry key: "build", "plan", or the custom name.
    pub fn key(&self) -> &str {
        match self {
            Self::Build => "build",
            Self::Plan => "plan",
            Self::Custom(name) => name,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "build" => Self::Build,
            "plan" => Self::Plan,
            name => Self::Custom(Arc::from(name)),
        }
    }
}

impl std::fmt::Display for ModeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key())
    }
}

/// A fully resolved mode definition. Label is the badge text; an override of
/// a built-in replaces the whole def.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeDef {
    pub id: ModeId,
    pub label: Arc<str>,
    /// System-prompt snippet appended verbatim (like `PLAN_PROMPT`). Vars are
    /// applied at prompt-build time.
    pub system_prompt: Option<String>,
    /// When set, all non-matching writes are blocked (plan-file-style lock).
    pub restrict_write_to: Option<PathBuf>,
    /// When `Some`, the mode swaps in exactly this toolset. `None` inherits
    /// the default (build) set.
    pub tools: Option<Vec<String>>,
}

impl ModeDef {
    pub fn default_for(id: ModeId) -> Self {
        let (label, system_prompt) = match &id {
            ModeId::Build => ("[BUILD]".into(), None),
            ModeId::Plan => ("[PLAN]".into(), Some(PLAN_PROMPT.to_owned())),
            ModeId::Custom(name) => {
                let upper = name.to_ascii_uppercase();
                (format!("[{upper}]").into(), None)
            }
        };
        Self {
            id,
            label,
            system_prompt,
            restrict_write_to: None,
            tools: None,
        }
    }
}

/// The user-facing shape from Lua/SDK: name plus optional fields to define or
/// fully override a mode.
#[derive(Debug, Clone, Default)]
pub struct ModeDefSpec {
    pub name: String,
    pub label: Option<Arc<str>>,
    pub system_prompt: Option<String>,
    pub restrict_write_to: Option<PathBuf>,
    pub tools: Option<Vec<String>>,
}

impl ModeDefSpec {
    /// Builds a def, filling defaults for any absent field. Name is validated
    /// (snake_case, non-empty, not a reserved conflict).
    pub fn build(self) -> Result<ModeDef, ModeError> {
        if self.name.is_empty() {
            return Err(ModeError::EmptyName);
        }
        let id = ModeId::parse(&self.name);
        let mut def = ModeDef::default_for(id);
        if let Some(label) = self.label {
            def.label = label;
        }
        def.system_prompt = self.system_prompt;
        def.restrict_write_to = self.restrict_write_to;
        def.tools = self.tools;
        Ok(def)
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ModeError {
    #[error("mode name must be non-empty")]
    EmptyName,
    #[error("unknown mode '{0}'")]
    Unknown(String),
}

/// Shared, thread-safe store of mode definitions.
#[derive(Debug, Default)]
pub struct ModeRegistry {
    modes: RwLock<HashMap<String, ModeDef>>,
}

impl ModeRegistry {
    /// Registers the built-in `build` and `plan` modes.
    pub fn builtin() -> Self {
        let mut map = HashMap::new();
        for id in [ModeId::Build, ModeId::Plan] {
            let key = id.key().to_owned();
            map.insert(key, ModeDef::default_for(id));
        }
        Self {
            modes: RwLock::new(map),
        }
    }

    /// Insert or replace a mode (override). `build`/`plan` are replaced; a new
    /// name defines a custom mode.
    pub fn define(&self, spec: ModeDefSpec) -> Result<(), ModeError> {
        let def = spec.build()?;
        self.modes
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(def.id.key().to_owned(), def);
        Ok(())
    }

    /// Restore a built-in's default def, dropping any plugin override.
    pub fn reset(&self, id: &ModeId) -> Result<(), ModeError> {
        let key = match id {
            ModeId::Build => "build".to_owned(),
            ModeId::Plan => "plan".to_owned(),
            ModeId::Custom(name) => name.to_string(),
        };
        let def = match id {
            ModeId::Build => ModeDef::default_for(ModeId::Build),
            ModeId::Plan => ModeDef::default_for(ModeId::Plan),
            ModeId::Custom(_) => return Err(ModeError::Unknown(key)),
        };
        self.modes
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, def);
        Ok(())
    }

    pub fn get(&self, id: &ModeId) -> Option<ModeDef> {
        self.modes
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id.key())
            .cloned()
    }

    /// Get by string key (used by SDK/ACP parsers).
    pub fn get_by_key(&self, key: &str) -> Option<ModeDef> {
        self.modes
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
    }

    /// Ordered list of all defined modes (built-ins first, then custom).
    pub fn list(&self) -> Vec<ModeDef> {
        let map = self.modes.read().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<ModeDef> = map.values().cloned().collect();
        out.sort_by_key(|d| match d.id {
            ModeId::Build => 0,
            ModeId::Plan => 1,
            ModeId::Custom(_) => 2,
        });
        out
    }

    /// Resolve the active mode's full def; falls back to `build` when a custom
    /// mode is not (re)defined.
    pub fn current(&self, active: &crate::AgentMode) -> ModeDef {
        match active {
            crate::AgentMode::Build => self
                .get(&ModeId::Build)
                .unwrap_or_else(|| ModeDef::default_for(ModeId::Build)),
            crate::AgentMode::Plan(_) => self
                .get(&ModeId::Plan)
                .unwrap_or_else(|| ModeDef::default_for(ModeId::Plan)),
            crate::AgentMode::Custom(id) => self
                .get(id)
                .unwrap_or_else(|| ModeDef::default_for(ModeId::Build)),
        }
    }

    /// Write-restriction for the active mode: the dynamic plan path wins for
    /// `Plan`; otherwise the def's `restrict_write_to`.
    pub fn restrict_write_to(&self, active: &crate::AgentMode) -> Option<PathBuf> {
        match active {
            crate::AgentMode::Build => None,
            crate::AgentMode::Plan(p) => Some(p.clone()),
            crate::AgentMode::Custom(id) => self.get(id).and_then(|d| d.restrict_write_to),
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.modes
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::AgentMode;

    fn reg() -> ModeRegistry {
        ModeRegistry::builtin()
    }

    #[test]
    fn builtin_registers_build_and_plan() {
        let r = reg();
        assert_eq!(r.list().len(), 2);
        assert!(r.contains("build"));
        assert!(r.contains("plan"));
    }

    #[test]
    fn builtin_plan_carries_plan_prompt() {
        let r = reg();
        let plan = r.get(&ModeId::Plan).unwrap();
        assert!(
            plan.system_prompt
                .as_deref()
                .unwrap_or("")
                .contains("Plan Mode")
        );
        assert_eq!(plan.label.as_ref(), "[PLAN]");
    }

    #[test]
    fn define_inserts_custom_mode() {
        let r = reg();
        r.define(ModeDefSpec {
            name: "audit".into(),
            label: Some("[AUDIT]".into()),
            system_prompt: Some("Audit rules".into()),
            ..Default::default()
        })
        .unwrap();
        let m = r.get(&ModeId::Custom("audit".into())).unwrap();
        assert_eq!(m.system_prompt.as_deref(), Some("Audit rules"));
        assert_eq!(m.label.as_ref(), "[AUDIT]");
    }

    #[test]
    fn define_overrides_builtin_plan() {
        let r = reg();
        r.define(ModeDefSpec {
            name: "plan".into(),
            system_prompt: Some("Override".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            r.get(&ModeId::Plan).unwrap().system_prompt.as_deref(),
            Some("Override")
        );
        assert_eq!(r.list().len(), 2);
    }

    #[test]
    fn reset_restores_builtin() {
        let r = reg();
        r.define(ModeDefSpec {
            name: "plan".into(),
            system_prompt: Some("Override".into()),
            ..Default::default()
        })
        .unwrap();
        r.reset(&ModeId::Plan).unwrap();
        assert!(
            r.get(&ModeId::Plan)
                .unwrap()
                .system_prompt
                .as_deref()
                .unwrap_or("")
                .contains("Plan Mode")
        );
    }

    #[test_case(&AgentMode::Build, None ; "build_no_restrict")]
    #[test_case(&AgentMode::Plan(PathBuf::from("plan.md")), Some("plan.md") ; "plan_restricts_to_path")]
    fn restrict_write_to_resolves(active: &AgentMode, expected: Option<&str>) {
        let r = reg();
        assert_eq!(
            r.restrict_write_to(active)
                .as_deref()
                .map(|p| p.to_string_lossy().into_owned()),
            expected.map(str::to_owned)
        );
    }

    #[test]
    fn custom_restrict_uses_def() {
        let r = reg();
        r.define(ModeDefSpec {
            name: "audit".into(),
            restrict_write_to: Some("audit.md".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            r.restrict_write_to(&AgentMode::Custom(ModeId::Custom("audit".into())))
                .as_deref()
                .map(|p| p.to_string_lossy().into_owned()),
            Some("audit.md".to_owned())
        );
    }

    #[test]
    fn current_falls_back_to_build_when_custom_missing() {
        let r = reg();
        let def = r.current(&AgentMode::Custom(ModeId::Custom("ghost".into())));
        assert_eq!(def.id, ModeId::Build);
    }

    #[test]
    fn empty_name_is_rejected() {
        assert_eq!(
            ModeDefSpec {
                name: String::new(),
                ..Default::default()
            }
            .build(),
            Err(ModeError::EmptyName)
        );
        let r = reg();
        assert!(
            r.define(ModeDefSpec {
                name: String::new(),
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn list_is_build_then_plan_then_custom() {
        let r = reg();
        r.define(ModeDefSpec {
            name: "zed".into(),
            ..Default::default()
        })
        .unwrap();
        let names: Vec<String> = r
            .list()
            .into_iter()
            .map(|d| d.id.key().to_owned())
            .collect();
        assert_eq!(
            names,
            ["build".to_owned(), "plan".to_owned(), "zed".to_owned()]
        );
    }
}
