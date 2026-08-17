use std::rc::Rc;

use futures::future::LocalBoxFuture;
use serde_json::json;
use spatiotemporal::{Component, Context, KeyId, Result, Steps};

use crate::host::{Roster, Surface, Toolbox};
use crate::keys::{Doc, LlmKey, SurfaceKey};

/// 本地插件：smoke / CI 用的界面。打印花名册、跑一遍工具，有 LLM 就 ping 一次。
pub struct Probe {
    pub tools: Toolbox,
    pub roster: Roster,
}

struct ProbeSurface {
    tools: Toolbox,
    roster: Roster,
    doc: String,
    llm: Option<Rc<dyn crate::host::Llm>>,
}

impl Surface for ProbeSurface {
    fn kind(&self) -> &'static str {
        "probe"
    }

    fn run(&self) {
        println!("界面：{}", self.kind());
        println!("文档：{}", self.doc);
        println!("插件：");
        for plugin in self.roster.list() {
            println!(
                "  · {:<12} [{}] {}",
                plugin.name, plugin.substrate, plugin.role
            );
        }
        println!("工具：");
        for tool in self.tools.list() {
            println!(
                "  · {:<10} [{}] {}",
                tool.name, tool.substrate, tool.description
            );
        }
        println!("--- outline ---");
        println!("{}", self.tools.call("outline", "{}").expect("outline"));
        println!("--- cite 后进先出 ---");
        println!(
            "{}",
            self.tools
                .call("cite", r#"{"input":"后进先出"}"#)
                .expect("cite")
        );
        if let Some(llm) = &self.llm {
            println!("--- llm {} ---", llm.model());
            let body = json!({
                "model": llm.model(),
                "messages": [{"role": "user", "content": "ping"}],
            });
            match llm.complete(body) {
                Ok(value) => {
                    let reply = value["choices"][0]["message"]["content"]
                        .as_str()
                        .unwrap_or(&value.to_string())
                        .to_owned();
                    println!("{reply}");
                }
                Err(error) => println!("{error}"),
            }
        }
    }
}

impl Component for Probe {
    fn name(&self) -> &str {
        "probe"
    }

    fn inject(&self) -> Vec<KeyId> {
        vec![KeyId::of::<Doc>(), KeyId::of::<LlmKey>()]
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let tools = self.tools.clone();
        let roster = self.roster.clone();
        Box::pin(async move {
            let doc = ctx.resolve::<Doc>()?;
            let llm = ctx.resolve::<LlmKey>()?;
            let surface: Rc<dyn Surface> = Rc::new(ProbeSurface {
                tools,
                roster,
                doc: doc.path(),
                llm: Some(llm),
            });
            ctx.set::<SurfaceKey>(surface);
            Ok(())
        })
    }
}
