use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use spatiotemporal::{Context, Error, Key, KeyId, KeyRegistry, Result};
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::bindings::composability::plugin::host::Host;

/// 「从上下文里取出一项能力，并投影成 guest 收得下的形状」。
type Projection = Box<dyn Fn(&Context) -> Result<String>>;

/// 宿主愿意让 wasm 插件看见的能力面。
///
/// 每一项都要宿主明确写出**投影**，这不是繁琐设计而是边界的真实形状：跨 WIT 边界
/// 的值只能是 WIT 类型，而原生 coeffect 是 `Rc<dyn Trait>`。所以「guest 不能引入
/// 新的 coeffect 种类」这条限制，在代码里就落成了这张表——表里没有的东西，guest
/// 连名字都报不出来。
///
/// 它同时维护一张 [`KeyRegistry`]：`expose::<K>` 一次把两件事都登记好，
/// 名字既能翻成 [`KeyId`] 填进 `inject`，也能在 guest 调 `capability` 时被投影。
pub struct Capabilities {
    keys: KeyRegistry,
    projections: HashMap<String, Projection>,
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities::new()
    }
}

impl Capabilities {
    pub fn new() -> Self {
        Capabilities {
            keys: KeyRegistry::new(),
            projections: HashMap::new(),
        }
    }

    /// 暴露一项能力：登记它的键，并给出怎么把它投影成 guest 能收到的字符串。
    ///
    /// 用 [`Key::NAME`] 作为 guest 侧的名字。
    pub fn expose<K, F>(&mut self, project: F) -> &mut Self
    where
        K: Key,
        F: Fn(&K::Api) -> String + 'static,
    {
        self.keys.add::<K>();
        self.projections.insert(
            K::NAME.to_owned(),
            Box::new(move |ctx: &Context| {
                let api = ctx.resolve::<K>()?;
                Ok(project(&api))
            }),
        );
        self
    }

    /// 把 guest 侧的名字翻成可填进 `Component::inject` 的键。
    pub fn inject(&self, granted: &[String]) -> Result<Vec<KeyId>> {
        self.keys.resolve_all(granted)
    }

    /// 已暴露的能力名，排序后返回。
    pub fn names(&self) -> Vec<&str> {
        self.keys.names()
    }

    /// 在加载时刻取一次能力快照。
    ///
    /// 快照不是对动态性的妥协，而恰好是这套语义：论文里一个 fiber 的 committed
    /// view 在它整段活跃期内是固定的，依赖一变它就被重载。所以「装上时取一次」
    /// 与「每次调用都去查」在可观察行为上没有差别——而快照顺带挡掉了重入：
    /// store 里不放 `Context`，guest 就没法在自己的转换还没结束时回头调内核。
    pub(crate) fn snapshot(
        &self,
        ctx: &Context,
        granted: &[String],
    ) -> Result<HashMap<String, String>> {
        granted
            .iter()
            .map(|name| {
                let project = self
                    .projections
                    .get(name)
                    .ok_or_else(|| Error::UnknownKey(name.clone()))?;
                Ok((name.clone(), project(ctx)?))
            })
            .collect()
    }
}

/// 一个 wasm 实例的宿主侧状态。
///
/// 每个字段都是拥有所有权且 `Send` 的——不是为了并发，而是因为
/// `wasmtime_wasi::p2::add_to_linker_sync` 要求 `T: WasiView`，而 `WasiView: Send`。
/// 这条约束反过来帮了忙：它从类型上就不允许把 `Rc<Context>` 塞进 store。
pub struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
    view: HashMap<String, String>,
    logs: Arc<Mutex<Vec<String>>>,
    /// `load` 期间 guest 报上来的工具。真正接到 [`ToolHost`] 是 `apply` 的事。
    pending_tools: Arc<Mutex<Vec<(String, String)>>>,
}

impl HostState {
    pub(crate) fn new(
        view: HashMap<String, String>,
        logs: Arc<Mutex<Vec<String>>>,
        pending_tools: Arc<Mutex<Vec<(String, String)>>>,
    ) -> Self {
        HostState {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
            view,
            logs,
            pending_tools,
        }
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl Host for HostState {
    fn log(&mut self, message: String) {
        if let Ok(mut logs) = self.logs.lock() {
            logs.push(message);
        }
    }

    fn capability(&mut self, name: String) -> std::result::Result<String, String> {
        // 没授予就是没有。guest 报一个没在配置里授予的名字，这里给的答案与
        // 「宿主根本没登记过这项能力」完全一样——它无从区分，也就无从探测。
        self.view
            .get(&name)
            .cloned()
            .ok_or_else(|| format!("没有授予这项能力：{name}"))
    }

    fn register_tool(&mut self, name: String, description: String) {
        if let Ok(mut pending) = self.pending_tools.lock() {
            pending.push((name, description));
        }
    }
}
