use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, Key, KeyId, Result, Steps};

type Invoke = Rc<dyn Fn(&str) -> Result<String>>;

/// 当前打开的 Markdown 文档。
pub trait Document {
    fn path(&self) -> String;
    fn text(&self) -> String;
}

enum Doc {}
impl Key for Doc {
    type Api = dyn Document;
    const NAME: &'static str = "markdown";
}

struct FileDoc {
    path: String,
    text: String,
}

impl Document for FileDoc {
    fn path(&self) -> String {
        self.path.clone()
    }
    fn text(&self) -> String {
        self.text.clone()
    }
}

/// 本地插件：读一个文件，提供 `markdown` 能力。
pub struct DocFile {
    doc: Rc<FileDoc>,
}

impl DocFile {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(DocFile {
            doc: Rc::new(FileDoc {
                path: path.display().to_string(),
                text,
            }),
        })
    }
}

impl Component for DocFile {
    fn name(&self) -> &str {
        "doc"
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let doc = self.doc.clone();
        Box::pin(async move {
            ctx.set::<Doc>(doc as Rc<dyn Document>);
            Ok(())
        })
    }
}

/// 本地插件：把「读全文」登记成工具。
pub struct ReadDoc {
    pub tools: Toolbox,
}

impl Component for ReadDoc {
    fn name(&self) -> &str {
        "read_doc"
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

/// DeepSeek 客户端。缺 key 时 `chat` 会给出明确错误，`--smoke` 仍能装上。
pub struct DeepSeek {
    pub api_key: Option<String>,
    pub base: String,
    pub model: String,
}

impl DeepSeek {
    pub fn from_env() -> Self {
        DeepSeek {
            api_key: std::env::var("DEEPSEEK_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            base: std::env::var("DEEPSEEK_BASE_URL")
                .unwrap_or_else(|_| "https://api.deepseek.com".into()),
            model: std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-chat".into()),
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

impl Clone for DeepSeek {
    fn clone(&self) -> Self {
        DeepSeek {
            api_key: self.api_key.clone(),
            base: self.base.clone(),
            model: self.model.clone(),
        }
    }
}

pub trait Llm {
    fn model(&self) -> String;
    fn complete(&self, body: serde_json::Value) -> Result<serde_json::Value>;
}

enum LlmKey {}
impl Key for LlmKey {
    type Api = dyn Llm;
    const NAME: &'static str = "llm";
}

impl Llm for DeepSeek {
    fn model(&self) -> String {
        self.model.clone()
    }

    fn complete(&self, body: serde_json::Value) -> Result<serde_json::Value> {
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
        response.into_json().map_err(|error| {
            spatiotemporal::Error::Component(format!("DeepSeek 响应不是 JSON：{error}"))
        })
    }
}

pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub substrate: String,
}

#[derive(Clone)]
pub struct Toolbox {
    inner: Rc<RefCell<BTreeMap<String, Registered>>>,
}

struct Registered {
    description: String,
    substrate: String,
    invoke: Invoke,
}

impl Toolbox {
    pub fn new() -> Self {
        Toolbox {
            inner: Rc::new(RefCell::new(BTreeMap::new())),
        }
    }

    pub fn insert(&self, name: String, description: String, substrate: &str, invoke: Invoke) {
        self.inner.borrow_mut().insert(
            name,
            Registered {
                description,
                substrate: substrate.to_owned(),
                invoke,
            },
        );
    }

    pub fn remove(&self, name: &str) {
        self.inner.borrow_mut().remove(name);
    }

    pub fn call(&self, name: &str, args: &str) -> Result<String> {
        let invoke = self
            .inner
            .borrow()
            .get(name)
            .map(|tool| tool.invoke.clone())
            .ok_or_else(|| spatiotemporal::Error::Component(format!("没有这个工具：{name}")))?;
        invoke(args)
    }

    pub fn list(&self) -> Vec<ToolInfo> {
        self.inner
            .borrow()
            .iter()
            .map(|(name, tool)| ToolInfo {
                name: name.clone(),
                description: tool.description.clone(),
                substrate: tool.substrate.clone(),
            })
            .collect()
    }

    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.list()
            .into_iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": format!("[{}] {}", tool.substrate, tool.description),
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "input": { "type": "string", "description": "工具的输入，可为空" }
                            }
                        }
                    }
                })
            })
            .collect()
    }
}

impl spatiotemporal_wasm::ToolHost for Toolbox {
    fn register(&self, name: String, description: String, invoke: spatiotemporal_wasm::ToolInvoke) {
        self.insert(name, description, "wasm", invoke);
    }
    fn unregister(&self, name: &str) {
        self.remove(name);
    }
}

impl spatiotemporal_script::ToolHost for Toolbox {
    fn register(
        &self,
        name: String,
        description: String,
        invoke: spatiotemporal_script::ToolInvoke,
    ) {
        self.insert(name, description, "script", invoke);
    }
    fn unregister(&self, name: &str) {
        self.remove(name);
    }
}

pub fn wasm_caps() -> Rc<spatiotemporal_wasm::Capabilities> {
    let mut caps = spatiotemporal_wasm::Capabilities::new();
    caps.expose::<Doc, _>(|doc| doc.text());
    Rc::new(caps)
}

pub fn script_caps() -> Rc<spatiotemporal_script::Capabilities> {
    let mut caps = spatiotemporal_script::Capabilities::new();
    caps.expose::<Doc, _>(|doc| doc.text());
    Rc::new(caps)
}

pub fn wasm_guest(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/guests")
        .join(format!("{name}.wasm"))
}

pub fn load_script(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(name),
    )
    .unwrap_or_else(|error| panic!("读不了脚本 {name}：{error}"))
}

pub fn lookup_doc(ctx: &Context) -> Option<Rc<dyn Document>> {
    ctx.lookup::<Doc>()
}

pub fn lookup_llm(ctx: &Context) -> Option<Rc<dyn Llm>> {
    ctx.lookup::<LlmKey>()
}
