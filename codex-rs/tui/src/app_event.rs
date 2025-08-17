use codex_core::protocol::Event;
use codex_file_search::FileMatch;
use crossterm::event::KeyEvent;
use ratatui::text::Line;
use std::path::PathBuf;
use std::time::Duration;

use crate::app::ChatWidgetArgs;
use crate::slash_command::SlashCommand;

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum AppEvent {
    CodexEvent(Event),

    /// Request a redraw which will be debounced by the [`App`].
    RequestRedraw,

    /// Actually draw the next frame.
    Redraw,

    /// Schedule a one-shot animation frame roughly after the given duration.
    /// Multiple requests are coalesced by the central frame scheduler.
    ScheduleFrameIn(Duration),

    KeyEvent(KeyEvent),

    /// Text pasted from the terminal clipboard.
    Paste(String),

    /// Request to exit the application gracefully.
    ExitRequest,

    /// Forward an `Op` to the Agent. Using an `AppEvent` for this avoids
    /// bubbling channels through layers of widgets.
    CodexOp(codex_core::protocol::Op),

    /// Dispatch a recognized slash command from the UI (composer) to the app
    /// layer so it can be handled centrally.
    DispatchCommand(SlashCommand),

    /// Kick off an asynchronous file search for the given query (text after
    /// the `@`). Previous searches may be cancelled by the app layer so there
    /// is at most one in-flight search.
    StartFileSearch(String),

    /// Result of a completed asynchronous file search. The `query` echoes the
    /// original search term so the UI can decide whether the results are
    /// still relevant.
    FileSearchResult {
        query: String,
        matches: Vec<FileMatch>,
    },

    InsertHistory(Vec<Line<'static>>),

    StartCommitAnimation,
    StopCommitAnimation,
    CommitTick,

    /// Restore overlay finished; carry approx token estimate and segments
    /// so the chat layer can report provider usage on the next turn.
    RestoreCompleted {
        approx_tokens: usize,
        segments: usize,
    },

    /// Relaunch chat bound to an existing rollout file and optional provider token.
    /// Used by Restore (server) to fully switch to the selected session so further
    /// history is written into it (and context is hydrated from it).
    RelaunchWithResume {
        path: PathBuf,
        provider_token: Option<String>,
    },

    /// Onboarding: result of login_with_chatgpt.
    OnboardingAuthComplete(Result<(), String>),
    OnboardingComplete(ChatWidgetArgs),

    /// Relaunch chat for Replay as a fresh session (no resume binding).
    /// The handler should respect current process cwd for parity with the
    /// recorded project root when the caller has already changed it.
    RelaunchForReplay,

    /// Start Replay in the current chat session by opening the restore
    /// overlay with a concrete plan. Items must be valid response items
    /// (already filtered) and chunks specify [start,end,tokens].
    ReplayStart {
        items: Vec<serde_json::Value>,
        chunks: Vec<(usize, usize, usize)>,
        token_total: usize,
    },

    /// Periodic tick to auto-advance Replay overlay.
    ReplayTick,

    /// Stop the auto-advance loop for Replay overlay.
    StopReplayAuto,

    /// Start a blocking server-resume handshake (Restore flow).
    /// Shows a status view and sends Op::HandshakeResume; UI remains blocked
    /// until a background event confirms success or failure.
    StartHandshake,
}
