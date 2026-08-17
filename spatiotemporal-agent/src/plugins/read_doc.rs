use std::rc::Rc;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, KeyId, Result, Steps};

use crate::host::Toolbox;
use crate::keys::Doc;

/// 本地插件：把「读全文」登记成工具。
pub struct ReadDoc {
    pub tools: Toolbox,
}

impl Component for ReadDoc {
    fn name(&self) -> &str {
        "read-doc"
    }

    fn inject(&self) -> Vec<KeyId> {
        vec![KeyId::of::<Doc>()]
    }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let tools = self.tools.clone();
        Box::pin(async move {
            let doc = ctx.resolve::<Doc>()?;
            let text = doc.text();
            let invoke = Rc::new(move |_args: &str| -> Result<String> {
                const LIMIT: usize = 24_000;
                if text.len() > LIMIT {
                    Ok(format!(
                        "{}…\n\n（正文已截断，完整 {} 字节。用 outline / cite 看结构或引用。）",
                        &text[..LIMIT],
                        text.len()
                    ))
                } else {
                    Ok(text.clone())
                }
            });
            tools.insert(
                "read_doc".into(),
                "读取当前 Markdown 文档的正文".into(),
                "native",
                invoke,
            );
            let tools = tools.clone();
            steps.step_sync(move || tools.remove("read_doc"))?;
            Ok(())
        })
    }
}
