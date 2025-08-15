use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::WidgetRef;

use super::bottom_pane_view::BottomPaneView;
use super::{BottomPane, CancellationEvent};
use codex_core::protocol::{InputItem, Op};
use std::cell::Cell;
use std::time::{Duration, Instant};
use serde_json::Value;

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
    // animation timing
    start: Instant,
    duration: Duration,
    // conservative threshold to split overly large sends
    max_tokens_per_send: usize,
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
            start: Instant::now(),
            duration: Duration::from_millis(1500),
            max_tokens_per_send: 1800,
        }
    }
    pub fn from_plan(items: Vec<Value>, chunks: Vec<(usize, usize, usize)>, token_total: usize) -> Self {
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
            start: Instant::now(),
            duration: Duration::from_millis(1500),
            max_tokens_per_send: 1800,
        }
    }
    #[cfg(test)]
    pub fn with_duration(mut self, ms: u64) -> Self { self.duration = Duration::from_millis(ms); self }

    fn send_next_chunk(&mut self, pane: &mut BottomPane) {
        let Some(items) = &self.items else { return; };
        let Some(chunks) = &self.chunks else { return; };
        let idx = self.cursor.get();
        if idx >= chunks.len() { return; }
        let (s,e,tok) = chunks[idx];
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
        for v in &items[s..e] {
            match v.get("type").and_then(|t| t.as_str()) {
                Some("message") => {
                    if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
                        for c in arr { if let Some(t) = c.get("text").and_then(|t| t.as_str()) { text.push_str(t); text.push('\n'); } }
                    }
                }
                Some("function_call") => {
                    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                    text.push_str(&format!("[tool:{}] ", name));
                    text.push_str(&v.get("arguments").map(|a| a.to_string()).unwrap_or_else(||"{}".to_string()));
                    text.push('\n');
                }
                Some("function_call_output") => {
                    if let Some(arr) = v.get("output").and_then(|o| o.as_array()) {
                        for o in arr { if let Some(t) = o.get("text").and_then(|t| t.as_str()) { text.push_str(t); text.push('\n'); } }
                    } else if let Some(t) = v.get("output_text").and_then(|t| t.as_str()) { text.push_str(t); text.push('\n'); }
                }
                _ => {}
            }
        }
        if !text.trim().is_empty() {
            pane.app_event_tx.send(crate::app_event::AppEvent::CodexOp(Op::UserInput { items: vec![InputItem::Text { text }]}));
        }
        let new_sent = self.token_sent.get().saturating_add(tok);
        self.token_sent.set(new_sent);
        let pct = ((new_sent as f64 / self.token_total as f64) * 100.0) as u16;
        self.percent.set(pct.min(100));
        self.cursor.set(idx+1);
        if self.cursor.get() >= self.total_segments { self.complete.set(true); }
    }
}

impl<'a> BottomPaneView<'a> for RestoreProgressView {
    fn handle_key_event(&mut self, pane: &mut BottomPane<'a>, key_event: KeyEvent) {
        use crossterm::event::KeyCode;
        match key_event.code {
            KeyCode::Enter => {
                if !self.canceled.get() && self.percent.get() < 100 {
                    if self.items.is_some() { self.send_next_chunk(pane); }
                    else {
                        let next = (self.percent.get() + 20).min(100);
                        self.percent.set(next);
                        if next == 100 { self.complete.set(true); }
                    }
                    pane.update_status_text(format!("Restoring… {}% ({} segments)", self.percent.get(), self.total_segments));
                    if self.complete.get() {
                        let segs_done = self.cursor.get().min(self.total_segments);
                        let summary = format!("Experimental restore complete: {}/{} segments (~{} tokens).", segs_done, self.total_segments, self.token_sent.get().max(1));
                        pane.app_event_tx.send(crate::app_event::AppEvent::InsertHistory(vec![ratatui::text::Line::from(summary)]));
                        // Notify chat layer so it can report provider usage
                        if self.items.is_some() {
                            pane.app_event_tx.send(crate::app_event::AppEvent::RestoreCompleted {
                                approx_tokens: self.token_sent.get().max(1),
                                segments: self.total_segments,
                            });
                        }
                    }
                }
            }
            KeyCode::Char('x') => {
                self.canceled.set(true);
                self.complete.set(true);
                pane.update_status_text("Restore cancelled".to_string());
                pane.app_event_tx.send(crate::app_event::AppEvent::InsertHistory(vec![
                    ratatui::text::Line::from("Experimental restore cancelled by user.")
                ]));
                pane.app_event_tx.send(crate::app_event::AppEvent::CodexOp(Op::Interrupt));
            }
            _ => {}
        }
        pane.request_redraw();
    }

    fn on_ctrl_c(&mut self, pane: &mut BottomPane<'a>) -> CancellationEvent {
        self.canceled.set(true);
        self.complete.set(true);
        pane.update_status_text("Restore cancelled".to_string());
        CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool { self.complete.get() }

    fn desired_height(&self, _width: u16) -> u16 { 1 }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if self.items.is_none() && !self.complete.get() && !self.canceled.get() {
            // timer only for simulated path
            let elapsed = Instant::now().duration_since(self.start);
            let pct = ((elapsed.as_millis() as f64 / self.duration.as_millis() as f64) * 100.0) as u16;
            let pct = pct.min(100);
            if pct > self.percent.get() {
                self.percent.set(pct);
                if pct == 100 { self.complete.set(true); }
            }
        }
        let label = if self.canceled.get() { "Restore cancelled" } else { "Restoring… (Enter next, Ctrl-X cancel)" };
        ratatui::text::Line::from(label).render_ref(area, buf);
    }
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
    fn progresses_to_completion_on_enter() {
        let (tx_raw, _rx) = channel::<AppEvent>();
        let tx = crate::app_event_sender::AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams { app_event_tx: tx, has_input_focus: true, enhanced_keys_supported: false });
        let mut view = RestoreProgressView::new(5);
        for _ in 0..5 {
            <RestoreProgressView as super::BottomPaneView>::handle_key_event(
                &mut view,
                &mut pane,
                KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }
            );
        }
        assert!(<RestoreProgressView as super::BottomPaneView>::is_complete(&view));
    }

    #[test]
    fn cancel_inserts_history_line() {
        let (tx_raw, rx) = channel::<AppEvent>();
        let tx = crate::app_event_sender::AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams { app_event_tx: tx, has_input_focus: true, enhanced_keys_supported: false });
        let mut view = RestoreProgressView::new(3).with_duration(10);
        <RestoreProgressView as super::BottomPaneView>::handle_key_event(
            &mut view,
            &mut pane,
            KeyEvent { code: KeyCode::Char('x'), modifiers: KeyModifiers::CONTROL, kind: KeyEventKind::Press, state: KeyEventState::NONE }
        );
        let events: Vec<AppEvent> = rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, AppEvent::InsertHistory(lines) if lines.iter().any(|l| l.to_string().contains("cancelled")))));
    }

    #[test]
    fn timer_reaches_completion() {
        let (tx_raw, _rx) = channel::<AppEvent>();
        let tx = crate::app_event_sender::AppEventSender::new(tx_raw);
        let _pane = BottomPane::new(BottomPaneParams { app_event_tx: tx, has_input_focus: true, enhanced_keys_supported: false });
        let view = RestoreProgressView::new(3).with_duration(10);
        let area = Rect { x: 0, y: 0, width: 80, height: 1 };
        let mut buf = Buffer::empty(area);
        <RestoreProgressView as super::BottomPaneView>::render(&view, area, &mut buf);
        std::thread::sleep(std::time::Duration::from_millis(30));
        <RestoreProgressView as super::BottomPaneView>::render(&view, area, &mut buf);
        assert!(<RestoreProgressView as super::BottomPaneView>::is_complete(&view));
    }
}
