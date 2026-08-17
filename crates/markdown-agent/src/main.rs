//! 一个会读 Markdown 的 agent：三种基质的插件挂在同一棵 fiber 树上。
//!
//! ```text
//! native  read_doc   把整篇文档交给模型
//! wasm    outline    抽标题大纲（能力面由 WIT 钉死）
//! script  cite       按关键词引用原文（模型现写代码的形状）
//! native  deepseek   调 LLM
//! native  web        浏览器里对话
//! ```
//!
//! 用法：
//!
//! ```bash
//! export DEEPSEEK_API_KEY=sk-...
//! cargo run -p markdown-agent -- crates/markdown-agent/assets/sample.md
//! ```
//!
//! `--smoke` 只装插件、列出工具然后退出，不需要 API key，CI 用这个。

mod chat;
mod plugins;
mod web;

use std::path::PathBuf;
use std::rc::Rc;

use spatiotemporal::App;
use spatiotemporal_script::ScriptPlugin;
use spatiotemporal_wasm::WasmPlugin;

use crate::plugins::{
    DeepSeek, DocFile, ReadDoc, Toolbox, load_script, script_caps, wasm_caps, wasm_guest,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let smoke = args.iter().any(|a| a == "--smoke");
    let doc_path = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/sample.md"));

    let mut app = App::new();
    let root = app.root();
    let tools = Toolbox::new();

    root.use_component(Rc::new(DocFile::open(&doc_path).unwrap_or_else(|error| {
        panic!("读不了 {}：{error}", doc_path.display());
    })));
    root.use_component(Rc::new(DeepSeek::from_env()));
    root.use_component(Rc::new(ReadDoc {
        tools: tools.clone(),
    }));

    let wasm_path = wasm_guest("outline");
    let wasm = WasmPlugin::open(&wasm_path, wasm_caps(), vec!["markdown".into()])
        .unwrap_or_else(|error| {
            panic!(
                "编不了 {}：{error}\n先跑 crates/markdown-agent/scripts/build-guests.sh",
                wasm_path.display()
            );
        })
        .with_tools(Rc::new(tools.clone()) as Rc<dyn spatiotemporal_wasm::ToolHost>);
    root.use_component(Rc::new(wasm));

    let script = ScriptPlugin::from_source(
        "cite",
        load_script("cite.js"),
        script_caps(),
        vec!["markdown".into()],
    )
    .expect("cite.js 语法该是合法的")
    .with_tools(Rc::new(tools.clone()) as Rc<dyn spatiotemporal_script::ToolHost>);
    root.use_component(Rc::new(script));

    app.settle();

    if smoke {
        println!("文档：{}", doc_path.display());
        println!("工具：");
        for tool in tools.list() {
            println!(
                "  · {:<10} [{}] {}",
                tool.name, tool.substrate, tool.description
            );
        }
        println!("--- outline ---");
        println!("{}", tools.call("outline", "{}").expect("outline"));
        println!("--- cite 后进先出 ---");
        println!(
            "{}",
            tools.call("cite", r#"{"input":"后进先出"}"#).expect("cite")
        );
        return;
    }

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);
    println!("文档：{}", doc_path.display());
    println!("打开 http://127.0.0.1:{port}");
    web::serve(port, doc_path, tools, app);
}
