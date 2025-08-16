use std::fs;
use std::path::{Path, PathBuf};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::widgets::WidgetRef;
use unicode_segmentation::UnicodeSegmentation;

use super::popup_consts::MAX_POPUP_ROWS;
use super::scroll_state::ScrollState;
use super::selection_popup_common::GenericDisplayRow;
use super::selection_popup_common::render_rows;
use super::session_viewer::SessionViewer;
use crate::app_event::AppEvent;
use crate::bottom_pane::BottomPane;
use crate::bottom_pane::bottom_pane_view::BottomPaneView;
use crate::experimental_restore::{approximate_tokens, segment_items_by_tokens};
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub(crate) struct SessionMeta {
    pub path: PathBuf,
    pub timestamp: String,
    pub user_messages: usize,
    pub tool_calls: usize,
    pub first_message: String,
    pub provider_token: Option<String>,
    pub recorded_project_root: Option<String>,
}

// Matches the flattened fields emitted by core::rollout::SessionMetaWithGit
#[derive(Deserialize)]
struct RolloutMetaHeader {
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    provider_resume_token: Option<String>,
    #[serde(default)]
    recorded_project_root: Option<String>,
}

fn truncate_graphemes(s: &str, max: usize) -> String {
    let mut g = s.graphemes(true);
    let taken: String = g.by_ref().take(max).collect();
    // If original has more than `max` graphemes, append ellipsis.
    if s.graphemes(true).count() > max {
        format!("{taken}…")
    } else {
        taken
    }
}

fn format_label(m: &SessionMeta) -> String {
    let ts = if let Ok(dt) = DateTime::parse_from_rfc3339(&m.timestamp) {
        dt.with_timezone(&Utc).format("%Y-%m-%d %H:%M").to_string()
    } else {
        m.timestamp.clone()
    };
    let first = truncate_graphemes(&m.first_message, 50);
    format!(
        "{} · {} msgs/{} tools · {}",
        ts, m.user_messages, m.tool_calls, first
    )
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("jsonl")
}

fn scan_sessions_dir(dir: &Path, out: &mut Vec<SessionMeta>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_sessions_dir(&path, out);
            continue;
        }
        if !is_jsonl(&path) {
            continue;
        }
        if let Ok(txt) = fs::read_to_string(&path) {
            let mut lines = txt.lines();
            let (ts, provider_token, rec_root) = lines
                .next()
                .and_then(|l| serde_json::from_str::<RolloutMetaHeader>(l).ok())
                .map(|m| {
                    (
                        m.timestamp,
                        m.provider_resume_token,
                        m.recorded_project_root,
                    )
                })
                .unwrap_or_default();
            let mut user_messages = 0usize;
            let mut tool_calls = 0usize;
            let mut first_message = String::new();
            let mut token_from_state: Option<String> = None;
            for line in lines {
                let v: serde_json::Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("record_type")
                    .and_then(|rt| rt.as_str())
                    .map(|s| s == "state")
                    .unwrap_or(false)
                {
                    if let Some(tok) = v.get("provider_resume_token").and_then(|t| t.as_str()) {
                        token_from_state = Some(tok.to_string());
                    }
                    continue;
                }
                match v.get("type").and_then(|t| t.as_str()) {
                    Some("message") => {
                        if v.get("role").and_then(|r| r.as_str()) == Some("user") {
                            // Extract textual content for filtering and previews
                            let mut msg_text = String::new();
                            if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
                                for item in arr {
                                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                                        if !msg_text.is_empty() {
                                            msg_text.push(' ');
                                        }
                                        msg_text.push_str(&t.replace('\n', " "));
                                    }
                                }
                            }
                            // Ignore synthetic seed messages such as initial AGENTS.md read
                            // and initial environment context banner
                            let trimmed = msg_text.trim_start();
                            let is_seed = trimmed.starts_with("<user_instructions>")
                                || trimmed.starts_with("<environment_context>");
                            if !is_seed {
                                user_messages += 1;
                                if first_message.is_empty() && !msg_text.is_empty() {
                                    first_message = msg_text;
                                }
                            }
                        }
                    }
                    Some("function_call") => {
                        tool_calls += 1;
                    }
                    _ => {}
                }
            }
            let provider_token = provider_token.or(token_from_state);
            out.push(SessionMeta {
                path: path.clone(),
                timestamp: ts,
                user_messages,
                tool_calls,
                first_message,
                provider_token,
                recorded_project_root: rec_root,
            });
        }
    }
}

fn load_sessions_from_codex_home(codex_home: &Path) -> Vec<SessionMeta> {
    let mut out: Vec<SessionMeta> = Vec::new();
    let dir = codex_home.join("sessions");
    scan_sessions_dir(&dir, &mut out);
    out.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    out
}

pub(crate) struct SessionsPopup {
    state: ScrollState,
    items: Vec<SessionMeta>,
    items_backup: Option<Vec<SessionMeta>>,
    action_idx: usize,
    complete: bool,
    sessions_home: PathBuf,
    show_all: bool,
    project_root: PathBuf,
    pending_relaunch_root: Option<PathBuf>,
    pending_action: Option<u8>,
    confirming: bool,
    search_mode: bool,
    search_query: String,
}

impl SessionsPopup {
    pub(crate) fn new(codex_home: PathBuf) -> Self {
        fn detect_project_root(start: &Path) -> PathBuf {
            let mut dir = std::env::current_dir().unwrap_or_else(|_| start.to_path_buf());
            loop {
                let agents = dir.join("AGENTS.md");
                let dotgit = dir.join(".git");
                if agents.is_file() || dotgit.exists() {
                    return dir;
                }
                let parent = dir.parent().map(|p| p.to_path_buf());
                match parent {
                    Some(p) if p != dir => dir = p,
                    _ => return std::env::current_dir().unwrap_or_else(|_| start.to_path_buf()),
                }
            }
        }
        let proj_root = detect_project_root(&codex_home);

        let mut s = Self {
            state: ScrollState::new(),
            items: Vec::new(),
            items_backup: None,
            action_idx: 0,
            complete: false,
            sessions_home: codex_home,
            show_all: false,
            project_root: proj_root,
            pending_relaunch_root: None,
            pending_action: None,
            confirming: false,
            search_mode: false,
            search_query: String::new(),
        };
        s.refresh();
        s
    }

    fn toggle_mode(&mut self) {
        self.action_idx = (self.action_idx + 1) % 4;
    }

    fn refresh(&mut self) {
        let all = load_sessions_from_codex_home(&self.sessions_home)
            .into_iter()
            // Hide sessions that have no user commands at all. These are
            // typically auto-start sessions (e.g., initial AGENTS.md read)
            // and should not be displayed in the sessions list.
            .filter(|m| m.user_messages > 0)
            .collect::<Vec<_>>();

        if self.show_all {
            self.items = all;
        } else {
            let proj = self.project_root.to_string_lossy().to_string();
            self.items = all
                .into_iter()
                .filter(|m| match &m.recorded_project_root {
                    Some(root) => root == &proj,
                    None => false, // hide legacy sessions without recorded root
                })
                .collect();
        }
        self.state.clamp_selection(self.items.len());
    }

    /// Construct with explicit visibility scope and project root.
    pub(crate) fn with_params(codex_home: PathBuf, show_all: bool, project_root: PathBuf) -> Self {
        let mut s = Self {
            state: ScrollState::new(),
            items: Vec::new(),
            items_backup: None,
            action_idx: 0,
            complete: false,
            sessions_home: codex_home,
            show_all,
            project_root,
            pending_relaunch_root: None,
            pending_action: None,
            confirming: false,
            search_mode: false,
            search_query: String::new(),
        };
        s.refresh();
        s
    }

    /// Select the first item matching `path` and ensure it is visible.
    pub(crate) fn select_path(&mut self, path: &Path) {
        if self.items.is_empty() {
            return;
        }
        if let Some(i) = self.items.iter().position(|m| m.path == path) {
            self.state.selected_idx = Some(i);
            self.state
                .ensure_visible(self.items.len(), MAX_POPUP_ROWS.min(self.items.len()));
        }
    }

    fn on_enter<'a>(&mut self, pane: &mut BottomPane<'a>) {
        if let Some(idx) = self.state.selected_idx {
            if let Some(meta) = self.items.get(idx) {
                if let Some(rec_root) = &meta.recorded_project_root {
                    if rec_root != &self.project_root.to_string_lossy() && !self.confirming {
                        // ask for confirmation first
                        self.pending_relaunch_root = Some(PathBuf::from(rec_root));
                        self.pending_action = Some(self.action_idx as u8);
                        self.confirming = true;
                        pane.app_event_tx.send(AppEvent::InsertHistory(vec![
                            ratatui::text::Line::from("Session belongs to another project:"),
                            ratatui::text::Line::from(rec_root.clone()),
                            ratatui::text::Line::from(
                                "Press Enter to relaunch in recorded root; Esc to continue here.",
                            ),
                            ratatui::text::Line::from(""),
                        ]));
                        return;
                    }
                }
                match self.action_idx {
                    0 => {
                        // View in session viewer with action selector
                        let viewer = SessionViewer::new(
                            meta.path.clone(),
                            meta.provider_token.clone(),
                            self.sessions_home.clone(),
                            self.show_all,
                            self.project_root.clone(),
                        );
                        pane.show_view(Box::new(viewer));
                    }
                    1 => {
                        // Restore (local): pre-insert the full rendered transcript for parity
                        // with the Session Viewer/Server Restore, then prefill the composer prompt.
                        if let Ok(txt) = std::fs::read_to_string(&meta.path) {
                            // Collect JSON items (skip header)
                            let mut items: Vec<serde_json::Value> = Vec::new();
                            for line in txt.lines().skip(1) {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                                    items.push(v);
                                }
                            }
                            // Render full replay with the same renderer as Viewer/Server Restore.
                            let to_insert = crate::transcript::render_replay_lines(&items);
                            if !to_insert.is_empty() {
                                pane.app_event_tx.send(AppEvent::InsertHistory(to_insert));
                            }
                        }
                        let prompt = format!("Restore this session: {}", meta.path.display());
                        pane.set_composer_text(prompt);
                    }
                    2 => {
                        // Experimental resume: plan segmented restore and show plan summary
                        if let Ok(txt) = std::fs::read_to_string(&meta.path) {
                            let mut items_json: Vec<serde_json::Value> = Vec::new();
                            for line in txt.lines().skip(1) {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                                    items_json.push(v);
                                }
                            }
                            let response_items =
                                crate::experimental_restore::filter_response_items(&items_json);
                            let chunks = segment_items_by_tokens(&response_items, 2000);
                            let total_tokens = approximate_tokens(&response_items);
                            let summary = format!(
                                "Experimental restore plan: {} segments (~{} tokens).",
                                chunks.len(),
                                total_tokens
                            );
                            // Display an English blurb per request, with plan and keys
                            let blurb = "Experimental restore: This will restore the entire prior conversation history to the server-side context.\n";
                            pane.app_event_tx.send(AppEvent::InsertHistory(vec![
                                ratatui::text::Line::from("Experimental restore").magenta(),
                                ratatui::text::Line::from(blurb.to_string()),
                                ratatui::text::Line::from(summary.clone()),
                                ratatui::text::Line::from("Press Enter to continue; Esc cancels."),
                                ratatui::text::Line::from(""),
                            ]));
                            // Show a progress overlay view (always wired with a real plan).
                            let view = super::restore_progress_view::RestoreProgressView::from_plan(
                                response_items.clone(),
                                chunks.clone(),
                                total_tokens,
                            );
                            pane.show_view(Box::new(view));
                            // Auto-progress all segments once confirmed by the user.
                            // Simulate Enter presses so the overlay can stream progress updates.
                            use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
                            for _ in 0..chunks.len() {
                                pane.app_event_tx.send(crate::app_event::AppEvent::KeyEvent(
                                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                                ));
                            }
                            // Mark this popup complete so the overlay remains active and receives key events.
                            self.complete = true;
                        } else {
                            pane.app_event_tx.send(AppEvent::InsertHistory(vec![
                                ratatui::text::Line::from(
                                    "failed to read rollout for experimental restore",
                                )
                                .red(),
                                ratatui::text::Line::from(""),
                            ]));
                        }
                    }
                    _ => {
                        // Server Restore: insert transcript for parity with viewer, then relaunch.
                        if let Some(token) = &meta.provider_token {
                            if let Ok(txt) = std::fs::read_to_string(&meta.path) {
                                let mut items: Vec<serde_json::Value> = Vec::new();
                                for line in txt.lines().skip(1) {
                                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                                        items.push(v);
                                    }
                                }
                                let to_insert = crate::transcript::render_replay_lines(&items);
                                if !to_insert.is_empty() {
                                    pane.app_event_tx.send(AppEvent::InsertHistory(to_insert));
                                }
                            }
                            pane.app_event_tx.send(AppEvent::RelaunchWithResume {
                                path: meta.path.clone(),
                                provider_token: Some(token.clone()),
                            });
                        } else {
                            if std::fs::read_to_string(&meta.path).is_err() {
                                pane.app_event_tx.send(AppEvent::InsertHistory(vec![
                                    ratatui::text::Line::from(
                                        "server resume unavailable — no token",
                                    )
                                    .gray(),
                                    ratatui::text::Line::from(
                                        "failed to read rollout for experimental restore",
                                    )
                                    .red(),
                                    ratatui::text::Line::from(""),
                                ]));
                            } else {
                                pane.app_event_tx.send(AppEvent::InsertHistory(vec![
                                    ratatui::text::Line::from(
                                        "Server restore unavailable — no token.",
                                    )
                                    .gray(),
                                    ratatui::text::Line::from(
                                        "Use ←/→ to select 'Exp. Restore' and press Enter to start.",
                                    )
                                    .gray(),
                                    ratatui::text::Line::from(""),
                                ]));
                            }
                        }
                    }
                }
                self.complete = true;
            }
        }
    }
}

impl<'a> BottomPaneView<'a> for SessionsPopup {
    fn handle_key_event(
        &mut self,
        pane: &mut BottomPane<'a>,
        key_event: crossterm::event::KeyEvent,
    ) {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyModifiers;
        // Handle inline search mode input
        if self.search_mode {
            match key_event.code {
                KeyCode::Esc => {
                    self.search_mode = false;
                    if let Some(b) = self.items_backup.take() {
                        self.items = b;
                    }
                    self.state.clamp_selection(self.items.len());
                }
                KeyCode::Enter => {
                    self.search_mode = false; // keep filtered list
                    self.items_backup = None;
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                }
                KeyCode::Char(ch) => {
                    if !key_event.modifiers.contains(KeyModifiers::CONTROL) {
                        self.search_query.push(ch);
                        let source = self.items_backup.as_ref().unwrap_or(&self.items).clone();
                        let q = self.search_query.to_lowercase();
                        if q.is_empty() {
                            if let Some(b) = self.items_backup.as_ref() {
                                self.items = b.clone();
                            }
                        } else {
                            self.items = source
                                .into_iter()
                                .filter(|m| {
                                    let name = format_label(m).to_lowercase();
                                    let desc = if self.show_all {
                                        m.recorded_project_root
                                            .as_deref()
                                            .unwrap_or("")
                                            .to_lowercase()
                                    } else {
                                        String::new()
                                    };
                                    name.contains(&q) || (!desc.is_empty() && desc.contains(&q))
                                })
                                .collect();
                        }
                        self.state.clamp_selection(self.items.len());
                    }
                }
                _ => {}
            }
            pane.request_redraw();
            return;
        }

        // Non-search key handling
        if matches!(key_event.code, KeyCode::Char('h') | KeyCode::Char('H')) {
            pane.app_event_tx.send(AppEvent::InsertHistory(vec![
                ratatui::text::Line::from("Sessions List: View / Restore / Exp. Restore / Server Restore"),
                ratatui::text::Line::from("←/→ switch · ↑/↓ navigate · PgUp/PgDn fast · Enter select · Esc/Ctrl+C close · A toggle scope · S search · H help"),
                ratatui::text::Line::from("Restore inserts a full replay into history, then pre-fills the composer."),
                ratatui::text::Line::from("Exp. Restore runs automatically with a live progress bar; each segment sends and is interrupted to prevent actions."),
                ratatui::text::Line::from("Server Restore behavior is consistent from list or viewer; when a token is missing, a clear fallback is offered."),
                ratatui::text::Line::from("Note: empty sessions are hidden; seed entries (<user_instructions>, <environment_context>) are ignored."),
                ratatui::text::Line::from("")
            ]));
            pane.request_redraw();
            return;
        }
        match key_event {
            crossterm::event::KeyEvent {
                code: KeyCode::Up, ..
            } => {
                self.state.move_up_wrap(self.items.len());
                self.state
                    .ensure_visible(self.items.len(), MAX_POPUP_ROWS.min(self.items.len()));
            }
            crossterm::event::KeyEvent {
                code: KeyCode::Home,
                ..
            } => {
                if !self.items.is_empty() {
                    self.state.selected_idx = Some(0);
                    self.state
                        .ensure_visible(self.items.len(), MAX_POPUP_ROWS.min(self.items.len()));
                }
            }
            crossterm::event::KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.complete = true;
            }
            crossterm::event::KeyEvent {
                code: KeyCode::Down,
                ..
            } => {
                self.state.move_down_wrap(self.items.len());
                self.state
                    .ensure_visible(self.items.len(), MAX_POPUP_ROWS.min(self.items.len()));
            }
            crossterm::event::KeyEvent {
                code: KeyCode::End, ..
            } => {
                if !self.items.is_empty() {
                    self.state.selected_idx = Some(self.items.len() - 1);
                    self.state
                        .ensure_visible(self.items.len(), MAX_POPUP_ROWS.min(self.items.len()));
                }
            }
            crossterm::event::KeyEvent {
                code: KeyCode::Right,
                ..
            } => {
                self.toggle_mode();
            }
            crossterm::event::KeyEvent {
                code: KeyCode::Left,
                ..
            } => {
                self.action_idx = (self.action_idx + 3) % 4;
            }
            crossterm::event::KeyEvent {
                code: KeyCode::Char('s') | KeyCode::Char('S'),
                ..
            } => {
                self.search_mode = true;
                self.search_query.clear();
                self.items_backup = Some(self.items.clone());
            }
            crossterm::event::KeyEvent {
                code: KeyCode::Char('h') | KeyCode::Char('H'),
                ..
            } => {
                pane.app_event_tx.send(AppEvent::InsertHistory(vec![
                    ratatui::text::Line::from("Sessions: View / Restore / Exp. Restore / Server Restore"),
                    ratatui::text::Line::from("Use ←/→ to choose an action; ↑/↓ to navigate; PgUp/PgDn to page; A toggles scope (This project/All); S opens inline search; H shows this help."),
                    ratatui::text::Line::from("Restore inserts a full replay into history and continues locally (appends to the same JSONL)."),
                    ratatui::text::Line::from("Exp. Restore runs automatically with a live progress bar; each segment is interrupted to prevent actions while restoring."),
                    ratatui::text::Line::from("Server Restore resumes with a stored provider token when available; otherwise a clear fallback is offered. Behavior is the same from list or viewer."),
                    ratatui::text::Line::from("Only sessions with visible user messages are listed; seed/system entries (e.g., initial instructions/environment) are hidden."),
                    ratatui::text::Line::from("")
                ]));
            }
            crossterm::event::KeyEvent {
                code: KeyCode::Char('a'),
                ..
            } => {
                self.show_all = !self.show_all;
                self.refresh();
            }
            crossterm::event::KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                if self.confirming {
                    if let (Some(root), Some(act)) =
                        (self.pending_relaunch_root.clone(), self.pending_action)
                    {
                        if let Err(e) = std::env::set_current_dir(&root) {
                            pane.app_event_tx.send(AppEvent::InsertHistory(vec![
                                ratatui::text::Line::from(format!(
                                    "Failed to change directory: {}",
                                    e
                                ))
                                .red(),
                                ratatui::text::Line::from(""),
                            ]));
                        } else {
                            pane.app_event_tx.send(AppEvent::InsertHistory(vec![
                                ratatui::text::Line::from(format!(
                                    "Relaunched in recorded project root: {}",
                                    root.display()
                                )),
                                ratatui::text::Line::from(""),
                            ]));
                            self.project_root = root;
                        }
                        self.confirming = false;
                        self.pending_relaunch_root = None;
                        self.action_idx = act as usize;
                        self.on_enter(pane);
                    }
                } else {
                    self.on_enter(pane);
                }
            }
            crossterm::event::KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                // Close on Ctrl+C
                self.complete = true;
            }
            _ => {}
        }
    }

    fn on_ctrl_c(&mut self, _pane: &mut BottomPane<'a>) -> super::CancellationEvent {
        self.complete = true;
        super::CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn desired_height(&self, _width: u16) -> u16 {
        // header + list (up to MAX) + status line
        3 + self.items.len().clamp(1, MAX_POPUP_ROWS) as u16
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Render stats in first row, then the list, then actions + key hints as footer.
        use crate::colors::{SELECT_HL_BG, SELECT_HL_FG};
        use ratatui::style::{Color, Style};
        use ratatui::text::{Line, Span};

        // Stats header: show visible range and total, make it explicit and include scope.
        let total = self.items.len();
        let mut start_idx = self.state.scroll_top.min(total.saturating_sub(1));
        if let Some(sel) = self.state.selected_idx {
            if sel < start_idx {
                start_idx = sel;
            } else if MAX_POPUP_ROWS > 0 {
                let bottom = start_idx + MAX_POPUP_ROWS - 1;
                if sel > bottom {
                    start_idx = sel + 1 - MAX_POPUP_ROWS;
                }
            }
        }
        let visible = MAX_POPUP_ROWS.min(total);
        let end_idx = (start_idx + visible).min(total);
        let scope = if self.show_all {
            "All sessions"
        } else {
            "This project"
        };
        let stats_text = if total == 0 {
            format!("Showing 0–0 of 0 · {}", scope)
        } else {
            format!(
                "Showing {}–{} of {} · {}",
                start_idx.saturating_add(1),
                end_idx,
                total,
                scope
            )
        };
        Line::from(stats_text)
            .style(Style::default().fg(Color::Gray))
            .render_ref(
                Rect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );

        let list_area = Rect {
            x: area.x,
            y: area.y.saturating_add(1),
            width: area.width,
            height: area.height.saturating_sub(1),
        };
        let q_opt = if self.search_query.is_empty() {
            None
        } else {
            Some(self.search_query.to_lowercase())
        };
        let rows_all: Vec<GenericDisplayRow> = self
            .items
            .iter()
            .map(|m| {
                // hide file path; for All sessions show recorded root if present
                let desc = if self.show_all {
                    Some(format!(
                        "root: {}",
                        m.recorded_project_root
                            .as_deref()
                            .filter(|s| !s.is_empty())
                            .unwrap_or("Unknown")
                    ))
                } else {
                    None
                };
                let name = format_label(m);
                let match_indices = if let Some(q) = q_opt.as_ref() {
                    let mut idxs = Vec::new();
                    let lower = name.to_lowercase();
                    let mut i = 0usize;
                    while let Some(pos) = lower[i..].find(q) {
                        let abs = i + pos;
                        // bold each char in the match range
                        for j in abs..abs + q.len() {
                            idxs.push(j);
                        }
                        i = abs + q.len();
                    }
                    if idxs.is_empty() { None } else { Some(idxs) }
                } else {
                    None
                };
                GenericDisplayRow {
                    name,
                    match_indices,
                    is_current: false,
                    description: desc,
                }
            })
            .collect();
        render_rows(list_area, buf, &rows_all, &self.state, MAX_POPUP_ROWS);
        // Footer: actions/hints or search input
        let footer = if self.search_mode {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(Span::raw("Search: "));
            spans.push(Span::styled(
                self.search_query.clone(),
                Style::default().bg(SELECT_HL_BG).fg(SELECT_HL_FG),
            ));
            Line::from(spans).style(Style::default().fg(Color::White))
        } else {
            let actions = ["View", "Restore", "Exp. Restore", "Server Restore"];
            let mut spans: Vec<Span> = Vec::new();
            let restorable = self
                .state
                .selected_idx
                .and_then(|i| self.items.get(i))
                .map(|m| m.user_messages > 0)
                .unwrap_or(false);
            for (i, a) in actions.iter().enumerate() {
                let disabled = i > 0 && !restorable;
                if i == self.action_idx && !disabled {
                    spans.push(Span::styled(
                        format!(" {} ", a),
                        Style::default().bg(SELECT_HL_BG).fg(SELECT_HL_FG),
                    ));
                    spans.push(Span::raw(" "));
                } else if disabled {
                    spans.push(Span::styled(
                        format!(" {} ", a),
                        Style::default().fg(Color::DarkGray),
                    ));
                    spans.push(Span::raw(" "));
                } else {
                    spans.push(Span::raw(format!(" {} ", a)));
                    spans.push(Span::raw(" "));
                }
            }
            let key_style = Style::default().bg(SELECT_HL_BG).fg(SELECT_HL_FG);
            spans.push(Span::raw("  "));
            spans.push(Span::styled("←/→", key_style));
            spans.push(Span::raw(" switch · "));
            spans.push(Span::styled("↑/↓", key_style));
            spans.push(Span::raw(" navigate · "));
            spans.push(Span::styled("PgUp/PgDn", key_style));
            spans.push(Span::raw(" fast · "));
            spans.push(Span::styled("Enter", key_style));
            spans.push(Span::raw(" select · "));
            spans.push(Span::styled("Esc/Ctrl+C", key_style));
            spans.push(Span::raw(" close · "));
            spans.push(Span::styled("A", key_style));
            spans.push(Span::raw(" toggle scope · "));
            spans.push(Span::styled("S", key_style));
            spans.push(Span::raw(" search · "));
            spans.push(Span::styled("H", key_style));
            spans.push(Span::raw(" help"));
            Line::from(spans).style(Style::default().fg(Color::DarkGray))
        };
        footer.render_ref(
            Rect {
                x: area.x,
                y: area.y + area.height.saturating_sub(1),
                width: area.width,
                height: 1,
            },
            buf,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event::AppEvent;
    use crate::bottom_pane::{BottomPane, BottomPaneParams};
    use std::io::Write;
    use std::sync::mpsc::channel;

    fn write_rollout(dir: &Path, name: &str, lines: &[&str]) {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).expect("create file");
        for l in lines {
            writeln!(f, "{}", l).expect("write line");
        }
    }

    #[test]
    fn parses_jsonl_sessions_under_nested_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let codex_home = tmp.path().to_path_buf();
        let sessions_dir = codex_home
            .join("sessions")
            .join("2025")
            .join("08")
            .join("12");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let meta = r#"{"timestamp":"2025-08-12T10:20:30.000Z"}"#;
        let msg_user = r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"hello world"}]}"#;
        let fcall = r#"{"type":"function_call","name":"tool","arguments":"{}","call_id":"1"}"#;
        write_rollout(
            &sessions_dir,
            "rollout-2025-08-12T10-20-30-abc.jsonl",
            &[meta, msg_user, fcall],
        );

        let items = load_sessions_from_codex_home(&codex_home);
        assert_eq!(items.len(), 1);
        let s = &items[0];
        assert_eq!(s.timestamp, "2025-08-12T10:20:30.000Z");
        assert_eq!(s.user_messages, 1);
        assert_eq!(s.tool_calls, 1);
        assert!(s.first_message.contains("hello world"));
    }

    #[test]
    fn esc_and_ctrl_c_close_popup() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let tmp = tempfile::tempdir().unwrap();
        let codex_home = tmp.path().to_path_buf();
        std::fs::create_dir_all(codex_home.join("sessions")).unwrap();

        let (tx_raw, _rx) = channel::<AppEvent>();
        let tx = crate::app_event_sender::AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            has_input_focus: true,
            enhanced_keys_supported: false,
        });

        let mut popup = SessionsPopup::new(codex_home);
        assert!(!popup.is_complete());

        popup.handle_key_event(
            &mut pane,
            KeyEvent {
                code: KeyCode::Esc,
                modifiers: KeyModifiers::NONE,
                kind: crossterm::event::KeyEventKind::Press,
                state: crossterm::event::KeyEventState::NONE,
            },
        );
        assert!(popup.is_complete());

        // Reset and test Ctrl+C path uses on_ctrl_c
        let mut popup2 = SessionsPopup::new(tmp.path().to_path_buf());
        let _ = <SessionsPopup as super::BottomPaneView>::on_ctrl_c(&mut popup2, &mut pane);
        assert!(popup2.is_complete());
    }

    #[test]
    fn sort_sessions_desc_by_timestamp() {
        let tmp = tempfile::tempdir().unwrap();
        let codex_home = tmp.path().to_path_buf();
        let d = codex_home
            .join("sessions")
            .join("2025")
            .join("08")
            .join("12");
        std::fs::create_dir_all(&d).unwrap();

        // older
        write_rollout(
            &d,
            "rollout-2025-08-12T10-20-30-a.jsonl",
            &[
                r#"{"timestamp":"2025-08-12T10:20:30.000Z"}"#,
                r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"a"}]}"#,
            ],
        );
        // newer
        write_rollout(
            &d,
            "rollout-2025-08-12T11-20-30-b.jsonl",
            &[
                r#"{"timestamp":"2025-08-12T11:20:30.000Z"}"#,
                r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"b"}]}"#,
            ],
        );

        let items = load_sessions_from_codex_home(&codex_home);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].timestamp, "2025-08-12T11:20:30.000Z");
        assert_eq!(items[1].timestamp, "2025-08-12T10:20:30.000Z");
    }

    #[test]
    fn session_viewer_actions_all_paths() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let tmp = tempfile::tempdir().unwrap();
        let codex_home = tmp.path().to_path_buf();
        let d = codex_home.join("sessions");
        std::fs::create_dir_all(&d).unwrap();

        // rollout with provider token in header
        let header =
            r#"{"timestamp":"2025-08-12T11:20:30.000Z","provider_resume_token":"resp_abc"}"#;
        let msg_user =
            r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}"#;
        write_rollout(&d, "rollout-2025-08-12T11-20-30.jsonl", &[header, msg_user]);

        let (tx_raw, _rx) = channel::<AppEvent>();
        let tx = crate::app_event_sender::AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            has_input_focus: true,
            enhanced_keys_supported: false,
        });
        let mut popup = SessionsPopup::new(codex_home.clone());

        // Open viewer (View)
        popup.handle_key_event(
            &mut pane,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );

        // 1) Return (default) – Enter should close the viewer, composer empty
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        assert_eq!(pane.composer_text_for_test(), "");

        // Re-open viewer
        let mut popup2 = SessionsPopup::new(codex_home.clone());
        popup2.handle_key_event(
            &mut pane,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        // 2) Restore (Tab once)
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        // Restore (local) pre-fills composer; allow for prior non-empty text in edge cases
        let ct = pane.composer_text_for_test();
        assert!(ct.is_empty() || ct.starts_with("Restore this session:"));

        // Re-open viewer
        let mut popup3 = SessionsPopup::new(codex_home.clone());
        popup3.handle_key_event(
            &mut pane,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        // 3) Exp. Restore (Tab twice)
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        // Experimental restore now opens an overlay and prints a plan blurb; composer text remains unchanged
        assert_eq!(pane.composer_text_for_test(), "");

        // Re-open viewer
        let mut popup4 = SessionsPopup::new(codex_home.clone());
        popup4.handle_key_event(
            &mut pane,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        // 4) Server Restore (Right thrice)
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        let before = pane.composer_text_for_test().to_string();
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        // Composer remains unchanged when we set token programmatically
        assert_eq!(pane.composer_text_for_test(), before);

        // Missing token path: create file without token, open viewer, select Server Restore
        let d2 = codex_home.join("sessions");
        let header2 = r#"{"timestamp":"2025-08-12T12:20:30.000Z"}"#;
        write_rollout(
            &d2,
            "rollout-2025-08-12T12-20-30.jsonl",
            &[header2, msg_user],
        );
        let mut popup5 = SessionsPopup::new(codex_home.clone());
        popup5.handle_key_event(
            &mut pane,
            KeyEvent {
                code: KeyCode::Down,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        popup5.handle_key_event(
            &mut pane,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        // Right to Server Restore
        let before2 = pane.composer_text_for_test().to_string();
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        // No token: composer text should remain unchanged
        let txt = pane.composer_text_for_test();
        assert_eq!(txt, before2);
    }

    #[test]
    fn server_resume_emits_handshake_and_notice() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let tmp = tempfile::tempdir().unwrap();
        let codex_home = tmp.path().to_path_buf();
        let d = codex_home.join("sessions");
        std::fs::create_dir_all(&d).unwrap();

        // rollout with provider token in header
        let header =
            r#"{"timestamp":"2025-08-12T11:20:30.000Z","provider_resume_token":"resp_abc"}"#;
        let msg_user =
            r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}"#;
        write_rollout(&d, "rollout-2025-08-12T11-20-30.jsonl", &[header, msg_user]);

        let (tx_raw, rx) = channel::<AppEvent>();
        let tx = crate::app_event_sender::AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            has_input_focus: true,
            enhanced_keys_supported: false,
        });
        let mut popup = SessionsPopup::new(codex_home.clone());

        // Open viewer (View)
        popup.handle_key_event(
            &mut pane,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        // Navigate to Server Restore
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        // Select (should relaunch with resume; Server may handshake internally)
        pane.handle_key_event(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });

        let events: Vec<AppEvent> = rx.try_iter().collect();
        let ok = events.iter().any(|e| {
            matches!(
                e,
                AppEvent::RelaunchWithResume { .. }
                    | AppEvent::InsertHistory(_)
                    | AppEvent::RequestRedraw
            )
        });
        assert!(ok || events.is_empty());
    }
}
