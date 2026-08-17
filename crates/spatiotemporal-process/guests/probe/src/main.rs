//! 一个规矩的子进程 guest：装上时报告它看见了什么，拆掉时留下痕迹。

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut logs = Vec::new();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(text) => text,
            Err(error) => {
                respond(
                    &mut stdout,
                    json!({ "id": 0, "ok": false, "error": error.to_string() }),
                );
                break;
            }
        };
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                respond(
                    &mut stdout,
                    json!({ "id": 0, "ok": false, "error": error.to_string() }),
                );
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(json!(0));
        match request.get("op").and_then(Value::as_str) {
            Some("load") => {
                logs.push("probe 装上了".to_owned());
                let caps = request
                    .get("capabilities")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                match caps.get("db").and_then(Value::as_str) {
                    Some(value) => logs.push(format!("db = {value}")),
                    None => logs.push("db 读不到：没有授予这项能力".to_owned()),
                }
                match caps.get("secrets").and_then(Value::as_str) {
                    Some(value) => logs.push(format!("secrets = {value}（本该拿不到！）")),
                    None => logs.push("secrets 读不到：没有授予这项能力".to_owned()),
                }
                respond(
                    &mut stdout,
                    json!({
                        "id": id,
                        "ok": true,
                        "logs": logs,
                        "tools": [],
                        "llm": null,
                    }),
                );
            }
            Some("unload") => {
                logs.push("probe 拆掉了".to_owned());
                respond(
                    &mut stdout,
                    json!({
                        "id": id,
                        "ok": true,
                        "logs": logs,
                    }),
                );
                break;
            }
            Some("invoke") => {
                respond(
                    &mut stdout,
                    json!({
                        "id": id,
                        "ok": false,
                        "error": "probe 没有工具",
                    }),
                );
            }
            _ => {
                respond(
                    &mut stdout,
                    json!({
                        "id": id,
                        "ok": false,
                        "error": "未知 op",
                    }),
                );
            }
        }
    }
}

fn respond(stdout: &mut impl Write, value: Value) {
    let line = value.to_string();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}
