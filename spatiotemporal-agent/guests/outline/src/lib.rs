//! wasm 插件：从授予的 markdown 文本里抽出标题大纲。

wit_bindgen::generate!({
    path: "../../../crates/spatiotemporal-wasm/wit",
    world: "plugin",
});

use composability::plugin::host::{capability, log, register_tool};
use exports::composability::plugin::lifecycle::Guest;

struct Outline;

impl Guest for Outline {
    fn load() -> Result<(), String> {
        register_tool(
            "outline",
            "提取当前 Markdown 文档的标题大纲（# / ## / ###）",
        );
        log("outline 装上了");
        Ok(())
    }

    fn unload() {
        log("outline 拆掉了");
    }

    fn invoke(name: String, _args: String) -> Result<String, String> {
        if name != "outline" {
            return Err(format!("没有这个工具：{name}"));
        }
        let md = capability("markdown")?;
        Ok(headings(&md))
    }
}

fn headings(md: &str) -> String {
    let lines: Vec<&str> = md
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('#') && line.contains(' '))
        .collect();
    if lines.is_empty() {
        "(没有标题)".into()
    } else {
        lines.join("\n")
    }
}

export!(Outline);
