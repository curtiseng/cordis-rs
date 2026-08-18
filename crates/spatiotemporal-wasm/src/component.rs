use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, Error, KeyId, Result, Steps};
use wasmtime::component::{HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

use crate::bindings::Plugin;
use crate::host::{Capabilities, HostState, set_tool_bridge};
use crate::{LlmHost, ToolHost, ToolInvoke};

/// 默认给 guest 的燃料额度。
///
/// 用燃料而不是墙钟期限，是因为它不需要另起一个线程去推 epoch，而且是确定性的：
/// 同一个 guest 每次都在同一条指令上耗尽。代价是它只约束**指令数**——一个卡在
/// 宿主调用里的 guest 不受燃料约束，不过这里的宿主函数都不阻塞。
const DEFAULT_FUEL: u64 = 100_000_000;

/// 一个被接成 fiber 的 wasm 组件。
///
/// 这个类型是整件事的全部：它就是 [`Component`] 的又一种实现。内核不知道 wasm
/// 存在，[`spatiotemporal::Registry`] 也只是把一个名字映到「造一个这玩意儿」。
pub struct WasmPlugin {
    engine: Engine,
    component: wasmtime::component::Component,
    name: String,
    /// 宿主在配置里**授予**的能力名。
    ///
    /// 刻意由配置给出，而不是去问 guest 要什么：`inject` 属于配置的一部分，
    /// 应当能被静态检视，而不是非得先把 guest 跑起来才知道。这也让它成为一个
    /// 授权模型——第三方送来一个 `.wasm`，是运维决定它能看见什么。
    granted: Vec<String>,
    inject: Vec<KeyId>,
    capabilities: Rc<Capabilities>,
    fuel: u64,
    logs: Arc<Mutex<Vec<String>>>,
    tools: Option<Rc<dyn ToolHost>>,
    llm: Option<Rc<dyn LlmHost>>,
}

impl WasmPlugin {
    /// 编译一个 `.wasm` 组件，并把授予它的能力名翻成 coeffect 键。
    ///
    /// 名字取自文件名——这正是内核把 [`Component::name`] 从 `&'static str` 放宽
    /// 成 `&str` 的原因。
    pub fn open(
        path: impl AsRef<Path>,
        capabilities: Rc<Capabilities>,
        granted: Vec<String>,
    ) -> Result<Self> {
        let path = path.as_ref();
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "wasm".to_owned());

        // 全有或全无：授予了一个宿主没暴露的名字就整体拒绝，绝不静默降级——
        // 一个少了某项依赖的组件会被激活得太早。
        let inject = capabilities.inject(&granted)?;

        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(wasm_error("建不出 wasm 引擎"))?;
        let component = wasmtime::component::Component::from_file(&engine, path)
            .map_err(wasm_error(&format!("编不动 {}", path.display())))?;

        Ok(WasmPlugin {
            engine,
            component,
            name,
            granted,
            inject,
            capabilities,
            fuel: DEFAULT_FUEL,
            logs: Arc::new(Mutex::new(Vec::new())),
            tools: None,
            llm: None,
        })
    }

    /// 把 guest 在 `load` 里登记的工具接到宿主的工具表上。
    pub fn with_tools(mut self, tools: Rc<dyn ToolHost>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// 把 guest 在 `load` 里登记的 LLM 接到宿主的 `llm` 键上。
    pub fn with_llm(mut self, llm: Rc<dyn LlmHost>) -> Self {
        self.llm = Some(llm);
        self
    }

    /// 改燃料额度。
    pub fn with_fuel(mut self, fuel: u64) -> Self {
        self.fuel = fuel;
        self
    }

    /// guest 至今记下的日志。
    pub fn logs(&self) -> Vec<String> {
        self.logs
            .lock()
            .map(|logs| logs.clone())
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for WasmPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPlugin")
            .field("name", &self.name)
            .field("granted", &self.granted)
            .field("fuel", &self.fuel)
            .finish_non_exhaustive()
    }
}

impl Component for WasmPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn inject(&self) -> Vec<KeyId> {
        self.inject.clone()
    }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let view = self.capabilities.snapshot(&ctx, &self.granted)?;
            let pending_tools = Arc::new(Mutex::new(Vec::new()));
            let pending_llm = Arc::new(Mutex::new(None));

            set_tool_bridge(self.tools.clone());

            let mut store = Store::new(
                &self.engine,
                HostState::new(
                    view,
                    self.logs.clone(),
                    pending_tools.clone(),
                    pending_llm.clone(),
                ),
            );
            store
                .set_fuel(self.fuel)
                .map_err(wasm_error("加不进燃料"))?;

            let mut linker = Linker::new(&self.engine);
            wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
                .map_err(wasm_error("接不上 wasi"))?;
            Plugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
                .map_err(wasm_error("接不上宿主接口"))?;

            let bindings = Plugin::instantiate(&mut store, &self.component, &linker)
                .map_err(wasm_error("实例化失败"))?;

            bindings
                .composability_plugin_lifecycle()
                .call_load(&mut store)
                .map_err(wasm_error("load 陷入"))?
                .map_err(|message| Error::Component(format!("guest 的 load 失败：{message}")))?;

            let live = Rc::new(RefCell::new(Live { store, bindings }));
            let registered: Vec<(String, String)> = pending_tools
                .lock()
                .map(|items| items.clone())
                .unwrap_or_default();

            if let Some(host) = &self.tools {
                for (name, description) in registered {
                    let live = live.clone();
                    let fuel = self.fuel;
                    let invoke_name = name.clone();
                    let invoke: ToolInvoke = Rc::new(move |args: &str| {
                        live.borrow_mut().invoke(&invoke_name, args, fuel)
                    });
                    host.register(name.clone(), description, invoke);
                    let host = host.clone();
                    steps.step_sync(move || host.unregister(&name))?;
                }
            }

            let model = pending_llm.lock().ok().and_then(|slot| slot.clone());
            if let Some(model) = model {
                let Some(host) = &self.llm else {
                    return Err(Error::Component("guest 登记了 LLM，但宿主没接".into()));
                };
                let live = live.clone();
                let fuel = self.fuel;
                let invoke: ToolInvoke =
                    Rc::new(move |args: &str| live.borrow_mut().invoke("__llm", args, fuel));
                host.install(&ctx, model, invoke)?;
            }

            let fuel = self.fuel;
            let name = self.name.clone();
            let logs = self.logs.clone();
            steps.step_sync(move || {
                set_tool_bridge(None);
                let outcome = live.borrow_mut().unload(fuel);
                if let Err(error) = outcome
                    && let Ok(mut logs) = logs.lock()
                {
                    logs.push(format!("{name} 的 unload 陷入了：{error}"));
                }
            })
        })
    }
}

struct Live {
    store: Store<HostState>,
    bindings: Plugin,
}

impl Live {
    fn invoke(&mut self, name: &str, args: &str, fuel: u64) -> Result<String> {
        let _ = self.store.set_fuel(fuel);
        let (store, bindings) = (&mut self.store, &self.bindings);
        bindings
            .composability_plugin_lifecycle()
            .call_invoke(store, name, args)
            .map_err(wasm_error("invoke 陷入"))?
            .map_err(|message| Error::Component(format!("工具 {name} 失败：{message}")))
    }

    fn unload(&mut self, fuel: u64) -> wasmtime::Result<()> {
        let _ = self.store.set_fuel(fuel);
        let (store, bindings) = (&mut self.store, &self.bindings);
        bindings.composability_plugin_lifecycle().call_unload(store)
    }
}

fn wasm_error(context: &str) -> impl FnOnce(wasmtime::Error) -> Error + use<'_> {
    move |error| Error::Component(format!("{context}：{error}"))
}
