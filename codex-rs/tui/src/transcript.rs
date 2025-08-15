use serde_json::Value;

/// Minimal transcript renderer for user/assistant messages used by viewers.
/// Converts response items (serde_json::Value) into plain lines like
/// "user: ..." or "assistant: ..." without re-executing anything.
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
        if !buf.is_empty() {
            let prefix = if role == Some("user") { "user:" } else { "assistant:" };
            out.push(format!("{prefix} {buf}"));
        }
    }
    out
}

/// Full transcript including tool calls and their outputs.
pub(crate) fn render_full_lines(items: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in items {
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
                    let prefix = if role == Some("user") { "user:" } else { "assistant:" };
                    out.push(format!("{prefix} {buf}"));
                }
            }
            Some("function_call") => {
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                let args = v.get("arguments").map(|a| a.to_string()).unwrap_or("{}".to_string());
                out.push(format!("tool: {} args: {}", name, args));
            }
            Some("function_call_output") => {
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
        assert_eq!(lines, vec!["user: hi".to_string(), "assistant: ok".to_string()]);
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
}
