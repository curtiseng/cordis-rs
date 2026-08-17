//! 一个赖着不走的 guest：`unload` 死循环。

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(text) => text,
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let id = request.get("id").cloned().unwrap_or(json!(0));
        match request.get("op").and_then(Value::as_str) {
            Some("load") => {
                let _ = respond(
                    &mut stdout,
                    json!({
                        "id": id,
                        "ok": true,
                        "logs": ["runaway 装上了"],
                        "tools": [],
                        "llm": null,
                    }),
                );
            }
            Some("unload") => loop {
                std::hint::spin_loop();
            },
            _ => {}
        }
    }
}

fn respond(stdout: &mut impl Write, value: Value) -> io::Result<()> {
    writeln!(stdout, "{value}")?;
    stdout.flush()
}
