use crate::history_cell::HistoryCell;
use codex_core::config_types::UriBasedFileOpener;
use mcp_types::CallToolResult;
use ratatui::style::Stylize;
use ratatui::text::Line as RLine;
use ratatui::text::Line;
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

/// Minimal transcript renderer for user/assistant messages used by viewers.
/// Converts response items (serde_json::Value) into plain lines like
/// "user: ..." or "assistant: ..." without re-executing anything.
#[cfg(test)]
pub(crate) fn render_user_assistant_lines(items: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in items {
        if v.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue;
        }
        let role = v.get("role").and_then(|r| r.as_str());
        if role != Some("user") && role != Some("assistant") {
            continue;
        }
        let mut buf = String::new();
        if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
            for item in arr {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    buf.push_str(t);
                }
            }
        }
        // Hide synthetic seed messages (e.g., initial AGENTS.md read) from viewer
        if !buf.is_empty() {
            if role == Some("user") {
                let t = buf.trim_start();
                if t.starts_with("<user_instructions>") || t.starts_with("<environment_context>") {
                    continue;
                }
            }
            let prefix = if role == Some("user") {
                "user:"
            } else {
                "assistant:"
            };
            out.push(format!("{prefix} {buf}"));
        }
    }
    out
}

/// Full transcript including tool calls and their outputs.
#[cfg(test)]
pub(crate) fn render_full_lines(items: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // If tool_event records are present, collect call_ids to avoid duplicating
    // raw function_call/function_call_output lines in the transcript.
    let mut tool_event_call_ids: HashSet<String> = HashSet::new();
    for v in items {
        if v.get("record_type")
            .and_then(|rt| rt.as_str())
            .map(|s| s == "tool_event")
            .unwrap_or(false)
        {
            if let Some(id) = v.get("call_id").and_then(|c| c.as_str()) {
                tool_event_call_ids.insert(id.to_string());
            }
        }
    }

    for v in items {
        // Handle tool_event records (preferred rendering if present)
        if v.get("record_type")
            .and_then(|rt| rt.as_str())
            .map(|s| s == "tool_event")
            .unwrap_or(false)
        {
            let kind = v.get("tool_kind").and_then(|k| k.as_str()).unwrap_or("");
            let phase = v.get("phase").and_then(|p| p.as_str()).unwrap_or("");
            match (kind, phase) {
                ("exec", "begin") => {
                    if let Some(cmd) = v.get("command").and_then(|c| c.as_array()) {
                        let first = cmd.get(0).and_then(|s| s.as_str()).unwrap_or("");
                        let rest = cmd
                            .iter()
                            .skip(1)
                            .filter_map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(" ");
                        let mut line = String::from("⚡ Running ");
                        line.push_str(first);
                        if !rest.is_empty() {
                            line.push(' ');
                            line.push_str(&rest);
                        }
                        out.push(line);
                    } else {
                        out.push("⚡ Running".to_string());
                    }
                }
                ("exec", "end") => {
                    let exit = v.get("exit_code").and_then(|e| e.as_i64()).unwrap_or(0);
                    if exit == 0 {
                        out.push("✓ Completed".to_string());
                    } else {
                        out.push(format!("✗ Failed (exit {})", exit));
                    }
                    if let Some(s) = v.get("stdout_trunc").and_then(|s| s.as_str()) {
                        if !s.is_empty() {
                            out.extend(s.lines().map(|l| l.to_string()));
                        }
                    }
                    if let Some(s) = v.get("stderr_trunc").and_then(|s| s.as_str()) {
                        if !s.is_empty() {
                            out.extend(s.lines().map(|l| l.to_string()));
                        }
                    }
                    out.push(String::new());
                }
                ("mcp", "begin") => {
                    out.push("tool running...".to_string());
                    if let Some(inv) = v.get("invocation") {
                        let server = inv.get("server").and_then(|s| s.as_str()).unwrap_or("");
                        let tool = inv.get("tool").and_then(|s| s.as_str()).unwrap_or("");
                        out.push(format!("  └ {}.{}", server, tool));
                    }
                    out.push(String::new());
                }
                ("mcp", "end") => {
                    let ok = v.get("success").and_then(|b| b.as_bool()).unwrap_or(false);
                    out.push(format!("tool {}", if ok { "success" } else { "failed" }));
                    out.push(String::new());
                }
                _ => {}
            }
            continue;
        }

        match v.get("type").and_then(|t| t.as_str()) {
            Some("message") => {
                let role = v.get("role").and_then(|r| r.as_str());
                if role != Some("user") && role != Some("assistant") {
                    continue;
                }
                let mut buf = String::new();
                if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
                    for item in arr {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            buf.push_str(t);
                        }
                    }
                }
                if !buf.is_empty() {
                    if role == Some("user") {
                        let t = buf.trim_start();
                        if t.starts_with("<user_instructions>")
                            || t.starts_with("<environment_context>")
                        {
                            continue;
                        }
                    }
                    let prefix = if role == Some("user") {
                        "user:"
                    } else {
                        "assistant:"
                    };
                    out.push(format!("{prefix} {buf}"));
                }
            }
            Some("function_call") => {
                // If a tool_event exists for this call_id, skip raw line to avoid duplication.
                if v.get("call_id")
                    .and_then(|c| c.as_str())
                    .map(|id| tool_event_call_ids.contains(id))
                    .unwrap_or(false)
                {
                    continue;
                }
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                let args = v
                    .get("arguments")
                    .map(|a| a.to_string())
                    .unwrap_or("{}".to_string());
                out.push(format!("tool: {} args: {}", name, args));
            }
            Some("function_call_output") => {
                // If a tool_event exists for this call_id, skip raw output to avoid duplication.
                if v.get("call_id")
                    .and_then(|c| c.as_str())
                    .map(|id| tool_event_call_ids.contains(id))
                    .unwrap_or(false)
                {
                    continue;
                }
                // Try array form first
                if let Some(arr) = v.get("output").and_then(|o| o.as_array()) {
                    let mut buf = String::new();
                    for o in arr {
                        if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                            buf.push_str(t);
                        }
                    }
                    if !buf.is_empty() {
                        out.push(format!("tool.out: {}", buf));
                    }
                } else if let Some(t) = v.get("output_text").and_then(|t| t.as_str()) {
                    if !t.is_empty() {
                        out.push(format!("tool.out: {}", t));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn flatten_ratatui_lines(lines: Vec<RLine<'static>>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| line.spans.iter().map(|s| s.content.clone()).collect())
        .collect()
}

fn render_markdown_text_to_strings(source: &str) -> Vec<String> {
    let mut rendered: Vec<RLine<'static>> = Vec::new();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    crate::markdown::append_markdown_with_opener_and_cwd(
        source,
        &mut rendered,
        UriBasedFileOpener::None,
        &cwd,
    );
    flatten_ratatui_lines(rendered)
}

fn extract_plain_text_from_message(v: &Value) -> String {
    let mut buf = String::new();
    if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
        for item in arr {
            if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                buf.push_str(t);
            }
        }
    }
    buf
}

/// User/assistant with markdown for assistant messages and a "codex" header like live view.
#[allow(dead_code)]
pub(crate) fn render_user_assistant_markdown_lines(items: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in items {
        if v.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue;
        }
        let role = v.get("role").and_then(|r| r.as_str());
        match role {
            Some("user") => {
                let buf = extract_plain_text_from_message(v);
                if !buf.is_empty() {
                    let t = buf.trim_start();
                    if t.starts_with("<user_instructions>")
                        || t.starts_with("<environment_context>")
                    {
                        continue;
                    }
                    out.push("user:".to_string());
                    out.extend(buf.lines().map(|l| l.to_string()));
                }
            }
            Some("assistant") => {
                let buf = extract_plain_text_from_message(v);
                if !buf.is_empty() {
                    out.push("codex".to_string());
                    out.extend(render_markdown_text_to_strings(&buf));
                }
            }
            _ => {}
        }
    }
    out
}

/// Full transcript with markdown for assistant and reasoning and tool_event support.
pub(crate) fn render_full_markdown_lines(items: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Build maps of in-flight tool events so we can render completed blocks consistently.
    let mut tool_event_call_ids: HashSet<String> = HashSet::new();
    #[derive(Clone)]
    struct ExecBeginInfo {
        command: Vec<String>,
        parsed: Vec<codex_core::parse_command::ParsedCommand>,
    }
    let mut exec_begins: std::collections::HashMap<String, ExecBeginInfo> = Default::default();
    #[derive(Clone)]
    struct McpBeginInfo {
        server: String,
        tool: String,
        arguments: Option<serde_json::Value>,
    }
    let mut mcp_begins: std::collections::HashMap<String, McpBeginInfo> = Default::default();
    for v in items {
        if v.get("record_type")
            .and_then(|rt| rt.as_str())
            .map(|s| s == "tool_event")
            .unwrap_or(false)
        {
            if let Some(id) = v.get("call_id").and_then(|c| c.as_str()) {
                tool_event_call_ids.insert(id.to_string());
            }
            let kind = v.get("tool_kind").and_then(|k| k.as_str());
            let phase = v.get("phase").and_then(|p| p.as_str());
            match (kind, phase) {
                (Some("exec"), Some("begin")) => {
                    let id = v
                        .get("call_id")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    let command = v
                        .get("command")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| s.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_else(Vec::new);
                    let parsed = v
                        .get("parsed")
                        .and_then(|p| {
                            serde_json::from_value::<Vec<codex_core::parse_command::ParsedCommand>>(
                                p.clone(),
                            )
                            .ok()
                        })
                        .unwrap_or_default();
                    exec_begins.insert(id, ExecBeginInfo { command, parsed });
                }
                (Some("mcp"), Some("begin")) => {
                    let id = v
                        .get("call_id")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(inv) = v.get("invocation") {
                        let server = inv
                            .get("server")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string();
                        let tool = inv
                            .get("tool")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = inv.get("arguments").cloned();
                        mcp_begins.insert(
                            id,
                            McpBeginInfo {
                                server,
                                tool,
                                arguments,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
    }
    for v in items {
        // tool_event rendering (same as in render_full_lines)
        if v.get("record_type")
            .and_then(|rt| rt.as_str())
            .map(|s| s == "tool_event")
            .unwrap_or(false)
        {
            let kind = v.get("tool_kind").and_then(|k| k.as_str()).unwrap_or("");
            let phase = v.get("phase").and_then(|p| p.as_str()).unwrap_or("");
            match (kind, phase) {
                ("exec", "end") => {
                    // Render a completed exec block using history_cell logic (collapses output nicely).
                    if let Some(id) = v.get("call_id").and_then(|c| c.as_str()) {
                        if let Some(begin) = exec_begins.get(id) {
                            let exit =
                                v.get("exit_code").and_then(|e| e.as_i64()).unwrap_or(0) as i32;
                            let stdout_s = v
                                .get("stdout_trunc")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string();
                            let stderr_s = v
                                .get("stderr_trunc")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string();
                            let cell = crate::history_cell::new_completed_exec_command(
                                begin.command.clone(),
                                begin.parsed.clone(),
                                crate::history_cell::CommandOutput {
                                    exit_code: exit,
                                    stdout: stdout_s,
                                    stderr: stderr_s,
                                },
                            );
                            let lines = cell.display_lines();
                            out.extend(flatten_ratatui_lines(lines));
                        }
                    }
                }
                ("mcp", "end") => {
                    if let Some(id) = v.get("call_id").and_then(|c| c.as_str()) {
                        if let Some(begin) = mcp_begins.get(id) {
                            let duration_ms =
                                v.get("duration_ms").and_then(|d| d.as_u64()).unwrap_or(0);
                            let ok = v.get("success").and_then(|b| b.as_bool()).unwrap_or(false);
                            let result_val =
                                v.get("result").cloned().unwrap_or(serde_json::Value::Null);
                            let result: Result<CallToolResult, String> = if ok {
                                serde_json::from_value(result_val.clone())
                                    .map_err(|e| format!("{e}"))
                            } else {
                                // On failure, result is typically a string; fall back to string repr.
                                match result_val {
                                    Value::String(s) => Err(s),
                                    other => Err(other.to_string()),
                                }
                            };
                            let invocation = codex_core::protocol::McpInvocation {
                                server: begin.server.clone(),
                                tool: begin.tool.clone(),
                                arguments: begin.arguments.clone(),
                            };
                            let cell = crate::history_cell::new_completed_mcp_tool_call(
                                80,
                                invocation,
                                Duration::from_millis(duration_ms),
                                ok,
                                result,
                            );
                            let lines = cell.display_lines();
                            out.extend(flatten_ratatui_lines(lines));
                        }
                    }
                }
                _ => {}
            }
            continue;
        }

        match v.get("type").and_then(|t| t.as_str()) {
            Some("message") => {
                let role = v.get("role").and_then(|r| r.as_str());
                match role {
                    Some("user") => {
                        let buf = extract_plain_text_from_message(v);
                        if !buf.is_empty() {
                            let t = buf.trim_start();
                            if t.starts_with("<user_instructions>")
                                || t.starts_with("<environment_context>")
                            {
                                continue;
                            }
                            out.push("user:".to_string());
                            out.extend(buf.lines().map(|l| l.to_string()));
                        }
                    }
                    Some("assistant") => {
                        let buf = extract_plain_text_from_message(v);
                        if !buf.is_empty() {
                            out.push("codex".to_string());
                            out.extend(render_markdown_text_to_strings(&buf));
                        }
                    }
                    _ => {}
                }
            }
            Some("function_call") => {
                if v.get("call_id")
                    .and_then(|c| c.as_str())
                    .map(|id| tool_event_call_ids.contains(id))
                    .unwrap_or(false)
                {
                    continue;
                }
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                let args = v
                    .get("arguments")
                    .map(|a| a.to_string())
                    .unwrap_or("{}".to_string());
                out.push(format!("tool: {name} args: {args}"));
            }
            Some("function_call_output") => {
                if v.get("call_id")
                    .and_then(|c| c.as_str())
                    .map(|id| tool_event_call_ids.contains(id))
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Some(arr) = v.get("output").and_then(|o| o.as_array()) {
                    let mut buf = String::new();
                    for o in arr {
                        if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                            buf.push_str(t);
                        }
                    }
                    if !buf.is_empty() {
                        out.push(format!("tool.out: {buf}"));
                    }
                } else if let Some(t) = v.get("output_text").and_then(|t| t.as_str()) {
                    if !t.is_empty() {
                        out.push(format!("tool.out: {t}"));
                    }
                }
            }
            // Reasoning entries
            Some("reasoning") => {
                out.push("thinking".to_string());
                // Prefer content; fallback to summary
                if let Some(content) = v.get("content").and_then(|c| c.as_array()) {
                    let mut buf = String::new();
                    for item in content {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            buf.push_str(t);
                        }
                    }
                    if !buf.is_empty() {
                        out.extend(render_markdown_text_to_strings(&buf));
                    }
                } else if let Some(summary) = v.get("summary").and_then(|s| s.as_array()) {
                    let mut buf = String::new();
                    for item in summary {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            buf.push_str(t);
                        }
                    }
                    if !buf.is_empty() {
                        out.extend(render_markdown_text_to_strings(&buf));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Replay saved items into styled Lines using the same building blocks as live UI.
/// - user messages via HistoryCell
/// - assistant/reasoning with headers and markdown
/// - exec/mcp via HistoryCell on tool_event (end)
pub(crate) fn render_replay_lines(items: &[Value]) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    // Track exec/mcp begin info so we can render completed cells consistently on end.
    #[derive(Clone)]
    struct ExecBeginInfo {
        command: Vec<String>,
        parsed: Vec<codex_core::parse_command::ParsedCommand>,
    }
    let mut exec_begins: std::collections::HashMap<String, ExecBeginInfo> = Default::default();
    #[derive(Clone)]
    struct McpBeginInfo {
        server: String,
        tool: String,
        arguments: Option<serde_json::Value>,
    }
    let mut mcp_begins: std::collections::HashMap<String, McpBeginInfo> = Default::default();

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    for v in items {
        if v.get("record_type")
            .and_then(|rt| rt.as_str())
            .map(|s| s == "tool_event")
            .unwrap_or(false)
        {
            let kind = v.get("tool_kind").and_then(|k| k.as_str()).unwrap_or("");
            let phase = v.get("phase").and_then(|p| p.as_str()).unwrap_or("");
            match (kind, phase) {
                ("exec", "begin") => {
                    let id = v
                        .get("call_id")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    let command = v
                        .get("command")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| s.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_else(Vec::new);
                    let parsed = v
                        .get("parsed")
                        .and_then(|p| {
                            serde_json::from_value::<Vec<codex_core::parse_command::ParsedCommand>>(
                                p.clone(),
                            )
                            .ok()
                        })
                        .unwrap_or_default();
                    exec_begins.insert(id, ExecBeginInfo { command, parsed });
                }
                ("exec", "end") => {
                    if let Some(id) = v.get("call_id").and_then(|c| c.as_str()) {
                        if let Some(begin) = exec_begins.get(id) {
                            let exit =
                                v.get("exit_code").and_then(|e| e.as_i64()).unwrap_or(0) as i32;
                            let stdout_s = v
                                .get("stdout_trunc")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string();
                            let stderr_s = v
                                .get("stderr_trunc")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string();
                            let cell = crate::history_cell::new_completed_exec_command(
                                begin.command.clone(),
                                begin.parsed.clone(),
                                crate::history_cell::CommandOutput {
                                    exit_code: exit,
                                    stdout: stdout_s,
                                    stderr: stderr_s,
                                },
                            );
                            out.extend(cell.display_lines());
                        }
                    }
                }
                ("mcp", "begin") => {
                    let id = v
                        .get("call_id")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(inv) = v.get("invocation") {
                        let server = inv
                            .get("server")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string();
                        let tool = inv
                            .get("tool")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = inv.get("arguments").cloned();
                        mcp_begins.insert(
                            id,
                            McpBeginInfo {
                                server,
                                tool,
                                arguments,
                            },
                        );
                    }
                }
                ("mcp", "end") => {
                    if let Some(id) = v.get("call_id").and_then(|c| c.as_str()) {
                        if let Some(begin) = mcp_begins.get(id) {
                            let duration_ms =
                                v.get("duration_ms").and_then(|d| d.as_u64()).unwrap_or(0);
                            let ok = v.get("success").and_then(|b| b.as_bool()).unwrap_or(false);
                            let result_val =
                                v.get("result").cloned().unwrap_or(serde_json::Value::Null);
                            let result: Result<CallToolResult, String> = if ok {
                                serde_json::from_value(result_val.clone())
                                    .map_err(|e| format!("{e}"))
                            } else {
                                match result_val {
                                    Value::String(s) => Err(s),
                                    other => Err(other.to_string()),
                                }
                            };
                            let invocation = codex_core::protocol::McpInvocation {
                                server: begin.server.clone(),
                                tool: begin.tool.clone(),
                                arguments: begin.arguments.clone(),
                            };
                            let cell = crate::history_cell::new_completed_mcp_tool_call(
                                80,
                                invocation,
                                std::time::Duration::from_millis(duration_ms),
                                ok,
                                result,
                            );
                            out.extend(cell.display_lines());
                        }
                    }
                }
                _ => {}
            }
            continue;
        }

        match v.get("type").and_then(|t| t.as_str()) {
            Some("message") => {
                let role = v.get("role").and_then(|r| r.as_str());
                if role == Some("user") {
                    let text = extract_plain_text_from_message(v);
                    if !text.trim().is_empty() {
                        // Hide synthetic seed entries (AGENTS.md and environment banner)
                        let t = text.trim_start();
                        if t.starts_with("<user_instructions>")
                            || t.starts_with("<environment_context>")
                        {
                            // Skip seed/system banners from viewer
                            continue;
                        }
                        let cell = crate::history_cell::new_user_prompt(text);
                        out.extend(cell.display_lines());
                    }
                } else if role == Some("assistant") {
                    let text = extract_plain_text_from_message(v);
                    if !text.trim().is_empty() {
                        out.push(Line::from("codex".magenta().bold()));
                        crate::markdown::append_markdown_with_opener_and_cwd(
                            &text,
                            &mut out,
                            UriBasedFileOpener::None,
                            &cwd,
                        );
                    }
                }
            }
            Some("reasoning") => {
                out.push(Line::from("thinking".magenta().italic()));
                // Prefer content; fallback to summary
                if let Some(content) = v.get("content").and_then(|c| c.as_array()) {
                    let mut buf = String::new();
                    for item in content {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            buf.push_str(t);
                        }
                    }
                    if !buf.is_empty() {
                        crate::markdown::append_markdown_with_opener_and_cwd(
                            &buf,
                            &mut out,
                            UriBasedFileOpener::None,
                            &cwd,
                        );
                    }
                } else if let Some(summary) = v.get("summary").and_then(|s| s.as_array()) {
                    let mut buf = String::new();
                    for item in summary {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            buf.push_str(t);
                        }
                    }
                    if !buf.is_empty() {
                        crate::markdown::append_markdown_with_opener_and_cwd(
                            &buf,
                            &mut out,
                            UriBasedFileOpener::None,
                            &cwd,
                        );
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_user_and_assistant_only() {
        let items = vec![
            serde_json::json!({"type":"message","role":"user","content":[{"text":"hi"}]}),
            serde_json::json!({"type":"function_call","name":"sh","arguments":"{}"}),
            serde_json::json!({"type":"message","role":"assistant","content":[{"text":"ok"}]}),
        ];
        let lines = render_user_assistant_lines(&items);
        assert_eq!(
            lines,
            vec!["user: hi".to_string(), "assistant: ok".to_string()]
        );
    }

    #[test]
    fn renders_tool_and_output() {
        let items = vec![
            serde_json::json!({"type":"function_call","name":"shell","arguments":"{\"cmd\":[\"echo\",\"hi\"]}"}),
            serde_json::json!({"type":"function_call_output","output":[{"text":"ok"}]}),
        ];
        let lines = render_full_lines(&items);
        assert!(lines.iter().any(|l| l.contains("tool: shell")));
        assert!(lines.iter().any(|l| l.contains("tool.out: ok")));
    }

    #[test]
    fn renders_tool_event_and_skips_raw_function_lines() {
        let items = vec![
            // Original function_call lines (should be suppressed if tool_event present)
            serde_json::json!({
                "type":"function_call",
                "name":"shell",
                "arguments":"{\"command\":[\"echo\",\"hi\"]}",
                "call_id":"c1"
            }),
            serde_json::json!({
                "type":"function_call_output",
                "call_id":"c1",
                "output":[{"text":"ok"}]
            }),
            // Tool events
            serde_json::json!({
                "record_type":"tool_event",
                "ts":"2025-01-01T00:00:00Z",
                "tool_kind":"exec",
                "phase":"begin",
                "call_id":"c1",
                "command":["echo","hi"],
                "cwd":"/tmp"
            }),
            serde_json::json!({
                "record_type":"tool_event",
                "ts":"2025-01-01T00:00:01Z",
                "tool_kind":"exec",
                "phase":"end",
                "call_id":"c1",
                "exit_code":0,
                "duration_ms":10,
                "stdout_trunc":"ok\n",
                "stderr_trunc":"",
                "success":true
            }),
        ];
        let lines = render_full_lines(&items);
        // Should contain running/completed lines
        assert!(lines.iter().any(|l| l.contains("Running echo hi")));
        assert!(lines.iter().any(|l| l.contains("Completed")));
        // Should not contain the raw function_call labels
        assert!(!lines.iter().any(|l| l.contains("tool: shell")));
        assert!(!lines.iter().any(|l| l.contains("tool.out:")));
    }
}
