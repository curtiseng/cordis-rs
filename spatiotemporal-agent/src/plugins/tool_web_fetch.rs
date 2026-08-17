use std::io::Read;
use std::rc::Rc;
use std::time::Duration;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, Result, Steps};

use crate::host::Toolbox;
use crate::tool_schema;
use crate::util::{arg_str, call_with_timeout, parse_json_args};

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BYTES: usize = 512 * 1024;

/// 本地插件：登记 `web_fetch` 工具（HTTP GET，有超时与大小限制）。
pub struct ToolWebFetch {
    pub tools: Toolbox,
}

impl Component for ToolWebFetch {
    fn name(&self) -> &str {
        "tool-web-fetch"
    }

    fn apply(&self, _ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let tools = self.tools.clone();
        Box::pin(async move {
            tools.insert_with_schema(
                "web_fetch".into(),
                "抓取 URL 正文（GET，30 秒超时，最多 512KB）".into(),
                "native",
                tool_schema::web_fetch_schema(),
                Rc::new(|args: &str| {
                    let args = args.to_owned();
                    call_with_timeout(FETCH_TIMEOUT, move || fetch_url(&args))
                }),
            );

            let tools = tools.clone();
            steps.step_sync(move || tools.remove("web_fetch"))?;
            Ok(())
        })
    }
}

fn fetch_url(args: &str) -> Result<String> {
    let value = parse_json_args(args)?;
    let url = arg_str(&value, "url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(spatiotemporal::Error::Component(
            "web_fetch 只支持 http:// 或 https://".into(),
        ));
    }

    let response = ureq::get(url)
        .timeout(FETCH_TIMEOUT)
        .call()
        .map_err(|error| spatiotemporal::Error::Component(format!("请求失败：{error}")))?;

    let status = response.status();
    let content_type = response
        .header("Content-Type")
        .unwrap_or("application/octet-stream")
        .to_owned();

    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|error| spatiotemporal::Error::Component(format!("读响应失败：{error}")))?;

    let truncated = body.len() > MAX_BYTES;
    if truncated {
        body.truncate(MAX_BYTES);
    }

    let text = if content_type.contains("text")
        || content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("javascript")
    {
        String::from_utf8_lossy(&body).into_owned()
    } else {
        format!("[二进制内容，{} 字节，Content-Type: {content_type}]", body.len())
    };

    let mut out = format!("status={status}\ncontent-type={content_type}\nurl={url}\n\n{text}");
    if truncated {
        out.push_str("\n\n…（响应已截断至 512KB）");
    }
    Ok(out)
}
