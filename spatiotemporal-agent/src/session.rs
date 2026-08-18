use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(serde::Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    pub turns: usize,
}

pub fn sessions_dir(workspace: &Path) -> PathBuf {
    workspace.join(".agent/sessions")
}

pub fn session_path(workspace: &Path, id: &str) -> PathBuf {
    sessions_dir(workspace).join(format!("{id}.jsonl"))
}

pub fn valid_session_id(id: &str) -> bool {
    id.starts_with("s-") && id.len() > 2 && id[2..].chars().all(|c| c.is_ascii_hexdigit())
}

pub fn new_session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("s-{nanos:x}")
}

/// 列出工作区内所有 session，按最近修改时间倒序。
pub fn list_sessions(workspace: &Path) -> spatiotemporal::Result<Vec<SessionSummary>> {
    let dir = sessions_dir(workspace);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut summaries = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(map_io)? {
        let entry = entry.map_err(map_io)?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !valid_session_id(id) {
            continue;
        }
        let updated_at = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let messages = derive_messages(workspace, id).unwrap_or_default();
        let turns = messages
            .iter()
            .filter(|msg| msg.get("role").and_then(Value::as_str) == Some("user"))
            .count();
        summaries.push(SessionSummary {
            id: id.to_owned(),
            title: session_title(&messages),
            updated_at,
            turns,
        });
    }
    summaries.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(summaries)
}

fn session_title(messages: &[Value]) -> String {
    messages
        .iter()
        .find(|msg| msg.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|msg| msg.get("content").and_then(Value::as_str))
        .map(truncate_title)
        .unwrap_or_else(|| "新会话".into())
}

fn truncate_title(text: &str) -> String {
    let trimmed = text.trim().replace('\n', " ");
    let max_chars = 48;
    if trimmed.chars().count() <= max_chars {
        return trimmed;
    }
    let short: String = trimmed.chars().take(max_chars).collect();
    format!("{short}…")
}

/// 从 JSONL 还原 OpenAI 格式的 messages（跳过 system）。
pub fn derive_messages(workspace: &Path, id: &str) -> spatiotemporal::Result<Vec<Value>> {
    if !valid_session_id(id) {
        return Ok(Vec::new());
    }
    let path = session_path(workspace, id);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(&path).map_err(map_io)?;
    let reader = BufReader::new(file);
    let mut messages = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(map_io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed).map_err(|error| {
            spatiotemporal::Error::Component(format!("session JSONL 解析失败：{error}"))
        })?;
        if value.get("role").and_then(Value::as_str) == Some("system") {
            continue;
        }
        if value.get("role").and_then(Value::as_str) == Some("event") {
            continue;
        }
        messages.push(value);
    }
    Ok(messages)
}

/// 从 JSONL 读取 UI 事件（role=event），供刷新后重建工具链时间线。
pub fn derive_events(workspace: &Path, id: &str) -> spatiotemporal::Result<Vec<Value>> {
    if !valid_session_id(id) {
        return Ok(Vec::new());
    }
    let path = session_path(workspace, id);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(&path).map_err(map_io)?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(map_io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed).map_err(|error| {
            spatiotemporal::Error::Component(format!("session JSONL 解析失败：{error}"))
        })?;
        if value.get("role").and_then(Value::as_str) == Some("event") {
            events.push(value);
        }
    }
    Ok(events)
}

pub fn append_message(workspace: &Path, id: &str, message: &Value) -> spatiotemporal::Result<()> {
    if !valid_session_id(id) {
        return Err(spatiotemporal::Error::Component(format!(
            "非法 session_id：{id}"
        )));
    }
    if message.get("role").and_then(Value::as_str) == Some("system") {
        return Ok(());
    }
    let dir = sessions_dir(workspace);
    std::fs::create_dir_all(&dir).map_err(map_io)?;
    let path = session_path(workspace, id);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(map_io)?;
    serde_json::to_writer(&mut file, message).map_err(|error| {
        spatiotemporal::Error::Component(format!("session 写入 JSON 失败：{error}"))
    })?;
    file.write_all(b"\n").map_err(map_io)?;
    Ok(())
}

pub fn append_messages(
    workspace: &Path,
    id: &str,
    messages: &[Value],
) -> spatiotemporal::Result<()> {
    for message in messages {
        append_message(workspace, id, message)?;
    }
    Ok(())
}

fn map_io(error: std::io::Error) -> spatiotemporal::Error {
    spatiotemporal::Error::Component(error.to_string())
}
