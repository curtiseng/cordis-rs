//! 把子进程接成时空可组合性演算里的一等 fiber。
//!
//! # 为什么这是一个独立的 crate
//!
//! 基质不是内核概念。[`spatiotemporal::Registry`] 已经是「名字 → 构造器」，
//! 所以 MCP 客户端、任意可执行 guest 只是 [`spatiotemporal::Component`] 的又一种
//! 实现：它的 `apply` 去 spawn 子进程、跑 `load`，把 `kill` / `unload` 用
//! `steps.step_sync` 登记成逆。内核不需要知道 stdio 存在。
//!
//! 分开的实际理由与 `spatiotemporal-wasm` / `spatiotemporal-script` 相同：内核
//! 有 5 个依赖、零 IO；子进程要接 stdin/stdout、可能要 tokio 或手写队列驱动
//! [`spatiotemporal::Spawn`]。
//!
//! # 协议
//!
//! 宿主与 guest 之间用 **NDJSON**（一行一个 JSON 对象）：
//!
//! - `{"id":1,"op":"load","capabilities":{"db":"…"}}`
//! - `{"id":2,"op":"invoke","name":"echo","args":"…"}`
//! - `{"id":3,"op":"unload"}`
//!
//! guest 用同形 `{ "id", "ok", … }` 回应。详见 [`component`] 模块注释。
//!
//! # 三条不会因为写得好而消失的限制
//!
//! 1. **guest 不能引入新的 coeffect 种类给原生插件消费。**
//! 2. **跨边界的值只能是字符串。**
//! 3. **guest 的逆必须可抢占。** 子进程没有 wasm 燃料，用的是 **unload 墙钟超时 +
//!    SIGKILL**。

use std::rc::Rc;

mod component;
mod host;
mod protocol;

pub use component::ProcessPlugin;
pub use host::Capabilities;

/// guest 登记一个工具时，宿主接到的那一面对。
pub trait ToolHost: 'static {
    fn register(&self, name: String, description: String, invoke: ToolInvoke);
    fn unregister(&self, name: &str);
}

/// 调一个子进程工具：参数和返回值都是字符串（通常是 JSON）。
pub type ToolInvoke = Rc<dyn Fn(&str) -> spatiotemporal::Result<String>>;

/// guest 登记自己为 LLM 时，宿主接到的那一面对。
pub trait LlmHost: 'static {
    fn install(
        &self,
        ctx: &spatiotemporal::Context,
        model: String,
        invoke: ToolInvoke,
    ) -> spatiotemporal::Result<()>;
}
