//! 一个一切都是插件的 agent：宿主只做注册表和加载器。
//!
//! LLM、界面、文档、工具各自是一行配置，基质可以是 native / wasm / script。
//! 换实现的写法跟 dsh 一样：关掉旧行，再 insert 新行。
//!
//! ```bash
//! export DEEPSEEK_API_KEY=sk-...
//! cargo run -p spatiotemporal-agent
//! ```
//!
//! `--smoke` 叠一层 patch：DeepSeek 换成脚本 echo，Web 换成 probe。不需要 API key。

mod chat;
mod host;
mod keys;
mod plugins;
mod registry;

use std::fs;
use std::path::PathBuf;

use spatiotemporal::{App, Loader, Patch, compose, parse_entries, parse_patches};

use crate::host::root_dir;
use crate::keys::lookup_surface;
use crate::registry::{Host, registry};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let smoke = args.iter().any(|a| a == "--smoke");
    let doc_path = args.iter().find(|a| !a.starts_with('-')).map(PathBuf::from);

    let base_path = args
        .windows(2)
        .find(|pair| pair[0] == "--config")
        .map(|pair| PathBuf::from(&pair[1]))
        .unwrap_or_else(|| root_dir().join("cordis.yml"));

    let base_text = fs::read_to_string(&base_path).unwrap_or_else(|error| {
        panic!("读不了 {}：{error}", base_path.display());
    });
    let base = parse_entries(&base_text).unwrap_or_else(|error| panic!("{error}"));

    let mut layers: Vec<Vec<Patch>> = Vec::new();
    if smoke {
        let patch_path = root_dir().join("cordis.smoke.yml");
        let text = fs::read_to_string(&patch_path).unwrap_or_else(|error| {
            panic!("读不了 {}：{error}", patch_path.display());
        });
        layers.push(parse_patches(&text).unwrap_or_else(|error| panic!("{error}")));
    }
    if let Some(path) = &doc_path {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        };
        layers.push(vec![Patch::config(
            "doc",
            serde_json::json!({ "path": path.display().to_string() }),
        )]);
    }

    let composed = compose(&base, &layers).unwrap_or_else(|error| panic!("{error}"));
    for warning in &composed.warnings {
        eprintln!("! {warning}");
    }

    let mut app = App::new();
    let host = Host::new();
    let loader = Loader::new(app.root(), registry(host));
    app.block_on(loader.apply(composed.entries))
        .unwrap_or_else(|error| panic!("装配失败：{error}"));

    let surface = lookup_surface(&app.root()).unwrap_or_else(|| {
        panic!("没有界面插件。配置里需要一行提供 surface 的组件（web 或 probe）。");
    });
    surface.run();
}
