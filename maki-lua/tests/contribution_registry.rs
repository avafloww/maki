use std::sync::Arc;

use maki_agent::tools::ToolRegistry;
use maki_lua::PluginHost;

fn host() -> PluginHost {
    PluginHost::new(Arc::new(ToolRegistry::new())).unwrap()
}

#[test]
fn same_owner_replaces_and_collect_returns_all_entries() {
    let host = host();
    host.load_source(
        "owner",
        r#"
        maki.api.register("renderers", "main", { version = 1 })
        maki.api.register("renderers", "other", "second")
        maki.api.register("renderers", "main", { version = 2 })
        local values = maki.api.collect("renderers")
        assert(values.main.version == 2)
        assert(values.other == "second")
        assert(next(maki.api.collect("missing")) == nil)
        "#,
    )
    .unwrap();
}

#[test]
fn foreign_owner_collision_fails_without_removing_original() {
    let host = host();
    host.load_source(
        "first",
        r#"maki.api.register("renderers", "main", "first")"#,
    )
    .unwrap();

    let error = host
        .load_source(
            "second",
            r#"maki.api.register("renderers", "main", "second")"#,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("contribution 'renderers.main' is already registered by plugin 'first'")
    );

    host.load_source(
        "probe",
        r#"assert(maki.api.collect("renderers").main == "first")"#,
    )
    .unwrap();
}

#[test]
fn contributions_are_cleared_on_unload_and_reload() {
    let host = host();
    host.load_source(
        "owner",
        r#"
        maki.api.register("renderers", "old", true)
        maki.api.register("other", "old", true)
        "#,
    )
    .unwrap();
    host.load_source(
        "owner",
        r#"
        assert(maki.api.collect("renderers").old == nil)
        assert(maki.api.collect("other").old == nil)
        maki.api.register("renderers", "new", true)
        "#,
    )
    .unwrap();
    host.unload("owner").unwrap();

    host.load_source(
        "probe",
        r#"assert(next(maki.api.collect("renderers")) == nil)"#,
    )
    .unwrap();
}

#[test]
fn failed_load_clears_contributions_registered_during_load() {
    let host = host();
    let error = host
        .load_source(
            "broken",
            r#"
            maki.api.register("renderers", "leaked", true)
            error("load failed")
            "#,
        )
        .unwrap_err();
    assert!(error.to_string().contains("load failed"));

    host.load_source(
        "probe",
        r#"assert(maki.api.collect("renderers").leaked == nil)"#,
    )
    .unwrap();
}
