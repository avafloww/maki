//! Wall-clock timestamps and relative-age formatting.
//!
//! `now()` returns seconds since the Unix epoch, the same clock the host and
//! storage layer use for session `updated_at`, so plugin timestamps can be
//! persisted and compared with host data. `ago()` renders a relative age the
//! same way maki's own pickers do, so plugins don't reimplement it.

use std::time::{SystemTime, UNIX_EPOCH};

use maki_lua_macro::{lua_fn, lua_table};
use mlua::{Lua, Result as LuaResult};

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_secs_f64()
}

/// Return the current wall-clock time as seconds since the Unix epoch
/// (`1970-01-01T00:00:00Z`), as a number.
///
/// This is the same clock the host uses for session `updated_at`, so the
/// value round-trips through storage and sort. It keeps sub-second precision
/// (unlike `os.time()`, which returns whole seconds), but stays compatible:
/// the whole part equals what `os.time()` returns. Pass the result to
/// `maki.time.ago` to render a relative age.
///
/// @return (number) Seconds since the Unix epoch, fractional.
/// @example
/// local t0 = maki.time.now()
/// -- ...work...
/// print(maki.time.ago(t0))
#[lua_fn]
fn now(_lua: &Lua) -> LuaResult<f64> {
    Ok(now_secs())
}

/// Format {instant} as a relative age like `3h ago`, or `just now` for less
/// than a minute. {instant} is a timestamp from `maki.time.now`.
///
/// Uses the same clock as `now`, so subtracting two `now` timestamps spans
/// real elapsed time (sub-second inclusive). A timestamp in the future (or a
/// non-number) renders as `just now`.
///
/// @param instant number Timestamp from `maki.time.now`.
/// @return (string) Relative age, e.g. `42min ago`.
/// @example
/// print(maki.time.ago(maki.time.now() - 30 * 60)) -- "30min ago"
#[lua_fn]
fn ago(_lua: &Lua, instant: f64) -> LuaResult<String> {
    let elapsed = (now_secs() - instant).max(0.0) as u64;
    Ok(format_ago(elapsed))
}

/// Shared relative-age formatter. Consumed by `maki.time.ago` for plugins
/// and by maki's own pickers (they feed it `Instant::elapsed().as_secs()`,
/// which is the same whole-second duration).
pub fn format_ago(secs: u64) -> String {
    if secs < 60 {
        return "just now".into();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}min ago");
    }
    let hrs = mins / 60;
    if hrs < 24 {
        return format!("{hrs}h ago");
    }
    format!("{}d ago", hrs / 24)
}

lua_table! {
    /// Wall-clock timestamps and relative-age formatting.
    ///
    /// `now` returns seconds since the Unix epoch, the same clock the host
    /// uses for session `updated_at`, so plugin timestamps can be persisted
    /// and sorted with host data. `ago` renders a relative age the same way
    /// maki's own pickers do, so callers get one consistent format instead
    /// of each plugin inventing its own.
    ///
    /// ```lua
    /// local t0 = maki.time.now()
    /// -- work...
    /// print(maki.time.ago(t0))
    /// ```
    "maki.time" => pub(crate) fn create_time_table(), DOCS [
        now, ago,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    const ONE_MIN: u64 = 60;
    const ONE_HOUR: u64 = 3600;
    const ONE_DAY: u64 = 86400;

    #[test_case(0 => "just now" ; "zero")]
    #[test_case(ONE_MIN - 1 => "just now" ; "under a minute")]
    #[test_case(ONE_MIN => "1min ago" ; "one minute")]
    #[test_case(5 * ONE_MIN => "5min ago" ; "five minutes")]
    #[test_case(ONE_HOUR - 1 => "59min ago" ; "just under an hour")]
    #[test_case(ONE_HOUR => "1h ago" ; "one hour")]
    #[test_case(2 * ONE_HOUR => "2h ago" ; "two hours")]
    #[test_case(ONE_DAY - 1 => "23h ago" ; "just under a day")]
    #[test_case(ONE_DAY => "1d ago" ; "one day")]
    #[test_case(3 * ONE_DAY => "3d ago" ; "three days")]
    #[test_case(30 * ONE_DAY => "30d ago" ; "a month of days")]
    fn formats_elapsed(secs: u64) -> String {
        format_ago(secs)
    }

    #[test]
    fn ago_clamps_future_timestamps() {
        let lua = Lua::new();
        let t = create_time_table(&lua).unwrap();
        let ago: mlua::Function = t.get("ago").unwrap();
        let far_future = now_secs() + ONE_DAY as f64;
        let out: String = ago.call(far_future).unwrap();
        assert_eq!(out, "just now");
    }

    #[test]
    fn now_is_wall_clock_and_monotonic() {
        let lua = Lua::new();
        let t = create_time_table(&lua).unwrap();
        let now: mlua::Function = t.get("now").unwrap();
        let a: f64 = now.call(()).unwrap();
        let b: f64 = now.call(()).unwrap();
        assert!(b >= a);
        assert!(a > 1_700_000_000.0, "should be contemporary epoch seconds");
    }
}
