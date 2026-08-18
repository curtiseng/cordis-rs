use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use serde_json::Value;
use spatiotemporal::{Component, Context, KeyId, Result, Steps};

use crate::keys::LlmKey;
use crate::tool_schema;

type Invoke = Rc<dyn Fn(&str) -> Result<String>>;

/// 当前打开的 Markdown 文档。
pub trait Document {
    fn path(&self) -> String;
    fn text(&self) -> String;
}

/// 对话用的语言模型。原生插件 `ctx.set`，guest 走 `registerLlm`。
pub trait Llm {
    fn model(&self) -> String;
    fn complete(&self, body: serde_json::Value) -> Result<serde_json::Value>;
}

/// 工作区文件系统（路径限制在 root 内）。
pub trait Fs {
    fn root(&self) -> &Path;
}

/// Shell 执行（cwd 限制在 root 内）。
pub trait Shell {
    fn root(&self) -> &Path;
}

/// 界面。`apply` 只把实现挂到 `surface` 上；真正跑起来是宿主 `run` 的事。
pub trait Surface {
    fn kind(&self) -> &'static str;
    fn run(&self);
}

/// 组装 system prompt（`system-prompt` 插件提供）。
pub struct PromptInput<'a> {
    pub workspace: &'a str,
    pub doc_path: Option<&'a str>,
    pub profile: crate::runtime::AgentProfile,
    pub tools: &'a Toolbox,
}

pub trait SystemPrompt {
    fn build(&self, input: PromptInput<'_>) -> String;
}

/// 可插拔 agent 循环（`agent-loop` 插件提供）。
pub trait AgentLoop {
    fn run_turn(&self, request: crate::chat::TurnRequest<'_>) -> crate::chat::Turn;
}

pub struct PluginInfo {
    pub name: String,
    pub substrate: String,
    pub role: String,
}

#[derive(Clone)]
pub struct Roster {
    inner: Rc<RefCell<Vec<PluginInfo>>>,
}

impl Roster {
    pub fn new() -> Self {
        Roster {
            inner: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn push(&self, info: PluginInfo) {
        self.inner.borrow_mut().push(info);
    }

    pub fn remove(&self, name: &str) {
        self.inner.borrow_mut().retain(|item| item.name != name);
    }

    pub fn list(&self) -> Vec<PluginInfo> {
        self.inner
            .borrow()
            .iter()
            .map(|item| PluginInfo {
                name: item.name.clone(),
                substrate: item.substrate.clone(),
                role: item.role.clone(),
            })
            .collect()
    }
}

/// 把一次装载记到花名册上。逆跟着这个 fiber 走。
pub struct Announcing {
    inner: Rc<dyn Component>,
    roster: Roster,
    substrate: String,
    role: String,
}

impl Announcing {
    pub fn wrap(
        inner: Rc<dyn Component>,
        roster: Roster,
        substrate: impl Into<String>,
        role: impl Into<String>,
    ) -> Rc<dyn Component> {
        Rc::new(Announcing {
            inner,
            roster,
            substrate: substrate.into(),
            role: role.into(),
        })
    }
}

impl Component for Announcing {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn inject(&self) -> Vec<KeyId> {
        self.inner.inject()
    }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let inner = self.inner.clone();
        let roster = self.roster.clone();
        let name = inner.name().to_owned();
        let info = PluginInfo {
            name: name.clone(),
            substrate: self.substrate.clone(),
            role: self.role.clone(),
        };
        Box::pin(async move {
            roster.push(info);
            let roster = roster.clone();
            steps.step_sync(move || roster.remove(&name))?;
            inner.apply(ctx, steps).await
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
    parameters: Value,
    invoke: Invoke,
}

impl Toolbox {
    pub fn new() -> Self {
        Toolbox {
            inner: Rc::new(RefCell::new(BTreeMap::new())),
        }
    }

    pub fn insert(&self, name: String, description: String, substrate: &str, invoke: Invoke) {
        self.insert_with_schema(
            name,
            description,
            substrate,
            tool_schema::text_query("工具输入"),
            invoke,
        );
    }

    pub fn insert_with_schema(
        &self,
        name: String,
        description: String,
        substrate: &str,
        parameters: Value,
        invoke: Invoke,
    ) {
        self.inner.borrow_mut().insert(
            name,
            Registered {
                description,
                substrate: substrate.to_owned(),
                parameters,
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

    pub fn schemas(&self) -> Vec<Value> {
        self.inner
            .borrow()
            .iter()
            .map(|(name, tool)| {
                tool_schema::function_schema(
                    name,
                    &format!("[{}] {}", tool.substrate, tool.description),
                    tool.parameters.clone(),
                )
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

impl spatiotemporal_process::ToolHost for Toolbox {
    fn register(
        &self,
        name: String,
        description: String,
        invoke: spatiotemporal_process::ToolInvoke,
    ) {
        self.insert(name, description, "process", invoke);
    }
    fn unregister(&self, name: &str) {
        self.remove(name);
    }
}

/// guest 的 `registerLlm` 落到 `llm` 这个键上。
pub struct LlmInstaller;

impl spatiotemporal_wasm::LlmHost for LlmInstaller {
    fn install(
        &self,
        ctx: &Context,
        model: String,
        invoke: spatiotemporal_wasm::ToolInvoke,
    ) -> Result<()> {
        ctx.set::<LlmKey>(Rc::new(GuestLlm { model, invoke }) as Rc<dyn Llm>);
        Ok(())
    }
}

impl spatiotemporal_script::LlmHost for LlmInstaller {
    fn install(
        &self,
        ctx: &Context,
        model: String,
        invoke: spatiotemporal_script::ToolInvoke,
    ) -> Result<()> {
        ctx.set::<LlmKey>(Rc::new(GuestLlm { model, invoke }) as Rc<dyn Llm>);
        Ok(())
    }
}

impl spatiotemporal_process::LlmHost for LlmInstaller {
    fn install(
        &self,
        ctx: &Context,
        model: String,
        invoke: spatiotemporal_process::ToolInvoke,
    ) -> Result<()> {
        ctx.set::<LlmKey>(Rc::new(GuestLlm { model, invoke }) as Rc<dyn Llm>);
        Ok(())
    }
}

struct GuestLlm {
    model: String,
    invoke: Invoke,
}

impl Llm for GuestLlm {
    fn model(&self) -> String {
        self.model.clone()
    }

    fn complete(&self, body: serde_json::Value) -> Result<serde_json::Value> {
        let text = (self.invoke)(&body.to_string())?;
        serde_json::from_str(&text).map_err(|error| {
            spatiotemporal::Error::Component(format!("guest LLM 返回的不是 JSON：{error}"))
        })
    }
}

pub fn wasm_caps() -> Rc<spatiotemporal_wasm::Capabilities> {
    let mut caps = spatiotemporal_wasm::Capabilities::new();
    caps.expose::<crate::keys::Doc, _>(|doc| doc.text());
    Rc::new(caps)
}

pub fn script_caps() -> Rc<spatiotemporal_script::Capabilities> {
    let mut caps = spatiotemporal_script::Capabilities::new();
    caps.expose::<crate::keys::Doc, _>(|doc| doc.text());
    Rc::new(caps)
}

pub fn process_caps() -> Rc<spatiotemporal_process::Capabilities> {
    let mut caps = spatiotemporal_process::Capabilities::new();
    caps.expose::<crate::keys::Doc, _>(|doc| doc.text());
    Rc::new(caps)
}

pub fn root_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn wasm_guest(name: &str) -> PathBuf {
    root_dir()
        .join("target/guests")
        .join(format!("{name}.wasm"))
}

pub fn process_guest(name: &str) -> PathBuf {
    root_dir().join("target/guests").join(name)
}

pub fn resolve_path(path: &str) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root_dir().join(candidate)
    }
}

pub fn grant_list(config: &spatiotemporal::Value) -> Vec<String> {
    config
        .get("grant")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub fn config_str<'a>(config: &'a spatiotemporal::Value, key: &str) -> Option<&'a str> {
    config.get(key).and_then(spatiotemporal::Value::as_str)
}
