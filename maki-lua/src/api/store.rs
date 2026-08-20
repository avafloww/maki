use std::collections::HashMap;
use std::sync::Arc;

use maki_lua_macro::{lua_fn, lua_table};
use mlua::{Lua, Result as LuaResult, Table, Value};

use crate::api::autocmd;

/// Autocmd event fired on every store change. `data` carries `registry` and,
/// for registrations, `key`.
pub(crate) const STORE_CHANGED: &str = "StoreChanged";

struct Entry {
    owner: Arc<str>,
    value: Value,
}

#[derive(Default)]
pub(crate) struct Store {
    entries: HashMap<String, HashMap<String, Entry>>,
}

impl Store {
    fn register(
        &mut self,
        registry: String,
        key: String,
        owner: Arc<str>,
        value: Value,
    ) -> LuaResult<()> {
        let entries = self.entries.entry(registry.clone()).or_default();
        if let Some(existing) = entries.get(&key)
            && existing.owner != owner
        {
            return Err(mlua::Error::runtime(format!(
                "store entry '{registry}.{key}' is already registered by plugin '{}'",
                existing.owner
            )));
        }
        entries.insert(key, Entry { owner, value });
        Ok(())
    }

    /// Remove every entry owned by a plugin and return the registries that
    /// lost at least one entry.
    pub(crate) fn clear_plugin(&mut self, plugin: &str) -> Vec<String> {
        let mut changed = Vec::new();
        for (name, entries) in &mut self.entries {
            let previous_len = entries.len();
            entries.retain(|_, entry| entry.owner.as_ref() != plugin);
            if entries.len() != previous_len {
                changed.push(name.clone());
            }
        }
        self.entries.retain(|_, entries| !entries.is_empty());
        changed
    }
}

/// Register a value in a store registry under a stable key. The same plugin
/// may replace its own value; a different plugin cannot claim an existing
/// key. Entries are removed automatically when their plugin unloads or
/// reloads.
///
/// The change fires the `StoreChanged` autocmd event with
/// `data = { registry, key }`, so consumers collect the initial state at
/// load time and then stay fresh from events instead of polling.
///
/// @param registry string Registry name, e.g. `"splash"`.
/// @param key string Stable entry identifier.
/// @param value any Value exposed to registry consumers; tables may contain functions.
/// @return
#[lua_fn]
fn register(
    lua: &Lua,
    #[ctx] plugin: Arc<str>,
    registry: String,
    key: String,
    value: Value,
) -> LuaResult<()> {
    let mut store = lua
        .app_data_mut::<Store>()
        .ok_or_else(|| mlua::Error::runtime("store not initialized"))?;
    store.register(registry.clone(), key.clone(), plugin, value)?;
    drop(store);
    let data = lua.create_table()?;
    data.set("registry", registry)?;
    data.set("key", key)?;
    autocmd::dispatch(lua, STORE_CHANGED, None, Value::Table(data));
    Ok(())
}

/// Collect the current entries of a store registry. Returns a fresh table
/// keyed by each entry's stable identifier; an unknown registry yields an
/// empty table.
///
/// @param registry string Registry name.
/// @return table Map of entry key to registered value.
#[lua_fn]
fn collect(lua: &Lua, registry: String) -> LuaResult<Table> {
    let result = lua.create_table()?;
    if let Some(store) = lua.app_data_ref::<Store>()
        && let Some(entries) = store.entries.get(&registry)
    {
        for (key, entry) in entries {
            result.set(key.as_str(), entry.value.clone())?;
        }
    }
    Ok(result)
}

lua_table! {
    /// Shared key-value store for plugin contributions. A plugin registers
    /// values under a (registry, key) pair: the same plugin may replace its
    /// own value, a different plugin cannot claim an existing key, and
    /// entries are removed automatically when their owning plugin unloads or
    /// reloads.
    ///
    /// Every change fires the `StoreChanged` autocmd event: `data =
    /// { registry, key }` on register, `data = { registry }` when an owner
    /// unloads (one event per touched registry, no ordering guarantee).
    /// Gather the initial state at load time and keep it fresh from events;
    /// do not poll.
    "maki.store" => pub(crate) fn create_store_table(plugin: Arc<str>), DOCS [
        register(plugin), collect,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLLISION: &str = "already registered by plugin 'a'";

    #[test]
    fn ownership_controls_replace_and_clear() {
        let mut store = Store::default();
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
        assert!(error.to_string().contains(COLLISION));
        assert!(matches!(
            store.entries["kind"]["key"].value,
            Value::Integer(2)
        ));

        assert!(store.clear_plugin("b").is_empty());
        assert_eq!(store.clear_plugin("a"), vec!["kind".to_owned()]);
        assert!(store.entries.is_empty());
    }
}
