use serde_json::Value;

/// Keep only entries that are valid ResponseItems for server restore.
/// Filters out any `record_type` lines (e.g., state/tool_event) and unknown entries.
pub(crate) fn filter_response_items(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .filter(|v| {
            matches!(
                v.get("type").and_then(|t| t.as_str()),
                Some("message")
                    | Some("reasoning")
                    | Some("function_call")
                    | Some("function_call_output")
                    | Some("local_shell_call")
            )
        })
        .cloned()
        .collect()
}

/// Approximate token count for a list of JSON response items.
/// Uses a simple heuristic: character count / 4, rounded up.
pub(crate) fn approximate_tokens(items: &[Value]) -> usize {
    let mut chars = 0usize;
    for v in items {
        match v.get("type").and_then(|t| t.as_str()) {
            Some("message") => {
                if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
                    for c in arr {
                        if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
                            chars += t.len();
                        }
                    }
                }
            }
            Some("function_call") => {
                chars += v
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map_or(0, |s| s.len());
                chars += v.get("arguments").map(|a| a.to_string().len()).unwrap_or(0);
            }
            Some("function_call_output") => {
                if let Some(arr) = v.get("output").and_then(|o| o.as_array()) {
                    for o in arr {
                        if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                            chars += t.len();
                        }
                    }
                } else if let Some(t) = v.get("output_text").and_then(|t| t.as_str()) {
                    chars += t.len();
                }
            }
            _ => {}
        }
    }
    chars.div_ceil(4)
}

/// Greedy segmentation of items by approximate token threshold.
/// Returns a vector of (start_index, end_index, token_estimate) for each chunk.
pub(crate) fn segment_items_by_tokens(
    items: &[Value],
    max_tokens_per_chunk: usize,
) -> Vec<(usize, usize, usize)> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < items.len() {
        let mut end = i;
        let mut est = 0usize;
        while end < items.len() {
            let e = approximate_tokens(&items[start..=end]);
            if e > max_tokens_per_chunk {
                break;
            }
            est = e;
            end += 1;
        }
        if end == start {
            // single over-limit item; force one-item chunk
            let e = approximate_tokens(&items[start..start + 1]);
            chunks.push((start, start + 1, e));
            start += 1;
            i = start;
            continue;
        }
        chunks.push((start, end, est));
        start = end;
        i = end;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> Value {
        serde_json::json!({"type":"message","role":role,"content":[{"text":text}]})
    }

    #[test]
    fn segments_under_threshold() {
        let items = vec![
            msg("user", "short"),
            msg("assistant", "hello"),
            msg("user", &"x".repeat(200)),
        ];
        let chunks = segment_items_by_tokens(&items, 50);
        assert!(!chunks.is_empty());
        for (_, _, t) in &chunks {
            assert!(*t <= 50);
        }
        // Chunks cover all items
        let total = chunks.iter().map(|(s, e, _)| e - s).sum::<usize>();
        assert_eq!(total, items.len());
    }

    #[test]
    fn single_over_limit_item_forces_one_item_chunk() {
        let items = vec![msg("user", &"z".repeat(2000))];
        let chunks = segment_items_by_tokens(&items, 10);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].1 - chunks[0].0, 1);
    }
}
