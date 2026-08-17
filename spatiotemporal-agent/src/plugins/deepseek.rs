use std::rc::Rc;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, Result, Steps};

use crate::host::Llm;
use crate::keys::LlmKey;

/// 本地插件：DeepSeek chat completions，提供 `llm` 能力。
#[derive(Clone)]
pub struct DeepSeek {
    pub api_key: Option<String>,
    pub base: String,
    pub model: String,
}

impl DeepSeek {
    pub fn from_config(config: &spatiotemporal::Value) -> Self {
        DeepSeek {
            api_key: config
                .get("api_key")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
                .or_else(|| {
                    std::env::var("DEEPSEEK_API_KEY")
                        .ok()
                        .filter(|s| !s.is_empty())
                }),
            base: config
                .get("base")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
                .or_else(|| std::env::var("DEEPSEEK_BASE_URL").ok())
                .unwrap_or_else(|| "https://api.deepseek.com".into()),
            model: config
                .get("model")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
                .or_else(|| std::env::var("DEEPSEEK_MODEL").ok())
                .unwrap_or_else(|| "deepseek-chat".into()),
        }
    }
}

impl Component for DeepSeek {
    fn name(&self) -> &str {
        "deepseek"
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let client: Rc<dyn Llm> = Rc::new(self.clone());
        Box::pin(async move {
            ctx.set::<LlmKey>(client);
            Ok(())
        })
    }
}

impl Llm for DeepSeek {
    fn model(&self) -> String {
        self.model.clone()
    }

    fn complete(&self, body: serde_json::Value) -> Result<serde_json::Value> {
        let client = self.clone();
        crate::util::call_with_timeout(std::time::Duration::from_secs(120), move || {
            client.complete_blocking(body)
        })
        .and_then(|text| {
            serde_json::from_str(&text).map_err(|error| {
                spatiotemporal::Error::Component(format!("DeepSeek 响应不是 JSON：{error}"))
            })
        })
    }
}

impl DeepSeek {
    fn complete_blocking(&self, body: serde_json::Value) -> Result<String> {
        let key = self
            .api_key
            .as_deref()
            .ok_or_else(|| spatiotemporal::Error::Component("没有 DEEPSEEK_API_KEY".into()))?;
        let url = format!("{}/chat/completions", self.base.trim_end_matches('/'));
        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {key}"))
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(120))
            .send_json(body)
            .map_err(|error| {
                spatiotemporal::Error::Component(format!("DeepSeek 请求失败：{error}"))
            })?;
        response.into_string().map_err(|error| {
            spatiotemporal::Error::Component(format!("DeepSeek 响应读取失败：{error}"))
        })
    }
}
