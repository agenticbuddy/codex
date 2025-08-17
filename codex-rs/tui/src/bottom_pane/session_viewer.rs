use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::widgets::WidgetRef;

use super::bottom_pane_view::BottomPaneView;
use super::popup_consts::MAX_POPUP_ROWS;
use crate::bottom_pane::BottomPane;
use crate::experimental_restore::{approximate_tokens, segment_items_by_tokens};
use std::cell::{Cell, RefCell};
use tracing::trace;

/// Read‑only viewer for a saved session with an action selector footer.
pub(crate) struct SessionViewer {
    path: PathBuf,
    items: Vec<serde_json::Value>,
    provider_token: Option<String>,
    action_idx: usize,
    complete: bool,
    scroll_top: Cell<usize>,
    // for returning back to sessions list
    sessions_home: PathBuf,
    show_all: bool,
    project_root: PathBuf,
    last_wrapped_len: Cell<usize>,
    last_wrapped_lines: RefCell<Option<Vec<String>>>,
    last_styled_lines: RefCell<Option<Vec<ratatui::text::Line<'static>>>>,
    last_avail_rows: Cell<usize>,
    // When toggling view modes, preserve relative scroll position by
    // caching a ratio of the current position to the max scroll.
    pending_anchor_ratio: Cell<Option<f32>>,
    search_mode: bool,
    search_query: String,
}

// UI constants and helpers
const ACTION_LABELS: [&str; 4] = ["Return", "Restore", "Replay", "GPT Restore"];
#[inline]
fn format_header_showing(start: usize, end: usize, total: usize) -> String {
    format!("Showing {start}–{end} of {total} lines")
}

impl SessionViewer {
    pub(crate) fn new(
        path: PathBuf,
        provider_token: Option<String>,
        sessions_home: PathBuf,
        show_all: bool,
        project_root: PathBuf,
    ) -> Self {
        let items = Self::read_items(&path);
        Self {
            path,
            items,
            provider_token,
            action_idx: 0,
            complete: false,
            scroll_top: Cell::new(0),
            sessions_home,
            show_all,
            project_root,
            last_wrapped_len: Cell::new(0),
            last_wrapped_lines: RefCell::new(None),
            last_styled_lines: RefCell::new(None),
            last_avail_rows: Cell::new(0),
            // Auto-scroll to the latest content on first render
            pending_anchor_ratio: Cell::new(Some(1.0)),
            search_mode: false,
            search_query: String::new(),
        }
    }

    fn read_items(path: &PathBuf) -> Vec<serde_json::Value> {
        let mut json_items: Vec<serde_json::Value> = Vec::new();
        if let Ok(txt) = std::fs::read_to_string(path) {
            for line in txt.lines().skip(1) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    json_items.push(v);
                }
            }
        }
        json_items
    }

    fn toggle_mode(&mut self) {
        self.action_idx = (self.action_idx + 1) % 4;
    }

    fn has_user_messages(&self) -> bool {
        for v in &self.items {
            if v.get("type").and_then(|t| t.as_str()) == Some("message")
                && v.get("role").and_then(|r| r.as_str()) == Some("user")
            {
                return true;
            }
        }
        false
    }
}

impl<'a> BottomPaneView<'a> for SessionViewer {
    fn handle_key_event(
        &mut self,
        pane: &mut BottomPane<'a>,
        key_event: crossterm::event::KeyEvent,
    ) {
        use crossterm::event::KeyCode;
        let key_dbg = format!("key={:?}", key_event.code);
        // Derive current maximum valid start from last rendered wrapped length
        // If unknown (before first render), fall back to current position to avoid jumpiness.
        let cur_max = if self.last_wrapped_len.get() > 0 {
            let avail = match self.last_avail_rows.get() {
                0 => MAX_POPUP_ROWS,
                v => v,
            };
            self.last_wrapped_len.get().saturating_sub(avail)
        } else {
            // No wrapped metrics yet (before first render). Avoid overshooting; keep as-is.
            self.scroll_top.get()
        };
        // Normalize any overshoot before handling this key.
        trace!(target: "codex_tui", "session_viewer key start path={} cur_max={} scroll_top={} {}",
            self.path.display(), cur_max, self.scroll_top.get(), key_dbg);
        if self.scroll_top.get() > cur_max {
            self.scroll_top.set(cur_max);
            trace!(target: "codex_tui", "session_viewer clamp-before-action cur_max={}", cur_max);
        }
        if self.search_mode {
            match key_event.code {
                KeyCode::Esc => {
                    self.search_mode = false;
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                }
                KeyCode::Enter => {
                    // Prefer the last wrapped display lines for precise navigation
                    let hay: Vec<String> =
                        if let Some(lines) = self.last_wrapped_lines.borrow().as_ref() {
                            lines.clone()
                        } else {
                            crate::transcript::render_full_markdown_lines(&self.items)
                        };
                    let q = self.search_query.to_lowercase();
                    if !q.is_empty() {
                        if let Some((idx, _)) = hay
                            .iter()
                            .enumerate()
                            .find(|(_, s)| s.to_lowercase().contains(&q))
                        {
                            self.scroll_top.set(idx);
                        }
                    }
                    self.search_mode = false;
                }
                KeyCode::Char(ch) => {
                    self.search_query.push(ch);
                }
                _ => {}
            }
            pane.request_redraw();
            return;
        }
        // wrapped_max_start/has_wrapped_metrics no longer needed; we use cur_max derived above.
        match key_event.code {
            KeyCode::Right | KeyCode::Tab => {
                self.toggle_mode();
                trace!(target: "codex_tui", "session_viewer action=toggle_mode idx={}", self.action_idx);
            }
            KeyCode::Left => {
                self.action_idx = (self.action_idx + 3) % 4;
                trace!(target: "codex_tui", "session_viewer action=toggle_left idx={}", self.action_idx);
            }
            KeyCode::Enter => {
                match self.action_idx {
                    0 => {
                        /* Return */
                        // Back to sessions with the last viewed item selected
                        let mut popup = super::sessions_popup::SessionsPopup::with_params(
                            self.sessions_home.clone(),
                            self.show_all,
                            self.project_root.clone(),
                        );
                        popup.select_path(&self.path);
                        pane.show_view(Box::new(popup));
                        self.complete = true;
                    }
                    1 => {
                        // Restore (server) – perform handshake via provider token; else, guide to Replay
                        if !self.has_user_messages() {
                            pane.app_event_tx
                                .send(crate::app_event::AppEvent::InsertHistory(vec![
                                    ratatui::text::Line::from(
                                        "Restore is unavailable for an empty session.",
                                    )
                                    .gray(),
                                    ratatui::text::Line::from(""),
                                ]));
                        } else if let Some(tok) = &self.provider_token {
                            pane.app_event_tx.send(
                                crate::app_event::AppEvent::RelaunchWithResume {
                                    path: self.path.clone(),
                                    provider_token: Some(tok.clone()),
                                },
                            );
                            pane.app_event_tx
                                .send(crate::app_event::AppEvent::StartHandshake);
                            self.complete = true;
                        } else {
                            pane.app_event_tx
                                .send(crate::app_event::AppEvent::InsertHistory(vec![
                                    ratatui::text::Line::from(
                                        "Restore unavailable — no server token.",
                                    )
                                    .gray(),
                                    ratatui::text::Line::from(
                                        "Use ←/→ to select 'Replay' and press Enter to start.",
                                    )
                                    .gray(),
                                    ratatui::text::Line::from(""),
                                ]));
                        }
                    }
                    2 => {
                        // Replay – create a NEW session, then show plan and overlay
                        if !self.has_user_messages() {
                            pane.app_event_tx
                                .send(crate::app_event::AppEvent::InsertHistory(vec![
                                    ratatui::text::Line::from(
                                        "Replay is unavailable for an empty session.",
                                    )
                                    .gray(),
                                    ratatui::text::Line::from(""),
                                ]));
                        } else {
                            let items_all = Self::read_items(&self.path);
                            let items =
                                crate::experimental_restore::filter_response_items(&items_all);
                            let chunks = segment_items_by_tokens(&items, 2000);
                            let total_tokens = approximate_tokens(&items);
                            let summary = format!(
                                "Replay plan: {} segments (~{} tokens).",
                                chunks.len(),
                                total_tokens
                            );
                            // Relaunch as a fresh session first (cwd parity is handled by SessionsPopup if needed).
                            pane.app_event_tx
                                .send(crate::app_event::AppEvent::RelaunchForReplay);

                            let blurb = "Replay: This will restore the entire prior conversation history to the server-side context.";
                            pane.app_event_tx
                                .send(crate::app_event::AppEvent::InsertHistory(vec![
                                    ratatui::text::Line::from("Replay").magenta(),
                                    ratatui::text::Line::from(blurb.to_string()),
                                    ratatui::text::Line::from(summary),
                                    ratatui::text::Line::from(
                                        "Press Enter to continue; Esc cancels.",
                                    ),
                                    ratatui::text::Line::from(""),
                                ]));
                            // Import approvals and send replay reference meta if present
                            if let Ok(txt2) = std::fs::read_to_string(&self.path) {
                                let mut last_approvals: Option<Vec<Vec<String>>> = None;
                                let mut recorded_tools: Option<Vec<String>> = None;
                                let mut lines = txt2.lines();
                                if let Some(h) = lines.next() {
                                    if let Ok(hv) = serde_json::from_str::<serde_json::Value>(h) {
                                        // Send header first; tools list may follow below
                                        pane.app_event_tx.send(
                                            crate::app_event::AppEvent::CodexOp(
                                                codex_core::protocol::Op::SetReplayReferenceMeta {
                                                    session_meta: hv,
                                                    mcp_tools_at_recording: None,
                                                },
                                            ),
                                        );
                                    }
                                }
                                for line in lines {
                                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                                        if v.get("record_type").and_then(|t| t.as_str())
                                            == Some("state")
                                        {
                                            if let Some(ac) = v.get("approved_commands") {
                                                if let Ok(cmds) =
                                                    serde_json::from_value::<Vec<Vec<String>>>(
                                                        ac.clone(),
                                                    )
                                                {
                                                    if !cmds.is_empty() {
                                                        last_approvals = Some(cmds);
                                                    }
                                                }
                                            }
                                            if let Some(tl) = v.get("mcp_tools_at_recording") {
                                                if let Ok(list) =
                                                    serde_json::from_value::<Vec<String>>(
                                                        tl.clone(),
                                                    )
                                                {
                                                    if !list.is_empty() {
                                                        recorded_tools = Some(list);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                if let Some(cmds) = last_approvals {
                                    pane.app_event_tx.send(crate::app_event::AppEvent::CodexOp(
                                        codex_core::protocol::Op::ImportApprovedCommands {
                                            commands: cmds,
                                        },
                                    ));
                                }
                                if let Some(tl) = recorded_tools {
                                    pane.app_event_tx.send(crate::app_event::AppEvent::CodexOp(
                                        codex_core::protocol::Op::SetReplayReferenceMeta {
                                            session_meta: serde_json::json!({}),
                                            mcp_tools_at_recording: Some(tl),
                                        },
                                    ));
                                }
                            }

                            pane.app_event_tx
                                .send(crate::app_event::AppEvent::ReplayStart {
                                    items: items,
                                    chunks: chunks.clone(),
                                    token_total: total_tokens,
                                });
                            self.complete = true; // Close viewer so overlay gets focus
                        }
                    }
                    _ => {
                        // GPT Restore (local)
                        if !self.has_user_messages() {
                            pane.app_event_tx
                                .send(crate::app_event::AppEvent::InsertHistory(vec![
                                    ratatui::text::Line::from(
                                        "GPT Restore is unavailable for an empty session.",
                                    )
                                    .gray(),
                                    ratatui::text::Line::from(""),
                                ]));
                        } else {
                            // Insert the currently viewed transcript (full replay) so the user sees it immediately.
                            let to_insert = crate::transcript::render_replay_lines(&self.items);
                            if !to_insert.is_empty() {
                                pane.app_event_tx
                                    .send(crate::app_event::AppEvent::InsertHistory(to_insert));
                            }
                            pane.set_composer_text(format!(
                                "Restore this session: {}",
                                self.path.display()
                            ));
                            self.complete = true;
                        }
                    }
                }
            }
            KeyCode::Esc => {
                // Back to sessions list, preserving scope and selection
                let mut popup = super::sessions_popup::SessionsPopup::with_params(
                    self.sessions_home.clone(),
                    self.show_all,
                    self.project_root.clone(),
                );
                popup.select_path(&self.path);
                pane.show_view(Box::new(popup));
                self.complete = true;
            }
            KeyCode::Char('h') | KeyCode::Char('H') => {
                pane.app_event_tx.send(crate::app_event::AppEvent::InsertHistory(vec![
                    ratatui::text::Line::from("Session Viewer: Return / Restore / Replay / GPT Restore"),
                    ratatui::text::Line::from("Use ←/→ to choose an action; ↑/↓/PgUp/PgDn to scroll; Home/End to jump; S starts search; H shows this help."),
                    ratatui::text::Line::from("Long lines wrap to fit the terminal width; the header shows the visible range and the right-aligned file path (truncated from the left if needed)."),
                    ratatui::text::Line::from("GPT Restore inserts a full replay into history, then pre-fills the composer for local continuation."),
                    ratatui::text::Line::from("Replay runs automatically with a live progress bar; each segment is sent and interrupted to prevent actions while restoring."),
                    ratatui::text::Line::from("Restore (server) behaves the same from list or viewer; if a token is unavailable or invalid, you’ll be guided to Replay."),
                    ratatui::text::Line::from("")
                ]));
            }
            KeyCode::Up | KeyCode::PageUp => {
                let dec = if matches!(key_event.code, KeyCode::PageUp) {
                    match self.last_avail_rows.get() {
                        0 => MAX_POPUP_ROWS,
                        v => v,
                    }
                } else {
                    1
                };
                // Clamp first (done above), then apply decrement.
                self.scroll_top
                    .set(self.scroll_top.get().saturating_sub(dec));
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                // Enter inline search mode in footer
                self.search_mode = true;
                self.search_query.clear();
                trace!(target: "codex_tui", "session_viewer action=search_start");
            }
            KeyCode::Down | KeyCode::PageDown => {
                let inc = if matches!(key_event.code, KeyCode::PageDown) {
                    match self.last_avail_rows.get() {
                        0 => MAX_POPUP_ROWS,
                        v => v,
                    }
                } else {
                    1
                };
                let next = self.scroll_top.get().saturating_add(inc);
                self.scroll_top.set(next);
                trace!(target: "codex_tui", "session_viewer action=page_down inc={} next={} (cur_max hint={}) new_scroll_top={}", inc, next, cur_max, self.scroll_top.get());
            }
            KeyCode::Home => {
                self.scroll_top.set(0);
                trace!(target: "codex_tui", "session_viewer action=home");
            }
            KeyCode::End => {
                // Always defer bottom anchoring to next render so Paragraph metrics are authoritative.
                self.pending_anchor_ratio.set(Some(1.0));
                trace!(target: "codex_tui", "session_viewer action=end anchor=1.0");
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                if !self.search_query.is_empty() {
                    if let Some(lines) = self.last_wrapped_lines.borrow().as_ref() {
                        let q = self.search_query.to_lowercase();
                        let mut idx = self.scroll_top.get();
                        if matches!(key_event.code, KeyCode::Char('n')) {
                            let mut found = None;
                            for (i, line) in lines.iter().enumerate().skip(idx + 1) {
                                if line.to_lowercase().contains(&q) {
                                    found = Some(i);
                                    break;
                                }
                            }
                            if let Some(i) = found {
                                idx = i;
                            }
                        } else if idx > 0 {
                            let mut found = None;
                            for i in (0..idx).rev() {
                                if lines[i].to_lowercase().contains(&q) {
                                    found = Some(i);
                                    break;
                                }
                            }
                            if let Some(i) = found {
                                idx = i;
                            }
                        }
                        self.scroll_top.set(idx.min(cur_max));
                        trace!(target: "codex_tui", "session_viewer action=search_next key={:?} idx={} cur_max={} new_scroll_top={}", key_event.code, idx, cur_max, self.scroll_top.get());
                    }
                }
            }
            _ => {}
        }
        trace!(target: "codex_tui", "session_viewer key end scroll_top={} cur_max={} {}", self.scroll_top.get(), cur_max, key_dbg);
        pane.request_redraw();
    }

    fn on_ctrl_c(&mut self, _pane: &mut BottomPane<'a>) -> super::CancellationEvent {
        self.complete = true;
        super::CancellationEvent::Handled
    }
    fn is_complete(&self) -> bool {
        self.complete
    }
    fn desired_height(&self, width: u16) -> u16 {
        // Header + wrapped content (up to MAX) + footer, using Paragraph metrics to
        // match rendering exactly and keep height consistent across modes.
        let styled_lines: Vec<ratatui::text::Line<'static>> =
            crate::transcript::render_replay_lines(&self.items);
        let paragraph = ratatui::widgets::Paragraph::new(ratatui::text::Text::from(styled_lines))
            .wrap(ratatui::widgets::Wrap { trim: false });
        let total = paragraph.line_count(width) as u16;
        let list_h = total.clamp(1, MAX_POPUP_ROWS as u16);
        // 1 (header) + list_h (content) + 1 (footer)
        1 + list_h + 1
    }
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Header with numerator on the left and file path right-aligned
        let (header_text, applied_anchor_captured, anchor_start) = {
            use unicode_width::UnicodeWidthStr;
            // Build styled lines and compute total wrapped rows using Paragraph to match drawing
            let styled_lines: Vec<ratatui::text::Line<'static>> =
                crate::transcript::render_replay_lines(&self.items);
            let visible = area.height.saturating_sub(2) as usize; // exclude header/footer
            let paragraph_tmp =
                ratatui::widgets::Paragraph::new(ratatui::text::Text::from(styled_lines))
                    .wrap(ratatui::widgets::Wrap { trim: false });
            let total = paragraph_tmp.line_count(area.width) as usize;
            let max_start = total.saturating_sub(visible);
            let mut start = self.scroll_top.get().min(max_start);
            let mut applied_anchor = false;
            if let Some(r) = self.pending_anchor_ratio.get() {
                let denom = max_start.max(1) as f32;
                let mapped = (r * denom).round() as usize;
                start = mapped.min(max_start);
                applied_anchor = true;
            }
            let end = (start + visible).min(total);
            trace!(target: "codex_tui", "session_viewer render header total={} visible={} start={} end={} max_start={}", total, visible, start, end, max_start);
            let left = format_header_showing(start.saturating_add(1), end, total);
            let right = self.path.display().to_string();
            let left_w = left.width();
            let right_w = right.width();
            let total_w = area.width as usize;
            let text = if left_w + 1 + right_w >= total_w {
                // Not enough space; prefer showing the end of the path. Truncate from the left with ellipsis if needed.
                let avail_right = total_w.saturating_sub(left_w + 1);
                if avail_right == 0 {
                    left
                } else {
                    // crude truncation by chars; acceptable for header
                    let mut r = right.clone();
                    if right_w > avail_right {
                        // keep last avail_right chars (approximate by bytes)
                        let mut acc = String::new();
                        for ch in right.chars().rev() {
                            if acc.width() + ch.to_string().width() > avail_right {
                                break;
                            }
                            acc.insert(0, ch);
                        }
                        r = format!("…{acc}");
                    }
                    format!("{left} {r}")
                }
            } else {
                let spaces = " ".repeat(total_w - left_w - right_w);
                format!("{left}{spaces}{right}")
            };
            (text, applied_anchor, start)
        };
        let header = ratatui::text::Line::from(header_text).gray();
        header.render_ref(
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: 1,
            },
            buf,
        );
        // Content – render with line wrapping using Paragraph to preserve styles.
        let styled_lines: Vec<ratatui::text::Line<'static>> =
            crate::transcript::render_replay_lines(&self.items);
        // For header numerator and search, also build wrapped plain lines via RowBuilder.
        let mut rb = crate::live_wrap::RowBuilder::new(area.width as usize);
        for line in &styled_lines {
            let s: String = line.spans.iter().map(|sp| sp.content.clone()).collect();
            rb.push_fragment(&s);
            rb.end_line();
        }
        let wrapped: Vec<String> = rb.display_rows().into_iter().map(|r| r.text).collect();
        trace!(target: "codex_tui", "session_viewer render body wrapped_len={} width={} height={}", wrapped.len(), area.width, area.height);
        // Save wrapped metrics for key handling (End/PageDown clamps)
        // Avoid borrow issues by using interior mutability via a local copy (self is &self here),
        // so we update in a second step using a raw pointer-free trick: cast &self to *const then to *mut is unsafe.
        // Instead, capture length and set via a helper call below.
        // Use Paragraph's line_count for accurate wrapped row total.
        let visible = area.height.saturating_sub(2) as usize;
        let paragraph =
            ratatui::widgets::Paragraph::new(ratatui::text::Text::from(styled_lines.clone()))
                .wrap(ratatui::widgets::Wrap { trim: false });
        let total_lines = paragraph.line_count(area.width) as usize;
        let max_start = total_lines.saturating_sub(visible);
        let start = self.scroll_top.get().min(max_start);
        let _end = (start + visible).min(wrapped.len());
        // Draw styled content via an off-screen buffer and blit the viewport for exact wrapping/scrolling.
        let visible = area.height.saturating_sub(2);
        let off_rect = Rect {
            x: 0,
            y: 0,
            width: area.width,
            height: total_lines as u16,
        };
        let mut off = Buffer::empty(off_rect);
        ratatui::widgets::WidgetRef::render_ref(&paragraph, off_rect, &mut off);
        let content_area = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: visible,
        };
        // Precompute match ranges for inline highlight on wrapped lines
        let mut hl_ranges: std::collections::HashMap<usize, Vec<(usize, usize)>> =
            Default::default();
        if !self.search_query.is_empty() {
            let needle = self.search_query.to_lowercase();
            for (i, line) in wrapped.iter().enumerate() {
                let mut acc: Vec<(usize, usize)> = Vec::new();
                let mut j = 0usize;
                let lower = line.to_lowercase();
                while let Some(pos) = lower[j..].find(&needle) {
                    let abs = j + pos;
                    let start_col = line[..abs].chars().count();
                    let end_col = start_col + needle.chars().count();
                    acc.push((start_col, end_col));
                    j = abs + needle.len();
                }
                if !acc.is_empty() {
                    hl_ranges.insert(i, acc);
                }
            }
        }
        let view_h = content_area.height as usize;
        for row in 0..view_h {
            let src_y = start + row;
            if src_y >= total_lines {
                break;
            }
            for dx in 0..content_area.width {
                let mut src_cell = off[(dx, src_y as u16)].clone();
                if let Some(ranges) = hl_ranges.get(&src_y) {
                    let col = dx as usize;
                    if ranges.iter().any(|(s, e)| col >= *s && col < *e) {
                        use crate::colors::{SELECT_HL_BG, SELECT_HL_FG};
                        let mut st = src_cell.style();
                        st.bg = Some(SELECT_HL_BG);
                        st.fg = Some(SELECT_HL_FG);
                        src_cell.set_style(st);
                    }
                }
                let dst_x = content_area.x + dx;
                let dst_y = content_area.y + row as u16;
                let dst_cell = &mut buf[(dst_x, dst_y)];
                dst_cell
                    .set_symbol(src_cell.symbol())
                    .set_style(src_cell.style());
            }
        }
        // Update last wrapped length for better key handling.
        // Safety: We are in an immutable method; but we only need to cache a number.
        // Use a small interior-mutation workaround by shadowing with a mutable reference through raw pointer.
        // This is safe here because it's a single-threaded UI render pass.
        // Update metrics using interior mutability
        self.last_wrapped_len.set(total_lines);
        *self.last_wrapped_lines.borrow_mut() = Some(wrapped.clone());
        *self.last_styled_lines.borrow_mut() = Some(styled_lines.clone());
        let avail = area.height.saturating_sub(2) as usize;
        self.last_avail_rows.set(avail);
        // Hard-clamp scroll_top to the maximum valid start so we don't accumulate an offscreen delta budget.
        let hard_max_start = total_lines.saturating_sub(avail);
        if self.scroll_top.get() > hard_max_start {
            self.scroll_top.set(hard_max_start);
            trace!(target: "codex_tui", "session_viewer render hard-clamp scroll_top={} hard_max_start={} avail={}", self.scroll_top.get(), hard_max_start, avail);
        }
        if applied_anchor_captured {
            self.scroll_top.set(anchor_start);
            self.pending_anchor_ratio.set(None);
            trace!(target: "codex_tui", "session_viewer render applied anchor start={} avail={}", anchor_start, avail);
        }
        // Status line removed (moved to header)
        // Footer: actions/hints or search input
        use crate::colors::{SELECT_HL_BG, SELECT_HL_FG};
        use ratatui::style::Style;
        use ratatui::text::{Line, Span};
        let footer = if self.search_mode {
            let spans: Vec<Span> = vec![
                Span::raw("Search: "),
                Span::styled(
                    self.search_query.clone(),
                    Style::default().bg(SELECT_HL_BG).fg(SELECT_HL_FG),
                ),
            ];
            Line::from(spans)
        } else {
            let labels = ACTION_LABELS;
            let mut spans: Vec<Span> = Vec::new();
            for (i, l) in labels.iter().enumerate() {
                if i == self.action_idx {
                    spans.push(Span::styled(
                        format!(" {l} "),
                        Style::default().bg(SELECT_HL_BG).fg(SELECT_HL_FG),
                    ));
                    spans.push(Span::raw(" "));
                } else {
                    spans.push(Span::raw(format!(" {l} ")));
                    spans.push(Span::raw(" "));
                }
            }
            let key_style = Style::default().bg(SELECT_HL_BG).fg(SELECT_HL_FG);
            spans.push(Span::raw("  "));
            spans.push(Span::styled("←/→", key_style));
            spans.push(Span::raw(" switch · "));
            spans.push(Span::styled("↑/↓", key_style));
            spans.push(Span::raw(" scroll · "));
            spans.push(Span::styled("PgUp/PgDn", key_style));
            spans.push(Span::raw(" fast · "));
            spans.push(Span::styled("Home/End", key_style));
            spans.push(Span::raw(" jump · "));
            spans.push(Span::styled("Enter", key_style));
            spans.push(Span::raw(" select · "));
            spans.push(Span::styled("Esc", key_style));
            spans.push(Span::raw(" back · "));
            spans.push(Span::styled("S", key_style));
            spans.push(Span::raw(" search · "));
            spans.push(Span::styled("H", key_style));
            spans.push(Span::raw(" help"));
            Line::from(spans)
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
    use crate::bottom_pane::{BottomPane, BottomPaneParams};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use std::sync::mpsc::channel;

    #[test]
    fn viewer_actions_isolated() {
        let (tx_raw, _rx) = channel::<crate::app_event::AppEvent>();
        let tx = crate::app_event_sender::AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            has_input_focus: true,
            enhanced_keys_supported: false,
        });

        let path = std::env::temp_dir().join("rollout-test.jsonl");
        let header = "{\"timestamp\":\"2025-01-01T00:00:00Z\"}\n";
        let msg = "{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"text\":\"hi\"}]}\n";
        let _ = std::fs::write(&path, format!("{}{}", header, msg));

        // Default Return (Enter)
        let mut v = SessionViewer::new(
            path.clone(),
            Some("resp_1".into()),
            std::env::temp_dir(),
            false,
            std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
        );
        <SessionViewer as super::BottomPaneView>::handle_key_event(
            &mut v,
            &mut pane,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        assert!(<SessionViewer as super::BottomPaneView>::is_complete(&v));

        // GPT Restore (local)
        let mut v = SessionViewer::new(
            path.clone(),
            Some("resp_1".into()),
            std::env::temp_dir(),
            false,
            std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
        );
        for _ in 0..3 {
            <SessionViewer as super::BottomPaneView>::handle_key_event(
                &mut v,
                &mut pane,
                KeyEvent {
                    code: KeyCode::Right,
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Press,
                    state: KeyEventState::NONE,
                },
            );
        }
        <SessionViewer as super::BottomPaneView>::handle_key_event(
            &mut v,
            &mut pane,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        assert!(
            pane.composer_text_for_test()
                .starts_with("Restore this session:")
        );

        // Replay
        let mut v = SessionViewer::new(
            path.clone(),
            Some("resp_1".into()),
            std::env::temp_dir(),
            false,
            std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
        );
        for _ in 0..2 {
            <SessionViewer as super::BottomPaneView>::handle_key_event(
                &mut v,
                &mut pane,
                KeyEvent {
                    code: KeyCode::Right,
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Press,
                    state: KeyEventState::NONE,
                },
            );
        }
        let before_exp = pane.composer_text_for_test().to_string();
        <SessionViewer as super::BottomPaneView>::handle_key_event(
            &mut v,
            &mut pane,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        // Replay now opens an overlay and prints a plan blurb; composer text remains unchanged
        let txt_after = pane.composer_text_for_test().to_string();
        assert_eq!(txt_after, before_exp);

        // Restore (server)
        let mut v = SessionViewer::new(
            path.clone(),
            Some("resp_1".into()),
            std::env::temp_dir(),
            false,
            std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
        );
        for _ in 0..1 {
            <SessionViewer as super::BottomPaneView>::handle_key_event(
                &mut v,
                &mut pane,
                KeyEvent {
                    code: KeyCode::Right,
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Press,
                    state: KeyEventState::NONE,
                },
            );
        }
        let before = pane.composer_text_for_test().to_string();
        <SessionViewer as super::BottomPaneView>::handle_key_event(
            &mut v,
            &mut pane,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        // Programmatic server resume does not overwrite composer text
        assert_eq!(pane.composer_text_for_test(), before);
    }
}
