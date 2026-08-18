use serde_json::Value;

#[derive(Clone, Debug)]
pub struct CompactionConfig {
    pub max_messages: usize,
    pub max_chars: usize,
    pub keep_recent: usize,
    pub max_tool_chars: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        CompactionConfig {
            max_messages: 48,
            max_chars: 64_000,
            keep_recent: 24,
            max_tool_chars: 8_000,
        }
    }
}

impl CompactionConfig {
    pub fn from_config(config: &spatiotemporal::Value) -> Self {
        let defaults = CompactionConfig::default();
        let section = config.get("compaction").unwrap_or(config);
        CompactionConfig {
            max_messages: section
                .get("max_messages")
                .and_then(spatiotemporal::Value::as_u64)
                .unwrap_or(defaults.max_messages as u64) as usize,
            max_chars: section
                .get("max_chars")
                .and_then(spatiotemporal::Value::as_u64)
                .unwrap_or(defaults.max_chars as u64) as usize,
            keep_recent: section
                .get("keep_recent")
                .and_then(spatiotemporal::Value::as_u64)
                .unwrap_or(defaults.keep_recent as u64) as usize,
            max_tool_chars: section
                .get("max_tool_chars")
                .and_then(spatiotemporal::Value::as_u64)
                .unwrap_or(defaults.max_tool_chars as u64) as usize,
        }
    }

    pub fn disabled(&self) -> bool {
        self.max_messages == 0 && self.max_chars == 0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompactionReport {
    pub truncated_tools: usize,
    pub dropped_messages: usize,
}

/// 压缩 OpenAI 格式的 history（不含 system）。
pub fn compact(messages: &[Value], config: &CompactionConfig) -> (Vec<Value>, CompactionReport) {
    if config.disabled() || messages.is_empty() {
        return (messages.to_vec(), CompactionReport::default());
    }

    let mut report = CompactionReport::default();
    let mut out: Vec<Value> = messages
        .iter()
        .map(|message| truncate_tool_message(message, config.max_tool_chars, &mut report))
        .collect();

    if out.len() <= config.max_messages && message_chars(&out) <= config.max_chars {
        return (repair_tool_messages(&out), report);
    }

    let keep = config.keep_recent.max(1).min(out.len());
    let dropped = out.len().saturating_sub(keep);
    if dropped == 0 {
        return (repair_tool_messages(&out), report);
    }

    let mut tail: Vec<Value> = out.split_off(out.len() - keep);
    tail = repair_tool_messages(&tail);
    report.dropped_messages = dropped;
    let mut compacted = vec![serde_json::json!({
        "role": "assistant",
        "content": format!("（较早的 {dropped} 条消息已压缩省略，仅保留最近 {keep} 条。）")
    })];
    compacted.extend(tail);
    (repair_tool_messages(&compacted), report)
}

/// 去掉没有对应 assistant `tool_calls` 的孤儿 tool 消息（compaction 截断后常见）。
pub fn repair_tool_messages(messages: &[Value]) -> Vec<Value> {
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        if message.get("role").and_then(Value::as_str) == Some("tool") {
            let Some(id) = message.get("tool_call_id").and_then(Value::as_str) else {
                continue;
            };
            if !has_tool_call_parent(&out, id) {
                continue;
            }
        }
        out.push(message.clone());
    }
    out
}

fn has_tool_call_parent(messages: &[Value], tool_call_id: &str) -> bool {
    for message in messages.iter().rev() {
        match message.get("role").and_then(Value::as_str) {
            Some("tool") => continue,
            Some("assistant") => {
                return message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .is_some_and(|calls| {
                        calls.iter().any(|call| {
                            call.get("id").and_then(Value::as_str) == Some(tool_call_id)
                        })
                    });
            }
            _ => return false,
        }
    }
    false
}

fn truncate_tool_message(
    message: &Value,
    max_tool_chars: usize,
    report: &mut CompactionReport,
) -> Value {
    if message.get("role").and_then(Value::as_str) != Some("tool") {
        return message.clone();
    }
    let Some(content) = message.get("content").and_then(Value::as_str) else {
        return message.clone();
    };
    if content.chars().count() <= max_tool_chars {
        return message.clone();
    }
    report.truncated_tools += 1;
    let truncated: String = content.chars().take(max_tool_chars).collect();
    let mut copy = message.clone();
    copy["content"] = Value::String(format!(
        "{truncated}\n…（tool 输出已截断至 {max_tool_chars} 字）"
    ));
    copy
}

fn message_chars(messages: &[Value]) -> usize {
    messages
        .iter()
        .map(|message| {
            serde_json::to_string(message)
                .map(|text| text.len())
                .unwrap_or(0)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncates_long_tool_output() {
        let messages = vec![
            json!({
                "role": "assistant",
                "tool_calls": [{
                    "id": "1",
                    "type": "function",
                    "function": { "name": "bash", "arguments": "{}" }
                }]
            }),
            json!({
                "role": "tool",
                "tool_call_id": "1",
                "content": "x".repeat(20_000)
            }),
        ];
        let config = CompactionConfig {
            max_tool_chars: 100,
            ..CompactionConfig::default()
        };
        let (out, report) = compact(&messages, &config);
        assert_eq!(report.truncated_tools, 1);
        assert!(out[1]["content"].as_str().unwrap().contains("已截断"));
    }

    #[test]
    fn drops_middle_when_over_message_limit() {
        let messages: Vec<Value> = (0..10)
            .map(|index| json!({ "role": "user", "content": format!("m{index}") }))
            .collect();
        let config = CompactionConfig {
            max_messages: 4,
            keep_recent: 3,
            max_chars: usize::MAX,
            max_tool_chars: usize::MAX,
        };
        let (out, report) = compact(&messages, &config);
        assert_eq!(report.dropped_messages, 7);
        assert_eq!(out.len(), 4);
        assert!(out[0]["content"].as_str().unwrap().contains("压缩省略"));
        assert_eq!(out[1]["content"], "m7");
    }

    #[test]
    fn drops_orphan_tool_after_compaction() {
        let messages: Vec<Value> = (0..8)
            .map(|index| {
                if index % 3 == 1 {
                    json!({
                        "role": "assistant",
                        "tool_calls": [{
                            "id": format!("call_{index}"),
                            "type": "function",
                            "function": { "name": "bash", "arguments": "{}" }
                        }]
                    })
                } else if index % 3 == 2 {
                    json!({
                        "role": "tool",
                        "tool_call_id": format!("call_{}", index - 1),
                        "content": "ok"
                    })
                } else {
                    json!({ "role": "user", "content": format!("m{index}") })
                }
            })
            .collect();
        let config = CompactionConfig {
            max_messages: 4,
            keep_recent: 3,
            max_chars: usize::MAX,
            max_tool_chars: usize::MAX,
        };
        let (out, report) = compact(&messages, &config);
        assert_eq!(report.dropped_messages, 5);
        assert!(
            out.windows(2).all(|pair| {
                pair[1].get("role").and_then(Value::as_str) != Some("tool")
                    || has_tool_parent_in_slice(pair)
            }),
            "compacted history must not start with orphan tool messages"
        );
    }

    fn has_tool_parent_in_slice(pair: &[Value]) -> bool {
        pair[1].get("role").and_then(Value::as_str) != Some("tool")
            || pair[0]
                .get("tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|calls| {
                    calls.iter().any(|call| {
                        call.get("id").and_then(Value::as_str)
                            == pair[1].get("tool_call_id").and_then(Value::as_str)
                    })
                })
    }

    #[test]
    fn repair_drops_orphan_tools() {
        let messages = vec![
            json!({ "role": "assistant", "content": "summary" }),
            json!({ "role": "tool", "tool_call_id": "missing", "content": "orphan" }),
            json!({ "role": "user", "content": "next" }),
        ];
        let out = repair_tool_messages(&messages);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["role"], "assistant");
        assert_eq!(out[1]["role"], "user");
    }
}
