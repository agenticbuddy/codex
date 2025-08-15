use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::widgets::WidgetRef;

use super::bottom_pane_view::BottomPaneView;
use super::popup_consts::MAX_POPUP_ROWS;
use crate::bottom_pane::BottomPane;

/// Read‑only viewer for a saved session with an action selector footer.
pub(crate) struct SessionViewer {
    path: PathBuf,
    items: Vec<serde_json::Value>,
    show_full: bool,
    provider_token: Option<String>,
    action_idx: usize,
    complete: bool,
    scroll_top: usize,
}

impl SessionViewer {
    pub(crate) fn new(path: PathBuf, provider_token: Option<String>) -> Self {
        let items = Self::read_items(&path);
        Self {
            path,
            items,
            provider_token,
            show_full: false,
            action_idx: 0,
            complete: false,
            scroll_top: 0,
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
}

impl<'a> BottomPaneView<'a> for SessionViewer {
    fn handle_key_event(
        &mut self,
        pane: &mut BottomPane<'a>,
        key_event: crossterm::event::KeyEvent,
    ) {
        use crossterm::event::KeyCode;
        match key_event.code {
            KeyCode::Right | KeyCode::Tab => self.toggle_mode(),
            KeyCode::Left => {
                self.action_idx = (self.action_idx + 3) % 4;
            }
            KeyCode::Enter => {
                match self.action_idx {
                    0 => {
                        /* Return */
                        self.complete = true;
                    }
                    1 => {
                        // Restore (local)
                        pane.set_composer_text(format!(
                            "Resume this session: {}",
                            self.path.display()
                        ));
                        self.complete = true;
                    }
                    2 => {
                        // Experimental
                        pane.set_composer_text(format!(
                            "Resume (experimental): {}",
                            self.path.display()
                        ));
                        self.complete = true;
                    }
                    _ => {
                        // Server Restore
                        if let Some(tok) = &self.provider_token {
                            pane.app_event_tx.send(crate::app_event::AppEvent::CodexOp(codex_core::protocol::Op::SetResumeToken { token: tok.clone() }));
                            pane.app_event_tx.send(crate::app_event::AppEvent::InsertHistory(vec![
                                ratatui::text::Line::from("restoring session…").gray(),
                                ratatui::text::Line::from("")
                            ]));
                            pane.app_event_tx.send(crate::app_event::AppEvent::CodexOp(codex_core::protocol::Op::HandshakeResume));
                            self.complete = true;
                        } else {
                            pane.app_event_tx
                                .send(crate::app_event::AppEvent::InsertHistory(vec![
                                    ratatui::text::Line::from(
                                        "server resume unavailable — no token",
                                    )
                                    .gray(),
                                    ratatui::text::Line::from("Use Tab to choose another action.")
                                        .gray(),
                                    ratatui::text::Line::from(""),
                                ]));
                        }
                    }
                }
            }
            KeyCode::Esc => {
                self.complete = true;
            }
            KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('h') | KeyCode::Char('H') => {
                self.show_full = !self.show_full;
                self.scroll_top = 0; // reset scroll when toggling view
            }
            KeyCode::Up | KeyCode::PageUp => {
                let dec = if matches!(key_event.code, KeyCode::PageUp) { MAX_POPUP_ROWS } else { 1 };
                self.scroll_top = self.scroll_top.saturating_sub(dec);
            }
            KeyCode::Down | KeyCode::PageDown => {
                let lines = if self.show_full { crate::transcript::render_full_lines(&self.items) } else { crate::transcript::render_user_assistant_lines(&self.items) };
                let visible = MAX_POPUP_ROWS;
                let max_start = lines.len().saturating_sub(visible);
                let inc = if matches!(key_event.code, KeyCode::PageDown) { MAX_POPUP_ROWS } else { 1 };
                self.scroll_top = (self.scroll_top + inc).min(max_start);
            }
            KeyCode::Home => {
                self.scroll_top = 0;
            }
            KeyCode::End => {
                let lines = if self.show_full { crate::transcript::render_full_lines(&self.items) } else { crate::transcript::render_user_assistant_lines(&self.items) };
                let visible = MAX_POPUP_ROWS;
                let max_start = lines.len().saturating_sub(visible);
                self.scroll_top = max_start;
            }
            _ => {}
        }
        pane.request_redraw();
    }

    fn on_ctrl_c(&mut self, _pane: &mut BottomPane<'a>) -> super::CancellationEvent {
        self.complete = true;
        super::CancellationEvent::Handled
    }
    fn is_complete(&self) -> bool {
        self.complete
    }
    fn desired_height(&self, _width: u16) -> u16 {
        // Header + list (up to MAX) + footer
        let lines = if self.show_full {
            crate::transcript::render_full_lines(&self.items)
        } else {
            crate::transcript::render_user_assistant_lines(&self.items)
        };
        let list_h = (lines.len() as u16).clamp(1, MAX_POPUP_ROWS as u16);
        2 + list_h + 1
    }
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Header
        let header = ratatui::text::Line::from(format!("view: {}", self.path.display())).gray();
        header.render_ref(
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: 1,
            },
            buf,
        );
        // Content – show a window of up to MAX_POPUP_ROWS lines with scroll support.
        let lines = if self.show_full {
            crate::transcript::render_full_lines(&self.items)
        } else {
            crate::transcript::render_user_assistant_lines(&self.items)
        };
        let visible = MAX_POPUP_ROWS;
        let max_start = lines.len().saturating_sub(visible);
        let start = self.scroll_top.min(max_start);
        let end = (start + visible).min(lines.len());
        let slice = &lines[start..end];
        let mut y = area.y.saturating_add(1);
        for s in slice {
            if y >= area.y.saturating_add(area.height.saturating_sub(2)) {
                break;
            }
            ratatui::text::Line::from(s.clone()).render_ref(
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
            y = y.saturating_add(1);
        }
        // Status line: show visible range and total
        let total = lines.len();
        let status_text = if total == 0 {
            "0–0 / 0".to_string()
        } else {
            format!("{}–{} / {}", start.saturating_add(1), end, total)
        };
        ratatui::text::Line::from(status_text).gray().render_ref(
            Rect {
                x: area.x,
                y: area.y + area.height.saturating_sub(2),
                width: area.width,
                height: 1,
            },
            buf,
        );
        // Footer: actions
        use crate::colors::{SELECT_HL_BG, SELECT_HL_FG};
        use ratatui::style::{Color, Style};
        use ratatui::text::{Line, Span};
        let labels = ["Return", "Restore", "Exp. Restore", "Server Restore"];
        let mut spans: Vec<Span> = Vec::new();
        for (i, l) in labels.iter().enumerate() {
            if i == self.action_idx {
                spans.push(Span::styled(
                    format!(" {} ", l),
                    Style::default().bg(SELECT_HL_BG).fg(SELECT_HL_FG),
                ));
                spans.push(Span::raw(" "));
            } else {
                spans.push(Span::raw(format!(" {} ", l)));
                spans.push(Span::raw(" "));
            }
        }
        spans.push(Span::styled(
            "  ←/→ switch · ↑/↓ scroll · PgUp/PgDn fast · Home/End jump · Enter select · Esc back · F toggle full history",
            Style::default().fg(Color::DarkGray),
        ));
        let footer = Line::from(spans);
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
        let _ = std::fs::write(&path, "{\"timestamp\":\"2025-01-01T00:00:00Z\"}\n{}");

        // Default Return (Enter)
        let mut v = SessionViewer::new(path.clone(), Some("resp_1".into()));
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

        // Restore
        let mut v = SessionViewer::new(path.clone(), Some("resp_1".into()));
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
                .starts_with("Resume this session:")
        );

        // Exp. Restore
        let mut v = SessionViewer::new(path.clone(), Some("resp_1".into()));
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
                .starts_with("Resume (experimental):")
        );

        // Server Restore
        let mut v = SessionViewer::new(path.clone(), Some("resp_1".into()));
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
