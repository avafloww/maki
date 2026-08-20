use std::collections::HashMap;
use std::sync::Arc;

use maki_lua_macro::{lua_fn, lua_table};
use mlua::{Lua, Result as LuaResult, Table, Value};

struct Contribution {
    owner: Arc<str>,
    value: Value,
}

#[derive(Default)]
pub(crate) struct ContributionStore {
    registries: HashMap<String, HashMap<String, Contribution>>,
    revisions: HashMap<String, u64>,
}

impl ContributionStore {
    fn register(
        &mut self,
        name: String,
        key: String,
        owner: Arc<str>,
        value: Value,
    ) -> LuaResult<()> {
        let entries = self.registries.entry(name.clone()).or_default();
        if let Some(existing) = entries.get(&key)
            && existing.owner != owner
        {
            return Err(mlua::Error::runtime(format!(
                "contribution '{name}.{key}' is already registered by plugin '{}'",
                existing.owner
            )));
        }
        entries.insert(key, Contribution { owner, value });
        *self.revisions.entry(name).or_default() += 1;
        Ok(())
    }

    pub(crate) fn clear_plugin(&mut self, plugin: &str) {
        for (name, entries) in &mut self.registries {
            let previous_len = entries.len();
            entries.retain(|_, entry| entry.owner.as_ref() != plugin);
            if entries.len() != previous_len {
                *self.revisions.entry(name.clone()).or_default() += 1;
            }
        }
        self.registries.retain(|_, entries| !entries.is_empty());
    }
}

/// Register a plugin-owned value in a named contribution registry.
/// The same plugin may replace its value. A different plugin cannot claim an
/// existing key. Contributions are removed automatically when their plugin is
/// unloaded or reloaded.
///
/// @param name string Registry name, e.g. `"splash"`.
/// @param key string Stable contribution identifier.
/// @param value any Value exposed to registry consumers; tables may contain functions.
/// @return
#[lua_fn]
fn register(
    lua: &Lua,
    #[ctx] plugin: Arc<str>,
    name: String,
    key: String,
    value: Value,
) -> LuaResult<()> {
    let mut store = lua
        .app_data_mut::<ContributionStore>()
        .ok_or_else(|| mlua::Error::runtime("contribution store not initialized"))?;
    store.register(name, key, plugin, value)
}

/// Collect the current values in a named contribution registry.
/// Returns a fresh table keyed by each contribution's stable identifier.
///
/// @param name string Registry name.
/// @return table Map of contribution key to registered value.
#[lua_fn]
fn collect(lua: &Lua, name: String) -> LuaResult<Table> {
    let result = lua.create_table()?;
    if let Some(store) = lua.app_data_ref::<ContributionStore>()
        && let Some(entries) = store.registries.get(&name)
    {
        for (key, entry) in entries {
            result.set(key.as_str(), entry.value.clone())?;
        }
    }
    Ok(result)
}

/// Return a monotonically increasing revision for a contribution registry.
/// Consumers that cache contributed functions can compare this value before
/// use and collect again after a contributor is loaded, reloaded, or unloaded.
///
/// @param name string Registry name.
/// @return integer Registry revision.
#[lua_fn]
fn contribution_revision(lua: &Lua, name: String) -> LuaResult<u64> {
    Ok(lua
        .app_data_ref::<ContributionStore>()
        .and_then(|store| store.revisions.get(&name).copied())
        .unwrap_or_default())
}

lua_table! {
    extend "maki.api" => pub(crate) fn add_methods(plugin: Arc<str>), DOCS [
        register(plugin), collect, contribution_revision,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_controls_replace_and_clear() {
        let mut store = ContributionStore::default();
        store
            .register(
                "kind".into(),
                "key".into(),
                Arc::from("a"),
                Value::Integer(1),
            )
            .unwrap();
        store
            .register(
                "kind".into(),
                "key".into(),
                Arc::from("a"),
                Value::Integer(2),
            )
            .unwrap();

        let error = store
            .register(
                "kind".into(),
                "key".into(),
                Arc::from("b"),
                Value::Integer(3),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("already registered by plugin 'a'")
        );
        assert!(matches!(
            store.registries["kind"]["key"].value,
            Value::Integer(2)
        ));
        assert_eq!(store.revisions["kind"], 2);

        store.clear_plugin("b");
        assert!(store.registries.contains_key("kind"));
        assert_eq!(store.revisions["kind"], 2);
        store.clear_plugin("a");
        assert!(store.registries.is_empty());
        assert_eq!(store.revisions["kind"], 3);
    }
}
