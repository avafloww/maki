# Manual input deferral (Alt+M)

## Goal

Give the user an explicit way to dismiss an input-demanding surface (a tool
permission prompt or the `question`/ask window, collectively "the model tool
calls that want to steal user input") and get back to typing, instead of being
forced to answer it immediately. This complements plan 8's *automatic* deferral
(queued on arrival, promoted on 2s idle or modal close): a *manual* Alt+M defer
is held until the user's next submit, and is surfaced as an affordance on the
prompt/ask so it is discoverable.

## Behavior

- `Alt+M` (`key::DEFER_INPUT`) hides whichever input surface is currently
  active and returns focus to the input box.
- A manually-hidden demand is queued with `hold_until_submit: true`. It ignores
  the 2s idle timer (unlike plan 8 auto-deferrals) and promotes only when the
  user submits the currently-focused input box. On promotion the panel
  re-appears (and rings its bell) so it can be answered.
- Because the model is often parked waiting for that tool's answer, a message
  submitted at the **main agent input box** while a demand is held is queued to
  the agent (`submit_or_queue` → shared queue), not misdelivered as the tool
  answer. This "queue the message" behavior is what the existing submission
  path already does while the agent is streaming; the release just re-arms
  promotion, it never reroutes the typed text.
- The affordance is shown on both surfaces: an `Alt+M defer, keep typing` hint
  row on the permission prompt (Normal state) and a left-aligned `Alt+M defer`
  bottom title on an active needs-input below-split (ask/question) float.

## Implementation

All changes are in `maki-ui`, building on plan 8's input-arbitration queue.

### Keybinding (`components/keybindings.rs`)

- `key::DEFER_INPUT = Alt+M` next to `EDIT_INPUT`.
- A `KEYBINDS` help entry (`General` context) so it appears in the in-app
  keybindings help and the generated keybindings docs.

### App (`app/mod.rs`)

- `InputDemand` gains `hold_until_submit: bool` (auto-deferrals keep `false`).
- `App` gains `submit_released: bool`, armed by a keyboard submit (top of
  `handle_submit` for the main box, and the `InputAction::Submit` arm of
  `handle_subagent_chat_key` for a subagent box) and consumed by the next
  promotion pass.
- `defer_active_input(&mut self) -> bool`:
  - Permission active → snapshot the open prompt into a `PermissionPayload`
    (`active_permission_payload` reads the `PermissionPrompt::Open` fields),
    close the prompt, clear `active_input`, and enqueue a `hold_until_submit`
    demand.
  - Question active → `float_mgr.release_focus()` (the window stays open),
    clear `active_input`, enqueue the held demand.
  - Nothing active → no-op, returns `false`.
- `promote_deferred_if_ready` consumes `submit_released` once at entry and, for
  a head that is `hold_until_submit`, treats `ready = submit_released` instead
  of the idle/blocking-modal timers. Non-held demand promotion is unchanged.
- `dispatch_overlay` (first branch) consumes `key::DEFER_INPUT` via
  `defer_active_input()`, so it works for both permission and question without
  those routes eating the key; it is consumed even as a no-op (Alt+M is a
  reserved hotkey, never a typed char).

### Float manager (`components/lua_float.rs`)

- `release_focus()` clears `focused_id`/`focused_rect` (the inverse of the
  existing `focus_input_window`). This is the one deliberate focus release;
  plan 8's M.3 avoidance of `release_focus` does not apply here because the
  float being released **is** the active surface (not a pre-existing popup), and
  the enqueued demand is `blocked_by_modal: false` + `hold_until_submit: true`,
  so promotion cannot fire prematurely via the modal-close clause.
- `render_window` adds a left-aligned `Alt+M defer` bottom title when the
  window is a `Split::Below` + `needs_input` one (i.e. the active ask window).

### Permission prompt (`components/permission_prompt.rs`)

- `HINT_DEFER_ROW = (key::DEFER_INPUT.label, "defer, keep typing")` appended to
  the Normal-state hint rows.

## Why this satisfies "queue the message"

When `Alt+M` defers a permission, the model stays parked on the tool's answer
guard and the run is still `Status::Streaming`. The submitted main-box text goes
through `submit_or_queue` → `queue_and_notify` (the existing busy path already
queues it to the shared queue). The deferral only adds the release: the held
demand re-promotes after the submit so the user can then answer the tool; the
typed message rides the queue and is later delivered as a user message, never as
the tool answer. Subagent input boxes submit to their driver queue as usual and
also arm the release.

## Tests

All in `maki-ui`:

- `app/tests.rs`
  - `alt_m_when_nothing_active_is_noop` — Alt+M with no input surface active is
    consumed without side effects.
  - `alt_m_defers_active_permission_until_submit` — idle-active permission is
    hidden by Alt+M; the 2s idle timer does **not** re-promote a hold; typing
    does not misanswer it; a submit (`handle_submit`) re-arms and re-promotes.
  - `alt_m_defers_active_question_until_submit` — same for the ask float
    (focus released on defer, restored on submit promotion).
  - Render assertions added to `permission_drawn_on_top_of_model_picker` and
    `question_drawn_on_top_when_active` that the defer hint is visible on each
    active surface.
- `components/lua_float.rs`
  - `release_focus_drops_only_focus_the_window_stays` — focus cleared, window
    still open so promotion can refocus it.

## Notes / non-goals

- Auto-deferral from plan 8 is untouched: `hold_until_submit` defaults to
  `false` on the arrival paths (permission request, `UiAction::OpenWin`).
- No change to the agent, Lua plugin layout, or docs beyond the generated
  keybindings entry. The deferral is a UI-only arbitration change.
- `Submit` of `exit`/`!`-shell/empty text still arms the release (it is set at
  the top of `handle_submit`), consistent with "the focused input box was
  submitted".

## Acceptance criteria

- **AC-1** An active permission prompt or ask float with a pending demand shows
  the `Alt+M` affordance.
- **AC-2** `Alt+M` hides the active surface, clears its focus, returns focus to
  the input box, and (re)queues the demand marked `hold_until_submit`.
- **AC-3** A held demand does **not** promote on 2s idle or on modal close; it
  promotes after the next submit of the focused input box, re-appearing and
  ringing its bell.
- **AC-4** Typing or submitting at the main input box while a demand is held
  never misdelivers the text as the tool answer; it flows through the normal
  submit/queue path.
