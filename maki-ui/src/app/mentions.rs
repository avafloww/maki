//! `@`-reference parser and submit-time expander.
//!
//! The completion popup (`components::file_completion`) offers `@skill:`,
//! `@subagent:`, and `@model:` references as the user types. This module is
//! the other half: it scans a finished prompt for those references, strips
//! them, and rewrites the message into an explicit directive the agent acts on
//! (delegate to a subagent, load skills, switch the session model). File
//! `@path` references are left untouched for the agent to read lazily.

use std::ops::Range;

use maki_providers::Model;
use maki_providers::ModelTier;
use maki_providers::model_registry;

use crate::components::file_completion::at_is_token_start;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reference {
    Skill(String),
    Subagent(String),
    Model(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Expanded {
    pub message: String,
    pub model_switch: Option<String>,
}

const DIRECTIVE_OPEN: &str = "<user_references>";
const DIRECTIVE_CLOSE: &str = "</user_references>";
const TASK_TOOL: &str = "task";

/// Scans `text` for `@`-references at token boundaries, returning each with
/// its byte range. Unrecognized `@` tokens, and `@` not at a boundary, are
/// skipped so they pass through to the agent verbatim.
pub(crate) fn parse_references(text: &str) -> Vec<(Reference, Range<usize>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap();
        if c == '@' && at_is_token_start(text, i) {
            let start = i;
            let mut j = i + 1;
            while j < text.len() {
                let cc = text[j..].chars().next().unwrap();
                if cc.is_whitespace() {
                    break;
                }
                j += cc.len_utf8();
            }
            if let Some(reference) = classify(&text[start + 1..j]) {
                out.push((reference, start..j));
            }
            i = j;
        } else {
            i += c.len_utf8();
        }
    }
    out
}

fn classify(token: &str) -> Option<Reference> {
    let (prefix, value) = token.split_once(':')?;
    if value.is_empty() {
        return None;
    }
    match prefix.to_ascii_lowercase().as_str() {
        "skill" | "s" => Some(Reference::Skill(value.to_string())),
        "subagent" | "a" => Some(Reference::Subagent(value.to_string())),
        "model" | "m" => Some(Reference::Model(value.to_string())),
        _ => None,
    }
}

pub(crate) fn expand_references(text: &str) -> Expanded {
    let refs = parse_references(text);
    if refs.is_empty() {
        return Expanded {
            message: text.to_string(),
            model_switch: None,
        };
    }

    let mut skills: Vec<String> = Vec::new();
    let mut subagent: Option<String> = None;
    let mut model: Option<String> = None;
    for (reference, _) in &refs {
        match reference {
            Reference::Skill(s) => skills.push(s.clone()),
            Reference::Subagent(s) if subagent.is_none() => subagent = Some(s.clone()),
            Reference::Model(s) if model.is_none() => model = Some(s.clone()),
            _ => {}
        }
    }

    let stripped = strip_tokens(text, &refs);

    match subagent {
        Some(ty) => Expanded {
            message: subagent_directive(&ty, model.as_deref(), &skills, &stripped),
            model_switch: None,
        },
        None => {
            // A bare tier (`weak`/`medium`/`strong`) next to a subagent maps
            // to `model_tier`; standalone, `ChangeModel("weak")` would fail
            // `Model::from_spec` (it wants a `/`). Resolve it to the spec
            // assigned to that tier so `@model:weak` means the same model in
            // both contexts. An unmapped tier falls through to the raw spec,
            // surfacing a clear "Invalid model" flash instead of a silent swap.
            let model_switch = model.map(|s| resolve_standalone_spec(&s));
            let mut message = String::new();
            if !skills.is_empty() {
                message.push_str(DIRECTIVE_OPEN);
                message.push_str("\nBefore answering, load these skills via the `skill` tool: ");
                message.push_str(&skills.join(", "));
                message.push_str(".\n");
                message.push_str(DIRECTIVE_CLOSE);
                message.push('\n');
            }
            message.push_str(&stripped);
            Expanded {
                message,
                model_switch,
            }
        }
    }
}

/// Removes each reference token plus one adjacent ASCII space/tab so the
/// surrounding words are not left with a gap, then trims the result.
fn strip_tokens(text: &str, refs: &[(Reference, Range<usize>)]) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut last_end = 0;
    for (_, range) in refs {
        kept.push_str(&text[last_end..range.start]);
        last_end = range.end;
        if let Some(c) = text.get(last_end..).and_then(|s| s.chars().next())
            && (c == ' ' || c == '\t')
        {
            last_end += c.len_utf8();
        }
    }
    kept.push_str(&text[last_end..]);
    kept.trim().to_string()
}

enum ModelRequest {
    /// Literal tier name → `model_tier`, the clamped tier path (no `allow_model` needed).
    Tier(String),
    /// Real spec → exact model. `fallback_tier` is the spec's user-override tier
    /// (from `/model`) if set, else its manifest/discovery tier via `Model::from_spec`,
    /// offered as a one-click fallback in the question. `None` when neither resolves
    /// (Compaction is never offered as a fallback tier).
    Exact { spec: String, fallback_tier: Option<String> },
}

fn model_clause(spec: &str) -> ModelRequest {
    match spec {
        "weak" | "medium" | "strong" => ModelRequest::Tier(spec.to_string()),
        _ => {
            let fallback = model_registry::override_tiers(spec)
                .into_iter()
                .find(|t| !matches!(t, ModelTier::Compaction))
                .or_else(|| {
                    Model::from_spec(spec)
                        .ok()
                        .map(|m| m.tier)
                        .filter(|t| !matches!(t, ModelTier::Compaction))
                });
            ModelRequest::Exact {
                spec: spec.to_string(),
                fallback_tier: fallback.map(|t| t.to_string()),
            }
        }
    }
}

/// Maps a standalone `@model:` spec to what `Action::ChangeModel` accepts: a
/// bare tier resolves to the spec currently assigned to it, anything else
/// passes through (a full spec, or an unmapped tier that fails loudly).
fn resolve_standalone_spec(spec: &str) -> String {
    let tier = match spec {
        "weak" => ModelTier::Weak,
        "medium" => ModelTier::Medium,
        "strong" => ModelTier::Strong,
        _ => return spec.to_string(),
    };
    model_registry::spec_for_tier_any(tier).unwrap_or_else(|| spec.to_string())
}

fn subagent_directive(ty: &str, model: Option<&str>, skills: &[String], body: &str) -> String {
    let mut s = String::new();
    s.push_str(DIRECTIVE_OPEN);
    s.push_str("\nDelegate the user's request to a subagent by calling the `");
    s.push_str(TASK_TOOL);
    s.push_str("` tool with subagent_type \"");
    s.push_str(ty);
    s.push('"');
    if let Some(spec) = model {
        match model_clause(spec) {
            ModelRequest::Tier(t) => {
                s.push_str(", model_tier \"");
                s.push_str(&t);
                s.push('"');
            }
            ModelRequest::Exact { spec, fallback_tier } => {
                s.push_str(". The user requested the exact model \"");
                s.push_str(&spec);
                s.push_str("\" for this subagent. The `task` tool accepts a `model` parameter only when its `allow_model` option is enabled: if the tool's input schema lists a `model` property, spawn with model \"");
                s.push_str(&spec);
                s.push_str("\". If it does not, `allow_model` is off and an exact model cannot be set. Do not silently substitute another model. Instead, call the `question` tool to tell the user that exact subagent models require enabling `allow_model` in the config, and ask how to proceed. Offer the options ");
                match fallback_tier {
                    Some(t) => {
                        s.push_str("\"Use the ");
                        s.push_str(&t);
                        s.push_str(" tier (Recommended)\", \"Use the current model\", \"Cancel\"");
                    }
                    None => {
                        s.push_str("\"Use the current model (Recommended)\", \"Cancel\"");
                    }
                }
            }
        }
    }
    s.push('.');
    if !skills.is_empty() {
        s.push_str(" In the subagent prompt, instruct it to first load these skills via the `skill` tool: ");
        s.push_str(&skills.join(", "));
        s.push('.');
    }
    s.push_str("\nThe subagent's task:\n");
    s.push_str(body);
    s.push_str("\nRelay the subagent's result back to the user.\n");
    s.push_str(DIRECTIVE_CLOSE);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use maki_storage::StateDir;
    use tempfile::TempDir;
    use test_case::test_case;

    fn parse_kinds(text: &str) -> Vec<Reference> {
        parse_references(text).into_iter().map(|(r, _)| r).collect()
    }

    #[test_case("@skill:review", vec![Reference::Skill("review".into())] ; "skill_long")]
    #[test_case("@s:review", vec![Reference::Skill("review".into())] ; "skill_short")]
    #[test_case("@subagent:research", vec![Reference::Subagent("research".into())] ; "subagent_long")]
    #[test_case("@a:general", vec![Reference::Subagent("general".into())] ; "subagent_short")]
    #[test_case("@model:weak", vec![Reference::Model("weak".into())] ; "model_long")]
    #[test_case("@m:zai/glm-5", vec![Reference::Model("zai/glm-5".into())] ; "model_short_with_slash")]
    #[test_case(
        "@skill:a @subagent:b @model:c",
        vec![
            Reference::Skill("a".into()),
            Reference::Subagent("b".into()),
            Reference::Model("c".into()),
        ]
        ; "mixed_in_order"
    )]
    #[test_case("@SKILL:Review", vec![Reference::Skill("Review".into())] ; "prefix_case_insensitive")]
    #[test_case("@nothing:whatever", vec![] ; "unknown_prefix")]
    #[test_case("@skill:", vec![] ; "empty_value_not_a_reference")]
    #[test_case("foo@bar", vec![] ; "mid_word_at_rejected")]
    #[test_case("me@foo.com", vec![] ; "email_rejected")]
    #[test_case("hello @skill:rev world", vec![Reference::Skill("rev".into())] ; "embedded_in_text")]
    fn parse_cases(text: &str, expected: Vec<Reference>) {
        assert_eq!(parse_kinds(text), expected);
    }

    #[test]
    fn parse_ranges_are_byte_offsets_of_the_token() {
        let refs = parse_references("fix @skill:pdf now");
        assert_eq!(refs.len(), 1);
        let (r, range) = &refs[0];
        assert_eq!(r, &Reference::Skill("pdf".into()));
        assert_eq!(&"fix @skill:pdf now"[range.clone()], "@skill:pdf");
    }

    #[test]
    fn ac5_subagent_directive() {
        let e = expand_references("@subagent:research review this package");
        assert!(!e.message.contains("@subagent:research"));
        assert!(e.message.contains("`task`"));
        assert!(e.message.contains("subagent_type \"research\""));
        assert!(e.message.contains("review this package"));
        assert!(e.model_switch.is_none());
    }

    #[test]
    fn ac6_subagent_model_skill_directive() {
        let e = expand_references("@subagent:general @m:weak @skill:pdf fix the report");
        assert!(e.message.contains("subagent_type \"general\""));
        assert!(e.message.contains("model_tier \"weak\""));
        assert!(e.message.contains("pdf"));
        assert!(e.message.contains("fix the report"));
        assert!(!e.message.contains("@m:weak"));
        assert!(!e.message.contains("@skill:pdf"));
        assert!(e.model_switch.is_none());
    }

    #[test]
    fn ac7_standalone_model_switch_strips_token() {
        let e = expand_references("@model:zai/glm-5 fix the bug");
        assert_eq!(e.model_switch.as_deref(), Some("zai/glm-5"));
        assert!(!e.message.contains("@model:zai/glm-5"));
        assert!(e.message.contains("fix the bug"));
    }

    #[test]
    fn ac8_unrecognized_tokens_passthrough() {
        let text = "foo@bar @nothing:whatever fix it";
        let e = expand_references(text);
        assert_eq!(e.message, text);
        assert!(e.model_switch.is_none());
    }

    #[test]
    fn skills_only_directive_lists_skills() {
        let e = expand_references("@skill:pdf @skill:csv summarize");
        assert!(e.message.contains("`skill` tool"));
        assert!(e.message.contains("pdf, csv"));
        assert!(e.message.contains("summarize"));
        assert!(!e.message.contains("@skill:"));
        assert!(e.model_switch.is_none());
    }

    #[test]
    fn subagent_with_literal_tier_uses_model_tier() {
        let e = expand_references("@subagent:research @m:strong do x");
        assert!(e.message.contains("model_tier \"strong\""));
        assert!(!e.message.contains("allow_model"));
    }

    #[test]
    fn subagent_with_exact_spec_includes_allow_model_guidance() {
        let e = expand_references("@subagent:research @m:nonexistent/fake do x");
        assert!(e.message.contains("nonexistent/fake"));
        assert!(e.message.contains("allow_model"));
        assert!(e.message.contains("`question`"));
        assert!(!e.message.contains("model_tier"));
        assert!(e.message.contains("Use the current model (Recommended)"));
        assert!(e.message.contains("Cancel"));
    }

    #[test]
    fn subagent_with_resolvable_exact_spec_offers_fallback_tier() {
        let spec = "anthropic/claude-sonnet-4-5";
        let tier = Model::from_spec(spec).unwrap().tier.to_string();
        let e = expand_references(&format!("@subagent:research @model:{spec} do x"));
        assert!(e.message.contains(spec));
        assert!(e.message.contains("allow_model"));
        assert!(e.message.contains("`question`"));
        assert!(e.message.contains(&format!("Use the {tier} tier")));
    }

    #[test]
    fn subagent_with_overridden_tier_offers_override_as_fallback() {
        // `nonexistent/overridden` does not resolve via `Model::from_spec`, so the
        // only source of a fallback tier is the `override_tiers` check in
        // `model_clause`. Dropping that check would leave `fallback_tier` None
        // and no tier would be offered — this test pins the override path.
        let spec = "nonexistent/overridden";
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());
        model_registry::set_and_persist(spec.into(), ModelTier::Medium, &dir);
        let e = expand_references(&format!("@subagent:research @model:{spec} do x"));
        model_registry::unset_and_persist(spec, ModelTier::Medium, &dir);

        assert!(e.message.contains(spec));
        assert!(e.message.contains("allow_model"));
        assert!(e.message.contains("`question`"));
        assert!(e.message.contains("Use the medium tier (Recommended)"));
        assert!(!e.message.contains("model_tier"));
    }

    #[test]
    fn multiple_subagent_refs_use_first() {
        let e = expand_references("@subagent:research @subagent:general do x");
        assert!(e.message.contains("subagent_type \"research\""));
        assert!(!e.message.contains("subagent_type \"general\""));
    }

    #[test]
    fn no_references_returns_text_unchanged() {
        let e = expand_references("just a plain prompt");
        assert_eq!(e.message, "just a plain prompt");
        assert!(e.model_switch.is_none());
    }

    #[test]
    fn model_switch_and_skills_compose_without_subagent() {
        let e = expand_references("@model:weak @skill:pdf explain");
        assert_eq!(e.model_switch.as_deref(), Some("weak"));
        assert!(e.message.contains("`skill` tool"));
        assert!(e.message.contains("pdf"));
        assert!(e.message.contains("explain"));
        assert!(!e.message.contains("@model:weak"));
    }

    #[test]
    fn standalone_bare_tier_resolves_to_assigned_spec_when_mapped() {
        // `weak` is only useful if a model is assigned to the weak tier; without
        // an override the registry returns None and we fall back to the raw
        // tier so the failure surfaces loudly rather than silently swapping.
        let e = expand_references("@model:weak fix it");
        assert!(matches!(e.model_switch.as_deref(), Some("weak")));
    }
}
