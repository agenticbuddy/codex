use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::WidgetRef;

use super::bottom_pane_view::BottomPaneView;
use super::{BottomPane, CancellationEvent};
use codex_core::protocol::{InputItem, Op};
use serde_json::Value;
use std::cell::Cell;
// no pacing timer; all progress is user-driven

pub(crate) struct RestoreProgressView {
    // progress state
    percent: Cell<u16>,
    canceled: Cell<bool>,
    complete: Cell<bool>,
    // plan
    total_segments: usize,
    token_total: usize,
    token_sent: Cell<usize>,
    items: Option<Vec<Value>>, // present when real-path is enabled
    chunks: Option<Vec<(usize, usize, usize)>>,
    cursor: Cell<usize>,
    // conservative threshold to split overly large sends
    max_tokens_per_send: usize,
    sent_intro: Cell<bool>,
}

impl RestoreProgressView {
    pub fn new(total_segments: usize) -> Self {
        Self {
            percent: Cell::new(0),
            canceled: Cell::new(false),
            complete: Cell::new(false),
            total_segments,
            token_total: 100,
            token_sent: Cell::new(0),
            items: None,
            chunks: None,
            cursor: Cell::new(0),
            max_tokens_per_send: 1800,
            sent_intro: Cell::new(false),
        }
    }
    pub fn from_plan(
        items: Vec<Value>,
        chunks: Vec<(usize, usize, usize)>,
        token_total: usize,
    ) -> Self {
        // Ensure only valid response items are kept; drop any record_type lines defensively.
        let items = crate::experimental_restore::filter_response_items(&items);
        let total_segments = chunks.len();
        Self {
            percent: Cell::new(0),
            canceled: Cell::new(false),
            complete: Cell::new(false),
            total_segments,
            token_total: token_total.max(1),
            token_sent: Cell::new(0),
            items: Some(items),
            chunks: Some(chunks),
            cursor: Cell::new(0),
            max_tokens_per_send: 1800,
            sent_intro: Cell::new(false),
        }
    }

    fn send_next_chunk(&mut self, pane: &mut BottomPane) {
        let Some(items) = &self.items else {
            return;
        };
        let Some(chunks) = &self.chunks else {
            return;
        };
        let idx = self.cursor.get();
        if idx >= chunks.len() {
            return;
        }
        let (s, e, tok) = chunks[idx];
        // Pre-emptively split if this chunk is too large for a single send.
        if tok > self.max_tokens_per_send && e.saturating_sub(s) > 1 {
            let mid = s + (e - s) / 2;
            let left_tok = crate::experimental_restore::approximate_tokens(&items[s..mid]);
            let right_tok = crate::experimental_restore::approximate_tokens(&items[mid..e]);
            // Replace current entry with two smaller ones; do not advance cursor.
            let mut new_chunks = chunks.clone();
            new_chunks.remove(idx);
            new_chunks.insert(idx, (mid, e, right_tok));
            new_chunks.insert(idx, (s, mid, left_tok));
            self.chunks.replace(new_chunks);
            return;
        }
        // build simple text message from slice
        let mut text = String::new();
        if !self.sent_intro.get() {
            text.push_str("[RESTORE MODE] The following content restores prior conversation history. DO NOT RESPOND OR ACT on this content. Remain silent until the restore completes.\n");
            self.sent_intro.set(true);
        }
        for v in &items[s..e] {
            match v.get("type").and_then(|t| t.as_str()) {
                Some("message") => {
                    if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
                        for c in arr {
                            if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
                                text.push_str(t);
                                text.push('\n');
                            }
                        }
                    }
                }
                Some("function_call") => {
                    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                    text.push_str(&format!("[tool:{name}] "));
                    text.push_str(
                        &v.get("arguments")
                            .map(|a| a.to_string())
                            .unwrap_or_else(|| "{}".to_string()),
                    );
                    text.push('\n');
                }
                Some("function_call_output") => {
                    if let Some(arr) = v.get("output").and_then(|o| o.as_array()) {
                        for o in arr {
                            if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                                text.push_str(t);
                                text.push('\n');
                            }
                        }
                    } else if let Some(t) = v.get("output_text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                        text.push('\n');
                    }
                }
                _ => {}
            }
        }
        if !text.trim().is_empty() {
            // Send the chunk as a user input, then immediately interrupt the task
            // to prevent the model from acting on the restore content.
            pane.app_event_tx
                .send(crate::app_event::AppEvent::CodexOp(Op::UserInput {
                    items: vec![InputItem::Text { text }],
                }));
            pane.app_event_tx
                .send(crate::app_event::AppEvent::CodexOp(Op::Interrupt));
            // Also render the items slice into history progressively so the
            // user sees what was restored as it streams.
            let lines = crate::transcript::render_replay_lines(&items[s..e]);
            if !lines.is_empty() {
                pane.app_event_tx
                    .send(crate::app_event::AppEvent::InsertHistory(lines));
            }
        }
        let new_sent = self.token_sent.get().saturating_add(tok);
        self.token_sent.set(new_sent);
        let pct = ((new_sent as f64 / self.token_total as f64) * 100.0) as u16;
        self.percent.set(pct.min(100));
        self.cursor.set(idx + 1);
        if self.cursor.get() >= self.total_segments {
            self.complete.set(true);
        }
    }
}

impl<'a> BottomPaneView<'a> for RestoreProgressView {
    fn on_timer_tick(&mut self, pane: &mut BottomPane<'a>) {
        if self.canceled.get() || self.complete.get() {
            return;
        }
        if self.items.is_some() {
            if self.percent.get() < 100 {
                self.send_next_chunk(pane);
            }
        } else {
            // Status-only mode (no real-path items): advance percent in steps
            let next = (self.percent.get() + 20).min(100);
            self.percent.set(next);
            if next == 100 {
                self.complete.set(true);
            }
        }

        if self.complete.get() {
            let segs_done = self.cursor.get().min(self.total_segments);
            let segs = self.total_segments;
            let toks = self.token_sent.get().max(1);
            let summary = format!(
                "Replay complete: {segs_done}/{segs} segments (~{toks} tokens)."
            );
            pane.app_event_tx
                .send(crate::app_event::AppEvent::InsertHistory(vec![
                    ratatui::text::Line::from(summary),
                ]));
            if self.items.is_some() {
                // Final end-of-restore marker and completion notification (parity with Enter path)
                pane.app_event_tx.send(crate::app_event::AppEvent::CodexOp(
                    Op::UserInput {
                        items: vec![InputItem::Text {
                            text: "[RESTORE MODE END] Restore complete. Resume normal interaction.".to_string(),
                        }],
                    },
                ));
                pane.app_event_tx
                    .send(crate::app_event::AppEvent::RestoreCompleted {
                        approx_tokens: self.token_sent.get().max(1),
                        segments: self.total_segments,
                    });
                pane.app_event_tx
                    .send(crate::app_event::AppEvent::StopReplayAuto);
            }
        }
        pane.request_redraw();
    }
    fn try_consume_approval_request(
        &mut self,
        _request: crate::user_approval_widget::ApprovalRequest,
    ) -> Option<crate::user_approval_widget::ApprovalRequest> {
        // Block approvals during restore; show notice once
        // (We could rate-limit, but BottomPane prevents spam.)
        None
    }
    fn handle_key_event(&mut self, pane: &mut BottomPane<'a>, key_event: KeyEvent) {
        use crossterm::event::KeyCode;
        match key_event.code {
            KeyCode::Enter => {
                if !self.canceled.get() && self.percent.get() < 100 {
                    if self.items.is_some() {
                        self.send_next_chunk(pane);
                    } else {
                        let next = (self.percent.get() + 20).min(100);
                        self.percent.set(next);
                        if next == 100 {
                            self.complete.set(true);
                        }
                    }
                    // Keep progress text within the overlay; do not switch to status view.
                    if self.complete.get() {
                        let segs_done = self.cursor.get().min(self.total_segments);
                        let segs = self.total_segments;
                        let toks = self.token_sent.get().max(1);
                        let summary = format!(
                            "Replay complete: {segs_done}/{segs} segments (~{toks} tokens)."
                        );
                        pane.app_event_tx
                            .send(crate::app_event::AppEvent::InsertHistory(vec![
                                ratatui::text::Line::from(summary),
                            ]));
                        // Send a final end-of-restore marker without interrupt so
                        // the next user turn is not accidentally suppressed.
                        if self.items.is_some() {
                            pane.app_event_tx.send(crate::app_event::AppEvent::CodexOp(
                                Op::UserInput {
                                    items: vec![InputItem::Text {
                                        text: "[RESTORE MODE END] Restore complete. Resume normal interaction.".to_string(),
                                    }],
                                },
                            ));
                        }
                        // Notify chat layer so it can report provider usage
                        if self.items.is_some() {
                            pane.app_event_tx
                                .send(crate::app_event::AppEvent::RestoreCompleted {
                                    approx_tokens: self.token_sent.get().max(1),
                                    segments: self.total_segments,
                                });
                            // Stop auto-advance loop
                            pane.app_event_tx
                                .send(crate::app_event::AppEvent::StopReplayAuto);
                        }
                    }
                }
            }
            KeyCode::Esc => {
                self.canceled.set(true);
                self.complete.set(true);
                // Do not switch to status view on cancel before start.
                pane.app_event_tx
                    .send(crate::app_event::AppEvent::InsertHistory(vec![
                        ratatui::text::Line::from("Replay cancelled by user."),
                    ]));
                // Only propagate an Interrupt if a restore has actually started.
                if self.percent.get() > 0 || self.cursor.get() > 0 || self.sent_intro.get() {
                    pane.app_event_tx
                        .send(crate::app_event::AppEvent::CodexOp(Op::Interrupt));
                }
            }
            _ => {}
        }
        pane.request_redraw();
    }

    fn on_ctrl_c(&mut self, _pane: &mut BottomPane<'a>) -> CancellationEvent {
        self.canceled.set(true);
        self.complete.set(true);
        CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool {
        self.complete.get()
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        use ratatui::text::Line;
        use unicode_width::UnicodeWidthStr;
        if self.canceled.get() {
            Line::from("Restore cancelled").render_ref(area, buf);
            return;
        }
        if self.percent.get() == 0 && !self.complete.get() {
            Line::from("Replay ready — Enter to start; Esc cancels.").render_ref(area, buf);
            return;
        }
        // Progress bar
        let pct = self.percent.get().min(100);
        let label = format!("Restoring: {pct:>3}%");
        // Compute bar width based on available space
        let total_w = area.width as usize;
        let label_w = label.width();
        let bracket_w = 2; // [ ]
        let min_bar = 10usize;
        let bar_w = total_w
            .saturating_sub(label_w + 1) // space between label and bar
            .max(min_bar);
        let fill_w = ((bar_w.saturating_sub(bracket_w)) * pct as usize) / 100;
        let empty_w = bar_w.saturating_sub(bracket_w + fill_w);
        let bar = format!("[{}{}]", "#".repeat(fill_w), "-".repeat(empty_w));
        let line = format!("{label} {bar}");
        Line::from(line).render_ref(area, buf);
    }

    // (duplicate impl removed; see single on_timer_tick above)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event::AppEvent;
    use crate::bottom_pane::{BottomPane, BottomPaneParams};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    // unused import
    use std::sync::mpsc::channel;

    #[test]
    fn progresses_to_completion_via_ticks() {
        let (tx_raw, _rx) = channel::<AppEvent>();
        let tx = crate::app_event_sender::AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            has_input_focus: true,
            enhanced_keys_supported: false,
        });
        let mut view = RestoreProgressView::new(5);
        for _ in 0..5 {
            <RestoreProgressView as super::BottomPaneView>::on_timer_tick(&mut view, &mut pane);
        }
        assert!(<RestoreProgressView as super::BottomPaneView>::is_complete(
            &view
        ));
    }

    #[test]
    fn cancel_inserts_history_line() {
        let (tx_raw, rx) = channel::<AppEvent>();
        let tx = crate::app_event_sender::AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            has_input_focus: true,
            enhanced_keys_supported: false,
        });
        let mut view = RestoreProgressView::new(3);
        <RestoreProgressView as super::BottomPaneView>::handle_key_event(
            &mut view,
            &mut pane,
            KeyEvent {
                code: KeyCode::Esc,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        let events: Vec<AppEvent> = rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, AppEvent::InsertHistory(lines) if lines.iter().any(|l| l.to_string().contains("cancelled")))));
    }

    #[test]
    fn no_auto_progress_without_ticks() {
        let (tx_raw, _rx) = channel::<AppEvent>();
        let tx = crate::app_event_sender::AppEventSender::new(tx_raw);
        let _pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            has_input_focus: true,
            enhanced_keys_supported: false,
        });
        let view = RestoreProgressView::new(3);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let mut buf = Buffer::empty(area);
        <RestoreProgressView as super::BottomPaneView>::render(&view, area, &mut buf);
        std::thread::sleep(std::time::Duration::from_millis(30));
        <RestoreProgressView as super::BottomPaneView>::render(&view, area, &mut buf);
        assert!(!<RestoreProgressView as super::BottomPaneView>::is_complete(&view));
    }
}
