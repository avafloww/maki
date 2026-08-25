# Usage readout below input box + Synthetic quota support

## Goal

Show live provider quota below the input box (like `pi-synthetic`) as a dense, blue→red readout, and make the readout work for the Synthetic provider by implementing `Synthetic::fetch_usage` against Synthetic's `/v2/quotas` endpoint. Users see usage without opening `/usage`.

## Implementation Summary

Two independent pieces:

1. **Provider side (maki-providers):** `Synthetic` currently inherits the default `fetch_usage` → `Ok(None)`. Add a real `fetch_usage` that calls `GET https://api.synthetic.new/v2/quotas` with the existing bearer auth and parses the response into `ProviderUsage` / `UsageLimit`s (5h + weekly lanes), using the established pattern already used by `OpenAi` (`fetch_usage` → `get_text` → `parse_usage`) and `Z.AI`/`Deepseek` (`From<Resp> for ProviderUsage`).

2. **UI side (maki-ui):** The fetched usage already lives in app-level `usage_slot: Arc<ArcSwapOption<UsageFetchState>>` (`app/mod.rs:330`), which is only consumed by `UsageModal` today. Add a compact inline renderer that polls the same slot and draws one dense line (`5h30% w50%`) near the input box, colored blue→red by percentage. Add refresh triggers so the value is populated without opening `/usage`.

Scope boundary / non-goals: no change to the existing `/usage` modal content (it keeps rendering the same limits); the pi-synthetic "warning below threshold" behavior (option 1) is explicitly out of scope per the request — we only do the always-visible colored readout (option 2).

Key files:
- `maki-providers/src/providers/synthetic.rs` (add fetch_usage + parse)
- `maki-providers/src/providers/openai/platform.rs` (reference pattern only)
- `maki-ui/src/components/usage_modal.rs` (shared limit types / formatting reference)
- `maki-ui/src/app/mod.rs` (`usage_slot`, refresh wiring, poll loop at ~2186)
- `maki-ui/src/app/view.rs` (render the inline line near input box)
- `maki-ui/src/event_loop.rs` (`refresh_usage`, trigger on provider change), `app/tests.rs`

## Implementation Plan

### Phase 1 — Confirmed quota response shape

Synthetic's quota schema is version/account dependent. Before writing the parser, capture a live response:

```
curl -s https://api.synthetic.new/v2/quotas -H "Authorization: Bearer $SYNTHETIC_API_KEY"
```

Documented reference (from `dev.synthetic.new/docs/synthetic/quotas` and community parsers):
- Legacy v2: `subscription { limit, requests, renewAt? }`, `search.hourly { limit, requests, renewAt? }`.
- v3 (rolling quotas): five-hour request limit + weekly credit limit lanes ("weekly mana bar"), plus search. Known lane keys include `rollingFiveHourLimit` and `weeklyTokenLimit` (per CodexBar's parser).

Save both fixture bodies (v2 and v3) into the test module. If a live key is unavailable during implementation, use the documented shapes above and the actual key is not required for the parser logic.

### Phase 2 — `Synthetic::fetch_usage` (maki-providers)

In `maki-providers/src/providers/synthetic.rs`:

- Add a `Serializable` `QuotasResponse` struct (or per-shape structs) mirroring the endpoint.
- Parse the 5h and weekly lanes into two `UsageLimit`s: `label: "5h"` and `label: "w"`, `percentage` computed as `round(100 * used / limit)` since the API returns used+limit (no percent field), `reset_at` from the timestamp when present, `detail: None`. Only include a limit when the lane is present in the response.
- Implement `Provider::fetch_usage` mirroring `OpenAi::fetch_usage` (`openai/platform.rs:344`):
  ```rust
  fn fetch_usage(&self) -> BoxFuture<'_, Result<Option<ProviderUsage>, AgentError>> {
      Box::pin(async move {
          let auth = self.auth.lock().unwrap().clone();
          let url = "https://api.synthetic.new/v2/quotas";
          let text = self.compat.get_text(&auth, url).await?;
          Ok(Some(parse_quotas(&text)?))
      })
  }
  ```
  (`get_text` exists on `OpenAiCompatProvider`, used by `OpenAi::fetch_usage`.)
- Handle both v2 and v3 shapes: detect presence of `subscription` vs v3 rolling lane keys; fall back to `subscription` for 5h when v3 keys are absent.

### Phase 3 — Inline usage readout (maki-ui)

Add a compact, provider-agnostic renderer (works for any provider that populates `usage_slot`):

- In `maki-ui/src/components/usage_modal.rs` (or a small new module), add a pure helper:
  ```rust
  fn compact_usage_line(usage: &ProviderUsage, theme: &theme::Theme) -> Line<'static>
  ```
  Renders one line joining each limit as `<label><pct>%` (e.g. `5h30% w50%`), each span colored by percentage via a new `usage_color(pct)` helper (interpolate blue → red across 0-100; returns a `Style`).
- When `ProviderUsage::Ready`, the line is rendered; for `Loading`/`Unsupported`/`Error` render nothing (keeps the area clean for providers without a quota endpoint).
- Wire it into `app/view.rs` near the input area (the same bottom layout region where the input box / placeholder is drawn, `view.rs:289-292`), polling `self.usage_slot` each frame. Add it to the frame `poll` chain at `app/mod.rs:2186`.
- Add `usage_color` to the theme module (const-blue and const-red endpoints; interpolation helper). Keep it independent of existing `accent`/`status` styles so the blue→red ramp is explicit.

### Phase 4 — Refresh triggers

`usage_slot` only fills when `/usage` opens or Ctrl+R fires (`Action::RefreshUsage`, `app/mod.rs:1787`, `807`). Make the readout populated without opening `/usage`:

- Emit `Action::RefreshUsage` on boot and on provider/model change. The slot is already reset to `None` in `refresh_provider` / `change_model` (`event_loop.rs:1403,1501,1546`); trigger a refresh right after each such reset so the new provider's quota loads immediately. Preserve the existing manual Ctrl+R and "/usage" triggers.
- No new periodic timer (keep scope minimal); the value is refreshed on provider change and manual refresh. Additional per-turn refresh is optional and out of scope unless trivial.

## Acceptance Criteria

- **AC.1** The `parse_quotas` parser returns `Ok(Some(ProviderUsage))` with two limits labeled `5h` and `w` for a v3-formatted `/v2/quotas` response, with `percentage` correctly computed from `used/limit`. (Criterion is scoped to the pure parser, not the network/auth path, which is already exercised by the shared `openai_compat` layer.)
- **AC.2** Same parse falls back to the v2 `subscription` lane when v3 rolling lane keys are absent, still producing a `5h` limit.
- **AC.3** Malformed `/v2/quotas` bodies return an `AgentError` (not a panic), matching existing provider behavior.
- **AC.4** `compact_usage_line` turns a `ProviderUsage` with the synthetic limits into the visible text `5h30% w50%` (the exact label strings `5h`/`w` are the recommended defaults and asserted against the chosen constant, per the labeling decision).
- **AC.5** `usage_color` returns blue at 0% and red at 100%, with a monotonic interpolation between (spot-check a midpoint).
- **AC.6** Switching provider/model or booting the loop causes `usage_slot` to leave `None` (transition to `Loading`) without the operator typing `/usage` or Ctrl+R. Verified at the `event_loop` layer, since provider-change reset lives there (`event_loop.rs:1403,1501,1546`) and `App::execute_command` does not exercise it.
- **AC.7** The `/usage` modal still renders correctly for a Synthetic `Ready`-state usage (regression). The test asserts against the chosen label constant, so it survives a labeling change.
- **AC.8** Rendering the app with a `Ready(usage)` populated in `usage_slot` draws the compact readout line (the `5h30% w50%` text) in the bottom/input region — i.e. the view wiring, not just the pure helper, is exercised.

## Test Strategy

All tests live alongside the code (unit tests in-file, render/scenario tests in `maki-ui/src/app/tests.rs`).

| Criterion | Test |
|---|---|
| AC.1 | `synthetic.rs` unit test, `#[test_case]` with v3 fixture → assert labels/percentages/reset_at. |
| AC.2 | `synthetic.rs` unit test with v2 fixture (`subscription`) → assert `5h` limit present, no `w` lane when absent. |
| AC.3 | `synthetic.rs` unit test feeding malformed JSON → `assert!(result.is_err())`. |
| AC.4 | `compact_usage_line` unit test → assert rendered `Line` text equals `5h30% w50%`. |
| AC.5 | `usage_color` unit tests: pct 0 → blue, pct 100 → red, midpoint → between. |
| AC.6 | `event_loop`-level test: construct a loop (or invoke the `change_model`/`refresh_provider` paths) and assert `usage_slot` transitions from `None` to `Loading` after dispatch. If the loop isn't constructible in test today, split AC.6 into "boot emits RefreshUsage" (integration smoke) and "provider change triggers refresh" (unit over `refresh_provider`) — the harness must exist; confirm during Phase 4. |
| AC.7 | `app::render` snapshot test feeding a `Ready(ProviderUsage)` into the modal and asserting the modal still shows expected lines (mirror existing `usage_modal.rs` render tests at ~417/601). |
| AC.8 | `app/tests.rs` `TestBackend` render test (`rendered_rows()` at ~1432): load a `Ready(usage)` into `usage_slot`, render the app, assert the readout span (e.g. `5h30% w50%`) appears in the input/bottom region. |

Run: `just check -p maki-providers -p maki-ui`, then `just lint`, then `just test`. Existing infra (nextest, `app::render` render tests, `#[test_case]`) fully covers the new behavior — no new test harness needed.

## Review Strategy

- **Plan review:** after writing this file, run a `plan_reviewer` pass and fix/rebut findings before `plan_submit`.
- **Implementation review:** executing session runs `just lint` and `just test`; then (following repo guidance) dispatch a `general` subagent to review the diff against these ACs, fixing or rebutting any critical/high findings, repeating until no critical findings remain.

## Documentation Strategy

User-visible change (a new persistent usage readout). Add a short mention to the user docs that usage is shown inline next to the input box when the provider exposes a quota endpoint, and that `/usage` shows the full detail. Keep it one line and link the existing usage/context docs instead of duplicating. If the change stays small and behavior self-evident, note in docs whether a changelog entry is expected (none exists currently → skip unless asked).

No `AGENTS.md` change needed (pure provider + UI addition, no new architectural convention).

## Risks, Blockers, and Required Decisions

- **Quota response shape (R):** v2 vs v3 schema differs and the exact v3 key names are not fully documented. Mitigation: Phase 1 captures a live body; parser handles both shapes with separate fixtures. If no key is available, the parser uses documented shapes and this is explicitly fine for logic (AC.1–AC.3 still verifiable offline). No operator decision needed.
- **Inline placement (R):** exact rendering spot near the input box depends on `bottom_area` layout. Mitigation: render via a dedicated line in the input/lower-layout region; AC.4/AC.7 cover text and modal output, and the implementer verifies visually. Low risk.
- **Decision (operator):** whether labels should be the dense short form (`5h`/`w`) for Synthetic specifically, which also changes what the `/usage` modal prints for Synthetic. Recommended: yes to match the density goal; other providers keep their descriptive labels. Decide before Phase 2. ACs are written to a chosen label constant (not a hard-coded literal), so this decision stays cheap to change later.

## As built (deviations from this plan)

This section records what the implementing session actually found/did, where it
differs from the sketch above. Future readers should trust this over the
earlier guesses.

### The real `/v2/quotas` response (Phase 1, live)

Captured live with the auth token at `~/.local/state/maki/auth/synthetic.json`:

```json
{
  "subscription":       { "limit": 500, "requests": 0, "renewsAt": "2026-08-25T06:17:51.747Z" },
  "search":            { "hourly": { "limit": 250, "requests": 0, "renewsAt": "2026-08-25T02:17:51.751Z" } },
  "freeToolCalls":     { "limit": 0, "requests": 0, "renewsAt": "2026-08-26T01:17:51.756Z" },
  "weeklyTokenLimit":  { "nextRegenAt": "2026-08-25T02:15:17.000Z", "percentRemaining": 46.70,
                         "maxCredits": "$24.00", "remainingCredits": "$11.20" },
  "rollingFiveHourLimit": { "nextTickAt": "2026-08-25T01:19:00.000Z", "tickPercent": 0.05,
                            "remaining": 364, "max": 500, "limited": false }
}
```

Key findings the original plan guessed wrong:

- The 5h lane is `rollingFiveHourLimit`, reporting **`remaining` / `max`**
  (request-limit usage), not `used` / `limit`. `tickPercent` exists (0.05) and is
  Synthetic's own tick figure, but the plan's "used/limit" intent maps to
  `100 * (max - remaining) / max`. We compute from `remaining`/`max`
  (e.g. live: 364/500 → `5h27%`). Easy to switch to `tickPercent` if a
  Synthetic-mirroring readout is preferred; not done yet.
- The weekly lane `weeklyTokenLimit` reports a **remaining** percentage
  (`percentRemaining`), so usage = `100 - percentRemaining` (live: 53%).
- All timestamps are **RFC 3339 strings** (`nextTickAt`, `nextRegenAt`,
  `renewsAt`), not epoch. `parse_reset` parses them via `jiff::Timestamp`
  to epoch ms (same pattern as the Anthropic `parse_reset`). The plan assumed
  `renewAt`/epoch; reality uses `renewsAt` + RFC 3339.
- The v2 `subscription` fallback is real but `requests`/`limit` with
  `renewsAt`. Kept only as a `5h` fallback when `rollingFiveHourLimit` is absent.

### Parser (Phase 2)

`parse_quotas` handles: `rollingFiveHourLimit` (→ `5h`), falling back to
`subscription`; and `weeklyTokenLimit` (→ `w`). `percentage` = `round(100·used/limit)`
clamped, `reset_at` from the ISO string via `parse_reset`, `detail: None`.
Empty/`limits.len()==0` returns `AgentError::Config` (never panics). Test
fixtures updated to the real field names (AC.1–AC.3 preserved).

### Placement (Phase 3 — changed from plan)

The plan sketched drawing the readout near the input box. Post-implementation
the operator asked for it **after the ctx length + USD cost in the status bar**,
so it lives in `status_bar.rs` (`StatusBarContext.usage`, appended to
`rest_spans` right after the context/cost and any global cost) rather than the
input box border. `app/view.rs` `usage_readout()` still gates to `Ready` only.
This also makes it visible even when overlays are open (status bar renders
independently of `render_bottom_panel`).

### Refresh triggers (Phase 4)

`refresh_usage` refactored into an inert `refresh_usage_into(slot)` plus a
focused convenience; the trigger lives:
- boot (start of `EventLoop::run`),
- `change_model`,
- `refresh_provider` (same-slug branch),
- `Action::LoadSession` (via `refresh_usage_into` on that session's slot).

Each still resets the slot to `None` first (the readout watch then repaints via
the `usage_readout_watch` in the `tick` poll chain). Manual Ctrl+R and `/usage`
triggers unchanged.

### AC.6 — no automated test (exception)

The plan's AC.6 wants an `event_loop`-level test that a provider/model change
leaves `usage_slot` as `Loading`. No such harness exists today — `EventLoop`
has no test constructor (its tests only cover `RunNotificationState`) and
building one is substantial scaffolding. The behavior is implemented and the
synchronous `slot.store(Loading)` before the spawn makes the transition
deterministic, but there is **no regression test**. Flagged so a future session
can add an `EventLoop` test constructor if desired.

### Docs

`site/docs/content/token-economy/_index.md` gained a one-liner describing the
inline readout and that `/usage` shows full detail. No changelog (none exists).
