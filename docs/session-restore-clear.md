# Session Restore — Clear Specification

## What Problem We Solve

- Primary: Enable restoring previous sessions so users can continue work without redoing context.
- Primary scenarios: the agent process crashed, an unrecoverable provider/server error occurred, the machine rebooted, or any interruption that ends the current run.
- Secondary: Allow switching execution to an incompatible server configuration (e.g., model changed to a non‑resumable one, execution parameters drifted) while still providing a way to rebuild context.
- Secondary (future‑proofing): Lay the groundwork for future features such as restore‑to‑checkpoints and session cloning.

## External Behavior After

- Sessions List
  - Shows a paginated list (up to 20 rows) with a stats line: “Showing X–Y of Z · [scope]”.
  - Scope modes: “This project” (default; excludes sessions with unknown project root) and “All sessions”. In “All sessions”, the recorded project root is shown when known; otherwise “root: Unknown”.
  - Only sessions with visible user messages are shown; seed/system banners do not form a session.
  - Footer actions (unified look/feel with approval modals): “View”, “Restore”, “Replay”, “GPT Restore”.
  - Key hints: S (search), H (help). Search filters displayed fields; matches are highlighted.

- Session Viewer (read‑only)
  - Renders the selected rollout with proper line wrapping to terminal width.
  - Header: left‑aligned numerator “start–end / total” (based on wrapped rows), right‑aligned truncated file path.
  - Navigation: ↑/↓, PgUp/PgDn, Home/End; S to search visible content; H for help.
  - Footer actions: “Return”, “Restore”, “Replay”, “GPT Restore”.

- Restore Actions (unified semantics regardless of entry point)
- Restore (server): If the rollout header provides a valid provider resume token, we resume the original server session. Configuration parity is honored (model, reasoning effort/summary, sandbox policy, MCP availability) unless the user intentionally changes it.
- Replay (formerly “Experimental Restore”): If the server cannot be resumed (missing/invalid token or provider refuses), we rebuild context by replaying the entire history to the server “for context only”. This creates a new session and may not reproduce behavior exactly.
  - GPT Restore (local): A local continuation path that pre‑fills the composer with a “Restore this session:” prompt suitable for manual, non‑token‑based continuation.
  - Visual continuity: Starting Replay opens a progress overlay (“Restoring: [###—] NN%”). Segments auto‑advance without simulated key presses: for each segment a restore preamble is sent followed by an immediate Interrupt so the model does not act on restore content. Restored content is rendered progressively into history; a concise summary and usage signal are added at the end. Replay runs as a new session and writes to a new JSONL; the old rollout remains unchanged.
  - Auto‑fallback: If a server resume fails mid‑turn and the CLI/TUI is configured with auto‑fallback, we suggest Replay and present the plan (segments and approximate tokens).

## Behavior Before

- Storage was JSONL with a header line (metadata) followed by one record per line (response items and optional state lines). This remains true and backward‑compatible.
- Providers only emit a reliable resume token upon response completion; mid‑turn resumption was not guaranteed.
- If termination occurred during “thinking” or tool calls, there was no reconciliation of half‑finished function calls on restart, which could leave the UI appearing stuck or inconsistent.
- UI lacked a consolidated action selector and consistent navigation/palette between list and viewer. Users had to guess the next step when a server resume was unavailable.

## What We Changed (From Before to After)

- Unified Actions and Palette
  - Added a consistent actions footer to both Sessions List and Session Viewer: the same four actions with the same highlight palette as approval modals.
  - Aligned key hints and selection visuals to reduce cognitive overhead.

- Server Restore with Clear Fallback
  - When a valid provider token is present, choosing Restore (server) performs a blocking Handshake: the bottom pane shows a shimmering “Checking server connection…”. On success (“Done”), the full replay is inserted and execution continues in the restored session; on failure (“Fail”), the UI offers Replay for the same session (plan printed, Replay overlay opened). When absent/invalid, the UI explicitly suggests Replay.
  - Replay shows a plan summary (segments and approximate tokens) and runs through a progress overlay that auto‑advances without simulated key presses. Segments are sent as user input then immediately Interrupted to avoid actions on restore content, and they are rendered progressively into the transcript. A summary line is printed and a usage signal is emitted for downstream accounting. At the end, a final end‑of‑restore marker is sent to resume normal interaction. Approved commands from the old session are imported into the new session to preserve executor behavior (now for both manual Replay and auto‑fallback). The old session header and recorded MCP tools (when available) are provided to the core so it can record `settings_changed` and surface `mcp_tools_missing` before the next turn.

- Search and Help
  - Sessions List and Session Viewer both support inline search with highlighted matches and quick help blurbs to explain keys and restore modes.

- Storage and Metadata Additions
  - Rollout header now records additional fields when available (e.g., recorded project root, cwd, model, reasoning settings, sandbox policy) for deterministic scoping and relaunch parity.
  - All additions are backward‑compatible; older rollouts remain readable.

- Auto‑Fallback Hooks
  - Background notices (e.g., “resume token missing”) surface subtle lines in history and optionally trigger Replay planning when auto‑fallback is enabled.

## Tests Added/Updated

- Chat Widget
  - auto_fallback_exp_restore_triggers_on_missing_token_background_event: missing server token mid‑turn with auto‑fallback shows a Replay plan and opens the overlay.
  - auto_fallback_exp_restore_triggers_on_token_error: server resume error (e.g., previous_response_not_found) triggers Replay and surfaces the plan.

- Sessions Popup (List)
  - parses_jsonl_sessions_under_nested_dirs: discovers JSONL sessions in date‑partitioned directories and validates counts/preview.
  - sort_sessions_desc_by_timestamp: ensures newest‑first ordering.
  - esc_and_ctrl_c_close_popup: validates quick exit semantics without side effects.
  - session_viewer_actions_all_paths: exercises “View/Restore/Replay/GPT Restore” flows via the embedded viewer, including no‑token path for server Restore and overlay behavior for Replay.
  - server_restore_runs_handshake_and_continues: selecting Restore (server) relaunches, shows the checking overlay, and on success inserts the full replay; on failure it offers Replay and opens the overlay.

- Session Viewer
  - viewer_actions_isolated: validates Return/GPT Restore/Replay/Restore server behaviors, composer text invariants, and internal selector logic.

- Restore Progress Overlay (Replay)
  - progresses_to_completion_on_enter: Enter advances through all segments; on completion inserts a full replay and summary, then signals completion.
  - cancel_inserts_history_line: Esc cancels once, prints a single cancellation line, and only Interrupts if sending had begun.
  - no_auto_progress_without_enter: overlay never advances without explicit Enter.

- Experimental Restore Utilities
  - segments_under_threshold: greedy segmentation respects token thresholds and covers the entire item range.
  - single_over_limit_item_forces_one_item_chunk: single oversized items are emitted as one‑item chunks to guarantee progress.

How to run
- TUI only: `cargo test -p codex-tui`
- Whole workspace (if core/common changed): `cargo test --all-features`

## Other Changes and Rationale

- Visual and UX consistency: selection highlight colors in `tui/src/colors.rs` match approval modals; actions footers and key hints are uniform across views.
- Bottom pane integration: approval modals, overlays, and status indicators are layered correctly (overlays do not render above modals; status returns after modal dismissal when a task is still running).
- Defensive notices: when server resume is unavailable, the UI prints concise guidance instead of silently failing.
- Backwards compatibility and safety: new metadata is additive; unknown fields are ignored by parsers; state lines without resume tokens are safe to skip.
- Future‑proofing: the Replay mechanics (segmentation, preamble + Interrupt, final full replay rendering) form the basis for checkpoint‑based restore and session cloning without further UI upheaval.
