# Session Restore Mechanics

This document explains how session restore works in the Rust TUI: what existed before, what changed and why, what changed internally, and how it works end‑to‑end now. It highlights user benefits and the technical necessity behind each change so operators and contributors can reason about behavior and maintenance trade‑offs. The scope is intentionally precise: we solved the session viewing/restore problems and did not touch unrelated behavior.

## Scope and Non‑Goals

- Scope: session viewing and restoration in Rust TUI, a unified action selector, palette parity with approval modals, persisted rollout metadata (model/version/token), server‑based restore when possible, safe fallback when not, and reconciliation of unfinished tool‑calls on resume.
- Non‑goals (unchanged by design):
  - Networking/sandbox controls and environment variables.
  - Core streaming semantics besides the minimal synthetic aborts for unfinished tool‑calls.
  - Approval logic and decision flow (only visual palette alignment was done).
  - File formats beyond what is required to store/read resume metadata.
  - Any refactors to commands unrelated to session viewing/restoration.

## How It Worked Before

- Persistence formats:
  - Rust used JSONL, with a single header line (session meta) followed by one record per line (response items and state lines). JSONL made append‑only writes simpler and robust to abrupt exits.
- Server resume token timing:
  - The resume token (`previous_response_id` / provider resume token) is only known after a response completes, i.e., on `response.completed`. No stable token exists mid‑turn because providers can’t guarantee resumption of an already streaming response.
- Mid‑turn termination:
  - If the CLI terminated during “thinking” or tool‑calls, there was no built‑in reconciliation of half‑finished function calls. On restart, the engine could appear stuck or inconsistent if it read a function call without a corresponding output.
- UI experience:
  - The sessions list didn’t provide a consolidated action selector, and the viewer wasn’t consistently action‑driven. Navigation and “what to do next” weren’t as explicit as desired.

## What Changed and Why (External Behavior)

This section summarises the externally visible behaviors introduced or refined.

- Sessions list layout and scoping:
  - Stats line moved to the top; actions moved to the bottom. The stats line reads “Showing X–Y of Z · [scope]”.
  - The on‑disk file path is no longer shown. In “All sessions”, the recorded project root is shown (when present). In “This project”, sessions without a recorded project are excluded to keep the scope precise.
  - Sessions with no visible user messages are hidden. Seed/system banners (e.g., initial AGENTS.md read, environment context) are ignored and do not create a visible session.
  - Paging: up to 20 rows are shown to reduce scrolling overhead.
  - Key hints include “S” (search) and “H” (help); key labels use the same background highlight as approval modals.
  - Actions footer shows: “View”, “Restore”, “Replay”, “GPT Restore”. “View” opens a read‑only viewer; other actions operate on the selected session.

- Session viewer layout and navigation:
  - Long lines are wrapped to the terminal width. The X–Y / total numerator is computed from these wrapped rows.
  - The header shows the numerator left‑aligned and the session file path right‑aligned (the path is truncated from the left with an ellipsis if space is insufficient).
  - Standard navigation (↑/↓, PgUp/PgDn, Home/End) scrolls by wrapped rows; the numerator updates consistently with what is displayed.
- Actions footer shows: “Return”, “Restore”, “Replay”, “GPT Restore”. “Return” returns to the sessions list.

- Restore actions and flows:
- Action labels are unified across entry points: in the sessions list — “View / Restore / Replay / GPT Restore”, in the session viewer — “Return / Restore / Replay / GPT Restore”.
- Restore (server) now performs a blocking Handshake:
  - After selecting Restore, the bottom pane shows a shimmering status “Checking server connection…”. The UI remains blocked during the check.
  - The TUI sends `Op::HandshakeResume` to the server, which validates the stored provider resume token.
  - On success (“Done”): the overlay is dismissed and the full replay of the selected session is inserted into history for visual continuity; subsequent work continues in that restored session (bound to its JSONL and server token).
  - On failure (“Fail”): the overlay is dismissed and the UI offers Replay for the same session. A concise plan (segments and ~tokens) is printed followed by the Replay overlay (Enter to proceed, Esc to cancel).
- When a server token is unavailable, the UI suggests running “Replay”. The plan summary (segments and ~tokens) is shown only when starting “Replay”.
- “Replay” runs automatically: after confirmation (from the list or viewer) the restore proceeds end‑to‑end without per‑segment confirmations. For each segment we send a preamble and immediately an Interrupt, so the model does not act on restored content. While restoring, a progress overlay is shown: “Restoring: [#####…..] NN%”, updated as chunks are sent. On completion, a full replay is inserted into history using the same renderer as the Viewer/Restore (server).
 - “Replay” runs automatically and auto‑progresses without simulated key presses. For each segment we send a preamble and immediately an Interrupt, so the model does not act on restored content. While restoring, a progress overlay is shown: “Restoring: [#####…..] NN%”, updated as chunks are sent. On completion, a full replay is inserted into history using the same renderer as the Viewer/Restore (server). A final marker is sent to end the restore mode so the next user turn is not suppressed.

- Search and help:
  - Sessions list: press “S” to open a search prompt in the footer; typing filters on what is displayed (label and, where shown, recorded root). Esc exits, Enter confirms. Matches are highlighted in the label.
  - Session viewer: press “S” to open a search prompt in the footer; typing searches the displayed, wrapped lines. Enter jumps to the first match; “n”/“N” go to next/previous match. Matches are highlighted inline.
  - Press “H” on either screen to print a brief, context‑specific help blurb (what is shown, key bindings, and a one‑liner on restore modes).

### Restore Updates and Conventions

- Unified behavior: all restore variants — `Restore` (server), `Replay`, and `GPT Restore` (local) — behave the same regardless of where they are launched from. The same actions and results are available from both the sessions list and the session viewer.
- Visual continuity: during preparation and immediately after restore the user sees the same context/output as before the session ended (a full replay of the session is inserted into history using the same renderer as the viewer).
- New “Restore” (server):
  - Attempts to continue with the same server connection that existed prior to the interruption while controlling all environment details that affect parity (model, reasoning level/version, available MCP servers, sandbox policy, etc.).
  - Users may deliberately choose a different model/reasoning level/MCP servers; any behavior drift from such changes is at the user’s risk.
  - Selecting server Restore effectively switches execution to the restored session, and further work is written to its history. If a new session had been started already, the agent detaches from it; the new session’s in‑memory state is fully overwritten (disk artifacts remain so you can switch back).
- “Replay” (formerly Experimental restore in TypeScript Codex‑CLI):
  - Handles cases where the server session cannot be found for any reason (expired, provider error, provider change, incompatible model change, etc.).
  - We attempt to rebuild context by sending the history to the server “for context only” so the server reconstructs the conversation context. Accuracy is not guaranteed.
  - This creates a new server session and uploads the entire history in chunks; the restored content is rendered progressively into the transcript while a progress overlay shows status. The new session writes to a new JSONL file; the old rollout remains unchanged. Approved commands recorded in the old session are imported into the new session to preserve executor behavior.
- Renamed restore modes (for consistency in the UI/docs):
  - Restore — formerly “Server Restore” (now the primary option).
  - Replay — formerly “Experimental Restore”.
  - GPT Restore — formerly “Restore” (local continuation without a server token).

### Internal Changes

- Viewer wrapping and numerator:
  - Wrapped rows are recomputed for the current width during render; the numerator and content viewport share the same wrapped rows and visible window height, ensuring consistent X–Y / total.
  - The previous footer status line is removed; the numerator lives in the header alongside the right‑aligned file path.

- Sessions list rendering and filtering:
  - File paths are removed from the list; recorded root appears only in “All sessions”. “This project” excludes sessions with no recorded root.
- In “All sessions” mode, sessions without `recorded_project_root` explicitly show “root: Unknown”.

- Search implementation:
  - List search filters by displayed fields (label and, when present, recorded root) and updates results live; Esc restores the original list. Matches are highlighted in the label.
  - Viewer search operates over the displayed, wrapped lines; Enter jumps to the first match, “n”/“N” navigate subsequent/previous matches; matches are highlighted inline.

- Server restore and experimental flow:
  - When a server token is missing, the UI suggests running “Replay”. The plan summary (segments and ~tokens) is shown only when starting “Replay”. Large histories are carefully split into segments.
  - Restore (server) performs a Handshake first; success inserts the full replay and continues; failure offers Replay immediately (no intermediate state is committed).
  - Replay ports several Restore controls for parity:
    - Approved commands are imported from the old rollout into the new session for both manual Replay and auto‑fallback Replay.
    - MCP tools diff: when available, the list of tools recorded in the old rollout is compared to the current environment and missing tools are surfaced as a background notice and recorded in state.
    - Settings parity: the old session header is provided to the core so it can record a concise `settings_changed` line (from→to) on the first user turn of the new session.
  - During “Replay” segments do not trigger model actions: after each segment an Interrupt is sent. Segments auto‑advance without key presses and are rendered progressively into history as they are sent; a concise summary is printed at the end.

### Help

- Press “H” in the sessions list or in the viewer to print a short help blurb into the history: what the screen shows, the key bindings, and a one‑liner on restore modes.

### Rationale for doing it here

- These UX affordances are closest to the TUI’s bottom‑pane widgets and share the same highlight palette and key handling. Deferring them to a separate task would create avoidable divergence between list/viewer behaviors and raise the cost of future tweaks.

- Unified action selector (in sessions list and viewer):

  - a) User convenience: One consistent way to choose actions with Left/Right arrows and background highlight. It reduces cognitive load by matching the approval modal’s look and feel and makes Enter/Esc behavior predictable.
  - b) Technical necessity: A single selector eliminates per‑screen keybinding drift and reduces edge‑cases around focus management. The uniform navigation logic simplifies state transitions and automated tests.

- Restore (server) with clear fallback:

  - a) User convenience: If a server resume token is present, we resume on the server; if not, the UI clearly falls back to local restore and explains why. Users no longer guess what “Resume (server)” will do.
  - b) Technical necessity: Providers only emit a reliable token at `response.completed`. Explicit gating prevents attempts to “resume” an already streaming turn (which can’t be continued) and avoids undefined server behavior.

- Session Viewer: full replay with auto‑scroll:

  - a) User convenience: The viewer brings the most recent exchange into view automatically, so you can resume without manual scrolling.
  - b) Technical necessity: Rendering via the same building blocks as live history preserves formatting (colors, tool output, reasoning) for consistent fidelity during replay and restore.

- Persisted metadata (model, version, last response id/token):

  - a) User convenience: Restores use the right context automatically and can attempt server resume when viable; no manual bookkeeping is required.
  - b) Technical necessity: Downstream logic (resume modes, compatibility checks) depends on these fields; storing them near the rollout provides a single source of truth for both CLIs.

- Synthetic aborts for unfinished tool‑calls on resume:

  - a) User convenience: The system no longer gets “stuck” after a crash or forced exit mid‑turn; it proceeds cleanly with a clear history entry.
  - b) Technical necessity: We can’t splice into a partially consumed SSE stream, and the request/response contract requires every function call to have a matching output. Injecting an explicit aborted output reconciles state and enables the next turn.

- Unified highlight palette (matches approval modals):

  - a) User convenience: Visual consistency makes navigation intuitive and reduces mis‑clicks/mis‑presses.
  - b) Technical necessity: One palette reduces duplicated constants and keeps UI snapshots/tests stable across implementations.

- CLI parity (modes and flags):
  - a) User convenience: Operators can trigger the same restore modes via flags or UI, depending on their workflow (keyboard‑driven vs. scripted).
  - b) Technical necessity: Feature parity keeps cross‑component assumptions intact, reduces drift between CLIs, and simplifies documentation/testing. Where flags differ, the help output is the source of truth.

### Unified Restore (server) path

- Both the sessions list and the session viewer trigger the same Restore (server) flow. After a successful restore, the active chat is fully re‑bound to the selected session: new turns append to the same JSONL and the restored transcript is used to hydrate context. This guarantees identical behavior regardless of where Restore (server) was initiated.

## Internal Changes

- Rust core (resume reconciliation):

  - On resume, the engine scans saved items to identify any tool calls without matching outputs and injects a synthetic `FunctionCallOutput("aborted", success=false)` into the first pending input. This guarantees the response item sequence remains well‑formed and the next turn can proceed deterministically.
  - The server‑resume token (`provider_resume_token`) is written only on `response.completed`, ensuring tokens are stable checkpoints rather than transient mid‑stream artifacts.

- Storage format and meta propagation:
- Rust JSONL header always carries a stable `timestamp` and may include additional metadata such as `id` (UUID), `instructions` (seed/system text), `git` (commit/branch/repository), and optionally `model`/`version` when available. State lines (`{"record_type":"state", ...}`) are appended as the session progresses and may include `provider_resume_token` once available.
- Compatibility: Existing session files continue to work. Headers and state lines are additive and forward‑compatible; readers ignore unknown fields and treat missing/empty state fields as no‑ops while still parsing valid items.

- UI (Rust):

  - Sessions list and viewer share an action selector with Left/Right navigation and background highlighting (palette synced with approval modals). The viewer renders a capped tail for performance.
  - Keyboard specifics: Left/Right switches action; Enter activates; Esc returns. Up/Down continue to navigate list rows where applicable. Tab is not used for the action selector to avoid conflicts with multiline editors and other overlays.

- Unified transcript renderer (first step):

  - Rust: added `tui/src/transcript.rs` with equivalent logic. The `SessionViewer` reads JSONL, parses items once, and formats user/assistant lines via the shared helper.
  - Rationale: one codepath per platform for live/replay formatting avoids duplication and future drift. This is the foundation for Adaptive view and future full‑history rendering (tool calls/outputs) without re‑execution.

- Adaptive vs Full history in viewers:
  - The Rust viewer supports a toggle (key `F`) to switch between a compact conversation view (user/assistant only) and a full history view that also includes tool calls and their outputs. Rendering uses the same transcript helpers and does not re‑execute tools — this is a pure visualization of saved items.

## Programmatic Server Resume (no chat text)

- External behavior:

- a) User convenience: Selecting “Restore” (server) no longer pre‑fills any prompt. Instead, the app quietly arms the next request to use the stored server context (previous_response_id) if available. The UI shows a subtle notice that the token has been applied.
  - b) Technical necessity: Using the server’s stored response ID programmatically (rather than emitting a textual “resume” message) is the most reliable way to continue a session without polluting the transcript. It also avoids accidental side‑effects.

- Internal changes:

  - Core protocol: added `Op::SetResumeToken { token }` to update the provider resume token after session configuration.
  - Core protocol: added `Op::HandshakeResume` which emits a `BackgroundEvent(resume_token_confirmed|resume_token_missing)` without touching the transcript; can be used for a “quiet handshake”.
- TUI: SessionsPopup/SessionViewer send `SetResumeToken` when “Restore” (server) is chosen for a session that contains a stored token; composer text remains unchanged.

- Handshake (optional, future):

  - a) UX: We can display “Restoring session…” to the user but keep the handshake exchange hidden from the conversation view.
  - b) Feasibility: Prefer a non‑advancing provider healthcheck/ping. If a provider requires a text handshake, keep it out of the user transcript and do not advance the session head if possible.

- Fallback:
- If the server rejects the token or no token is found, users are offered “Replay” with an upfront estimate: number of segments and approximate token cost. The TUI shows a short summary in history and opens a restore overlay where Enter proceeds or Esc/Ctrl‑C cancels.

## Replay (Segmented)

- Planning and estimate:
- Both stacks segment the rollout into chunks by approximate tokens. When the user selects Replay (or when Restore is unavailable), the UI shows: “Replay plan: N segments (~T tokens).” This provides an upfront cost preview before proceeding.
- Progress and control:
  - A compact overlay presents progress; Ctrl‑X cancels. The final line summarizes completion and the approximate tokens sent. When providers return usage, the Rust TUI also surfaces the first post‑restore TokenCount next to the estimate.
- Real‑path behavior (feature‑flagged):
  - Rust TUI: `CODEX_TUI_EXPERIMENTAL_RESTORE_SEND=1` sends chunks as user input; oversized chunks are split conservatively to avoid provider overflows.
- Non‑goals:
  - No tool re‑execution; tool calls/outputs are only rendered in Full History view.
  - Exact pricing; only token counts are displayed.

Phrasing (TUI):

- Server resume active: “Restoring session using server context…”
- Server resume unavailable: “Server resume unavailable — no token.”
- Replay estimate: “Replay plan: N segments (~T tokens).”
- Cancel: “Replay cancelled by user.”

## How It Works Now

- Where state is stored:

  - `~/.codex/sessions/...` — Rust: JSONL with a header (`timestamp`, `model`, `version`) and optional state lines.
  - On `response.completed`, Rust appends a state line such as:
    `{"record_type":"state","provider_resume_token":"resp_..."}`.

- TUI session visibility:
  - The sessions list and the session viewer are displayed in the TUI bottom pane as modal views. The viewer focuses on the latest context, auto‑scrolling to show the most recent exchanges. For performance in terminals, the viewer renders a capped tail (latest lines) rather than the entire transcript; users can toggle Full History (key `F`) to include tool calls and outputs for reference. Modal overlays such as the Replay progress appear in the bottom pane and use the standard navigation hints (Enter to advance, Ctrl‑X to cancel). Subtle notices (e.g., server resume handshake) are inserted as small history lines to avoid polluting the main transcript.
  - Sessions list navigation: Left/Right to switch action; Up/Down to navigate; PageUp/PageDown to move faster; Enter to select; Esc/Ctrl+C to close; `A` toggles “This project” / “All sessions”.
  - Session viewer navigation: Up/Down to scroll; PageUp/PageDown to scroll faster; Home/End to jump to top/bottom; `F` toggles Full History.
  - Status lines: the sessions list and the session viewer display a compact status indicator `start–end / total` just above the footer.

## Project Scoping and Visibility

- Motivation: show relevant sessions by default; avoid restoring in the wrong project.
- Project root detection:
  - Walk up from the current `cwd` until `AGENTS.md` or `.git` is found; the first match is the project root.
  - If neither exists, the project root is the current `cwd`.
- Recorded project metadata:
  - Rollouts include `recorded_project_root` and `recorded_cwd` in the header.
  - Fields are additive and optional; readers ignore unknown/missing fields.
- Default visibility and toggle:
  - Default filter: “This project” — only sessions whose `recorded_project_root` matches the detected project root are shown.
  - Toggle: “All sessions” shows every entry and displays each session’s recorded project root for clarity.
- Cross‑project restore:
  - When a session belongs to another project:
    - Option A (recommended): relaunch in `recorded_project_root` (reinitialize cwd/session and reopen the chat UI in that context).
    - Option B: continue in the current `cwd` with a clear notice that paths/tools may not match.
- Backward compatibility:

  - Sessions without `recorded_project_root` remain visible under “All sessions” and are labeled accordingly.
  - No flags or commands are changed.

- Restore modes (sessions list and viewer):

  - Return — close the overlay/viewer and return to the composer.
  - Restore (server) — resumes using the stored provider token when present; otherwise the UI suggests Replay and prints an informational notice.
  - Replay — rebuilds server context by sending history in chunks (no actions taken during restore).
  - GPT Restore — local continuation: the composer is prefilled with `Resume this session: <path>`.

- Resume algorithm at a glance (Rust):

  1. Read header + items from JSONL.
  2. If any `function_call` lacks a matching output, synthesize an aborted output item and append it to the first pending input on resume.
  3. If a `provider_resume_token` is present and the operator selects Restore (server) (or flags indicate), use it; otherwise use local restore.
  4. Start the next turn from a consistent state; never attempt to resume a partially consumed SSE stream.

- Error handling and safeguards:

  - Missing token: clearly reported; local restore remains available.
  - Corrupt records: ignored line‑by‑line with best‑effort parsing; valid items still render/restore.
  - Strict contracts: every tool call is paired with an output (real or synthetic) before continuing.
  - IO failures: the sessions scanner and readers handle missing files or unreadable entries conservatively, skipping invalid files without crashing the UI.

- Known limitations:
  - If `disable_response_storage=true`, no server token is available → only local restore.
  - Mid‑stream resume isn’t supported by providers; resume always starts from the last completed checkpoint.
  - Very large histories are truncated in the Rust viewer to preserve TTY rendering performance, while still showing the latest context.

## Implementation Touchpoints (for reviewers)

- Rust TUI:
  - `tui/src/bottom_pane/sessions_popup.rs` — sessions popup with action selector and server restore gating; default filter “This project” with a toggle to “All sessions”.
  - `tui/src/bottom_pane/session_viewer.rs` — read‑only viewer with selector; isolated unit test for Left/Right/Enter/Esc behavior.
  - Range/status lines: the sessions popup and the session viewer render a `start–end / total` indicator above the footer to aid navigation.
  - `tui/src/colors.rs` — `SELECT_HL_BG/FG` for highlight parity with approval modal and selectors.
  - `tui/src/user_approval_widget.rs` — palette alignment only (no logic change).
  - Core: reconciliation logic lives in Rust core; token emission on `response.completed` unchanged in principle, but consumed by the new flows. Rollout header now includes `recorded_project_root`/`recorded_cwd`.

## More changed files and rationale

- Rust (TUI + Core)
  - `core/src/rollout.rs`: Adds `recorded_project_root` and `recorded_cwd` to the JSONL header (detected via `AGENTS.md`/`.git` walk‑up) so scope filtering and relaunch are deterministic.
  - `tui/src/cli.rs`: Surfaces sessions flags and entry options consistent with the sessions list and viewer behavior.
  - `tui/src/app.rs`: Dispatches AppEvents used by restore flows (notices, handshake, relaunch/continue); integrates with bottom‑pane views.
  - `tui/src/app_event.rs`: Centralizes events (e.g., InsertHistory, CodexOp) used by server handshake and restore overlays.
  - `tui/src/chatwidget.rs`: Surfaces background notices (e.g., resume handshake), integrates token‑usage and status updates in history; wires bottom‑pane hints.
  - `tui/src/colors.rs`: Ensures highlight colors match approval modal and selectors for a consistent UX.
  - `tui/src/lib.rs`: Crate glue/tests harness; unchanged behaviorally but part of the integration surface.
  - `tui/src/slash_command.rs`: Slash command plumbing; unchanged behaviorally for restore but part of the CLI feature set.
  - `tui/src/bottom_pane/sessions_popup.rs`: Default “This project” filter; toggle `A` to “All sessions”; cross‑project relaunch confirmation; `start–end / total` status line; key hints.
  - `tui/src/bottom_pane/session_viewer.rs`: Scrolling (Up/Down, PgUp/PgDn, Home/End); `start–end / total` status line; action selector; key hints.
  - `tui/src/bottom_pane/restore_progress_view.rs`: Experimental Restore progress overlay with cancel (Ctrl‑X) and final summary in history.
  - `tui/src/bottom_pane/*` (others): Supporting widgets and common selection rendering (no logic change beyond shared hints/structure).

## Change Control and Risk Mitigation

- Minimal footprint: we scoped changes to files directly involved in session view/restore, palette, and meta persistence. No unrelated commands or flows were modified.
- Backwards compatible: storage additions are additive; old sessions remain readable; missing fields trigger safe fallbacks.
- Defensive UI: clear notices for unavailable server resume; consistent keybindings reduce accidental actions.
- Test coverage: unit tests for selectors, fallbacks, and resume reconciliation; snapshot/behavior tests ensure renderers behave under terminal constraints.

## Appendix: Unified Highlight Palette

- The selection highlight palette is unified for consistency with the approval modals and to reduce cognitive overhead.
- Rust: `codex-rs/tui/src/colors.rs` defines matching `SELECT_HL_BG`/`SELECT_HL_FG`. They are used in `SessionsPopup`, `SessionViewer`, and `UserApprovalWidget` (approval modal) so selection visuals are consistent across the TUI.
- Navigation/hints are aligned: Left/Right to switch, Enter to select, Esc to go back. This mirrors the approval modal and reduces mode‑switching confusion.
- Appendix: Session Record Schema (Rust JSONL)
- File layout:
  - Line 1: Header JSON (metadata). Required: `timestamp` (ISO 8601). Common optional fields: `id` (UUID), `instructions` (seed text), `git` (commit/branch/repository), `model`/`version` when available.
  - Additional header fields persisted to aid restore parity: `reasoning_effort`, `reasoning_summary`, and `sandbox_policy` (all optional, recorded when known).
  - Subsequent lines: either `{"record_type":"state", ...}` entries (e.g., `provider_resume_token` when known) or response items (e.g., `message`, `function_call`, `function_call_output`, `reasoning`).
- Contract:
  - Each line is an independent JSON object (append‑only). Unknown fields are ignored; missing fields are treated as absent. State lines may appear without a resume token; parsers should ignore state lines they don’t recognize.

### CLI behavior on config drift

- When restoring from a rollout file via CLI flags, if the current configuration (model, reasoning_effort, reasoning_summary, sandbox_policy) differs from what is recorded in the session header, the CLI exits with an error unless one of the following is specified:
  - `--apply-session-settings`: apply the session settings from the header and continue.
  - `--keep-current-config`: keep the current config and continue.

This makes the CLI behavior explicit and avoids silently changing environment assumptions. Interactive TUI flows continue to present an in‑UI confirmation instead of exiting.

### Persisted approvals and MCP availability

- State records now include optional `approved_commands` (commands granted for the session). When a session is restored, these are loaded to match original behavior.
- MCP tools availability at restore time may differ from recording time. The TUI emits a warning only if tools recorded as available are missing at restore; otherwise it remains silent.

### Optional CLI auto‑fallback to Experimental Restore

- When using `--resume-experimental` with `--auto-fallback-exp-restore`, if the provider/server resume token is missing or invalid, the TUI automatically prepares an Experimental Restore plan from the rollout file and opens the restore overlay. If the environment variable `CODEX_TUI_EXPERIMENTAL_RESTORE_SEND=1` is set, the plan is sent immediately instead of requiring a confirmation.

## Appendix: Tests added

The restore feature set adds and updates tests across the TUI to cover the new flows and guard against regressions.

- TUI Chat widget
  - `codex-rs/tui/src/chatwidget/tests.rs::auto_fallback_exp_restore_triggers_on_missing_token_background_event`: when a background event indicates a missing server resume token and `--auto-fallback-exp-restore` is enabled, the widget announces a Replay plan and opens the restore overlay.
  - `codex-rs/tui/src/chatwidget/tests.rs::auto_fallback_exp_restore_triggers_on_token_error`: on server resume errors (e.g., `response.failed: previous_response_not_found`), the widget falls back to Replay and surfaces the plan summary.

- Sessions popup (list)
  - `codex-rs/tui/src/bottom_pane/sessions_popup.rs::parses_jsonl_sessions_under_nested_dirs`: discovers JSONL rollouts under date‑partitioned subdirectories; validates counts and first‑message preview.
  - `codex-rs/tui/src/bottom_pane/sessions_popup.rs::sort_sessions_desc_by_timestamp`: ensures newest‑first ordering.
  - `codex-rs/tui/src/bottom_pane/sessions_popup.rs::esc_and_ctrl_c_close_popup`: confirms Esc and Ctrl+C close the popup without side effects.
  - `codex-rs/tui/src/bottom_pane/sessions_popup.rs::session_viewer_actions_all_paths`: exercises “View”, “Restore (server)”, “Replay”, and “GPT Restore” from the list via the embedded viewer; verifies Replay opens the overlay and does not overwrite composer text; server Restore uses the stored token and preserves composer text. Also covers the no‑token path for server Restore.
  - `codex-rs/tui/src/bottom_pane/sessions_popup.rs::server_resume_emits_handshake_and_notice`: selecting server Restore emits a relaunch/handshake event and a subtle notice in history when a provider token is present.

- Session viewer (read‑only)
  - `codex-rs/tui/src/bottom_pane/session_viewer.rs::viewer_actions_isolated`: validates the footer selector and actions inside the viewer: Return (Enter) closes; GPT Restore pre‑fills composer; Replay opens overlay and leaves composer unchanged; Restore (server) preserves composer.

- Restore progress overlay (Replay)
  - `codex-rs/tui/src/bottom_pane/restore_progress_view.rs::progresses_to_completion_on_enter`: Enter advances segments to completion; on completion, a full replay is inserted into history plus a summary line and a `RestoreCompleted` signal.
  - `codex-rs/tui/src/bottom_pane/restore_progress_view.rs::cancel_inserts_history_line`: Esc cancels with a single history line; Interrupt is only sent if any sending began.
  - `codex-rs/tui/src/bottom_pane/restore_progress_view.rs::no_auto_progress_without_enter`: overlay never advances automatically without Enter.

- Experimental restore utilities
  - `codex-rs/tui/src/experimental_restore.rs::segments_under_threshold`: greedy segmentation respects the token threshold and covers the entire item range.
  - `codex-rs/tui/src/experimental_restore.rs::single_over_limit_item_forces_one_item_chunk`: single oversized items form a one‑item chunk to guarantee progress.

How to run:
- TUI crate tests: `cargo test -p codex-tui`
- Full workspace (when core/common changed): `cargo test --all-features`

Notes:
- Tests rely on in‑memory terminals (`ratatui::backend::TestBackend`) and temporary rollout fixtures; no external network is required.
