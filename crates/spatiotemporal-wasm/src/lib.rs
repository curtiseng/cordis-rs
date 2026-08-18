//! 把 WebAssembly 组件接成时空可组合性演算里的一等 fiber。
//!
//! # 为什么这是一个独立的 crate
//!
//! 因为**基质不是内核概念**。[`spatiotemporal::Registry`] 已经是「名字 → 构造器」，
//! 所以一个 wasm 插件只是 [`spatiotemporal::Component`] 的又一种实现：它的 `apply`
//! 去实例化一个 wasm 组件，把 guest 的 `unload` 用 `steps.step` 登记成逆。内核不需要
//! 知道 wasm 存在。
//!
//! 分开的实际理由是依赖性质：内核有 5 个依赖、零 IO；wasmtime 一家就带
//! 上百个 crate。想读演算的人不该被迫编译一个 wasm 运行时。
//!
//! # 内核为此让出的三处
//!
//! - [`spatiotemporal::Component::name`] 返回 `&str`：名字来自 `.wasm` 文件。
//! - [`spatiotemporal::KeyRegistry`]：guest 只能用字符串声明依赖，宿主负责翻译，
//!   于是 guest 说不出宿主没登记的能力。
//! - [`spatiotemporal::Spawn`]：宿主自带执行器，wasm 的异步调用才有地方跑。
//!
//! # 三条不会因为写得好而消失的限制
//!
//! 1. **guest 不能引入新的 coeffect 种类给原生插件消费。** 原生侧拿到的是
//!    `Rc<dyn Trait>`，那个 trait 必须在宿主编译期就存在。所以 `wit/plugin.wit`
//!    里的 world 决定了 guest 能提供什么，而不是 guest 自己决定。
//! 2. **大 payload 的高频事件过边界要付序列化代价。** 叶子工具（调用稀疏、
//!    payload 小）是甜点区；流式事件不是。
//! 3. **guest 的逆必须可抢占。** 一个卡住的 `unload` 会拖死整次卸载，因此这里给它
//!    设期限并用 epoch 打断——这件事适配器自己做得完，不需要内核配合。

use std::rc::Rc;

mod component;
mod host;

/// bindgen 的产物关在这里，免得 WIT 的包名跑到 crate 根上。
mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "plugin",
    });
}

pub use component::WasmPlugin;
pub use host::{Capabilities, HostState};

/// guest 登记一个工具时，宿主接到的那一面对。
///
/// 适配器在 `load` 成功之后把每个工具接进来，并在这个 fiber 的逆里拆掉。
/// 撤销由宿主持有，guest 破坏不了自己的清理。
pub trait ToolHost: 'static {
    fn register(&self, name: String, description: String, invoke: ToolInvoke);
    fn unregister(&self, name: &str);
    /// 桥接宿主工具表；未接时 guest 调 `call-tool` 会失败。
    fn call_tool(&self, _name: &str, _args: &str) -> spatiotemporal::Result<String> {
        Err(spatiotemporal::Error::Component(
            "宿主未接 tool 桥接".into(),
        ))
    }
}

/// 调一个 wasm 工具：参数和返回值都是字符串（通常是 JSON）。
pub type ToolInvoke = Rc<dyn Fn(&str) -> spatiotemporal::Result<String>>;

/// guest 登记自己为 LLM 时，宿主接到的那一面对。
///
/// 适配器在 `load` 成功之后调用 [`LlmHost::install`]，把 guest 的 `invoke("__llm")`
/// 包成宿主的 coeffect。安装发生在这个 fiber 的 `apply` 里，所以 `ctx.set` 的
/// 逆跟着这个 fiber 走——guest 破坏不了自己的清理。
pub trait LlmHost: 'static {
    fn install(
        &self,
        ctx: &spatiotemporal::Context,
        model: String,
        invoke: ToolInvoke,
    ) -> spatiotemporal::Result<()>;
}
