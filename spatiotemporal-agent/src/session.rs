use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

pub fn sessions_dir(workspace: &Path) -> PathBuf {
    workspace.join(".agent/sessions")
}

pub fn session_path(workspace: &Path, id: &str) -> PathBuf {
    sessions_dir(workspace).join(format!("{id}.jsonl"))
}

pub fn new_session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("s-{nanos:x}")
}

/// 从 JSONL 还原 OpenAI 格式的 messages（跳过 system）。
pub fn derive_messages(workspace: &Path, id: &str) -> spatiotemporal::Result<Vec<Value>> {
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
        messages.push(value);
    }
    Ok(messages)
}

pub fn append_message(workspace: &Path, id: &str, message: &Value) -> spatiotemporal::Result<()> {
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
