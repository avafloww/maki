//! Fuzzy matching backed by nucleo, the same matcher maki's built-in
//! pickers use. Plugins get consistent type-ahead search without
//! re-implementing subsequence logic.

use maki_lua_macro::{lua_fn, lua_table};
use mlua::{Lua, Result as LuaResult, Table, Value as LuaValue};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// nucleo fuzzy match of {query} against {text}.
///
/// Returns the nucleo score (higher is better) and the 1-based codepoint
/// indices of the matched characters, ascending and deduplicated. Every
/// whitespace-separated query word must match somewhere in the text (order
/// does not matter). An empty or whitespace-only query matches with score 0
/// and no indices.
pub(crate) fn fuzzy_match(query: &str, text: &str) -> Option<(u32, Vec<u32>)> {
    let query = query.trim();
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut chars = Vec::new();
    // codepoint haystack, not Utf32Str::new's grapheme segmentation: the
    // returned indices are codepoint positions (consumers map them to bytes
    // via utf8.offset), and graphemes would shift them for emoji/markers
    let haystack = if text.is_ascii() {
        Utf32Str::Ascii(text.as_bytes())
    } else {
        chars.extend(text.chars());
        Utf32Str::Unicode(&chars)
    };
    let mut indices = Vec::new();
    let score = pattern.indices(haystack, &mut matcher, &mut indices)?;
    indices.sort_unstable();
    indices.dedup();
    let indices = indices.into_iter().map(|index| index + 1).collect();
    Some((score, indices))
}

fn string_arg(val: &LuaValue, what: &str) -> LuaResult<String> {
    match val {
        LuaValue::String(s) => String::from_utf8(s.as_bytes().to_vec())
            .map_err(|_| mlua::Error::runtime(format!("{what}: expected valid UTF-8 string"))),
        _ => Err(mlua::Error::runtime(format!(
            "{what}: expected string, got {}",
            val.type_name()
        ))),
    }
}

/// Fuzzy match {query} against {text} with nucleo, the same matcher maki's
/// built-in pickers use. Every whitespace-separated word in {query} must
/// match somewhere in {text}; word order does not matter. An empty or
/// whitespace-only query matches everything.
///
/// @param query string Search words, whitespace separated.
/// @param text string Text to search in.
/// @return (table|nil) nil when no word matches. On a match: {score = number, indices = {…}} where indices are the 1-based codepoint offsets of the matched characters, ascending.
/// @example
/// local m = maki.match.fuzzy("gh pr", "gh pr 441 review")
/// if m then
///   print(m.score) -- matched codepoint offsets in m.indices
/// end
#[lua_fn]
fn fuzzy(lua: &Lua, query: LuaValue, text: LuaValue) -> LuaResult<Option<Table>> {
    let query = string_arg(&query, "match.fuzzy: query")?;
    let text = string_arg(&text, "match.fuzzy: text")?;
    let Some((score, indices)) = fuzzy_match(&query, &text) else {
        return Ok(None);
    };
    let t = lua.create_table()?;
    t.set("score", score)?;
    t.set("indices", indices)?;
    Ok(Some(t))
}

lua_table! {
    /// Fuzzy matching via nucleo, the same matcher maki's built-in pickers use.
    ///
    /// Use it for type-ahead search over a plugin's own item list.
    ///
    /// ```lua
    /// local m = maki.match.fuzzy("gh pr", "gh pr 441 review")
    /// if m then
    ///   print(m.score) -- matched codepoint offsets in m.indices
    /// end
    /// ```
    "maki.match" => pub(crate) fn create_match_table(), DOCS [
        fuzzy,
    ]
}

#[cfg(test)]
mod tests {
    use super::fuzzy_match;
    use test_case::test_case;

    #[test]
    fn non_string_args_error_names_the_arg() {
        let lua = mlua::Lua::new();
        let t = super::create_match_table(&lua).unwrap();
        let fuzzy: mlua::Function = t.get("fuzzy").unwrap();
        lua.globals().set("f", &fuzzy).unwrap();
        let msg: String = lua
            .load(
                r#"local ok, err = pcall(f, 123, "hello")
            assert(not ok)
            return tostring(err)"#,
            )
            .eval()
            .unwrap();
        assert!(msg.contains("match.fuzzy: query"), "{msg}");
        let msg: String = lua
            .load(
                r#"local ok, err = pcall(f, "hello", {})
            assert(not ok)
            return tostring(err)"#,
            )
            .eval()
            .unwrap();
        assert!(msg.contains("match.fuzzy: text"), "{msg}");
    }

    #[test_case("", "hello world" ; "empty_query")]
    #[test_case("   ", "hello world" ; "whitespace_only_query")]
    fn empty_query_matches_with_zero_score(query: &str, text: &str) {
        assert_eq!(fuzzy_match(query, text), Some((0, Vec::new())));
    }

    #[test_case("xyz", "hello world" ; "missing_term")]
    #[test_case("hello xyz", "hello world" ; "one_missing_term_fails_and")]
    fn no_match_returns_none(query: &str, text: &str) {
        assert_eq!(fuzzy_match(query, text), None);
    }

    #[test]
    fn indices_are_1_based_codepoints() {
        let (_, indices) = fuzzy_match("ap", "apple").unwrap();
        assert_eq!(indices, vec![1, 2]);
    }

    #[test]
    fn multi_term_order_is_independent() {
        assert!(fuzzy_match("441 review", "review gh pr 441").is_some());
    }

    #[test]
    fn smart_case_uppercase_query_is_case_sensitive() {
        assert_eq!(fuzzy_match("APPLE", "apple pie"), None);
    }

    #[test]
    fn smart_case_lowercase_query_is_case_insensitive() {
        assert!(fuzzy_match("apple", "Apple pie").is_some());
    }

    #[test]
    fn cjk_query_yields_codepoint_indices() {
        let (_, indices) = fuzzy_match("好世", "你好世界").unwrap();
        assert_eq!(indices, vec![2, 3]);
    }

    #[test]
    fn indices_are_codepoints_not_graphemes() {
        // "👍🏽" is two codepoints, one grapheme: "a" is codepoint 3, grapheme 2.
        let (_, indices) = fuzzy_match("a", "👍🏽abc").unwrap();
        assert_eq!(indices, vec![3]);
        // "🇺🇸" is two regional-indicator codepoints, one grapheme.
        let (_, indices) = fuzzy_match("b", "🇺🇸ab").unwrap();
        assert_eq!(indices, vec![4]);
    }

    #[test]
    fn contiguous_match_scores_higher_than_scattered() {
        let (tight, _) = fuzzy_match("app", "apple").unwrap();
        let (loose, _) = fuzzy_match("app", "axpyp").unwrap();
        assert!(tight > loose);
    }

    #[test]
    fn repeated_terms_dedup_indices() {
        let (_, indices) = fuzzy_match("a a", "banana").unwrap();
        assert_eq!(indices, vec![2]);
    }
}
