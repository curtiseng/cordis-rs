//! 把一段 QuickJS 脚本接成时空可组合性演算里的一等 fiber。
//!
//! # 为什么这是一个独立的 crate
//!
//! 基质不是内核概念。[`spatiotemporal::Registry`] 已经是「名字 → 构造器」，
//! 所以一段模型现写的 JavaScript 只是 [`spatiotemporal::Component`] 的又一种实现：
//! 它的 `apply` 去跑 `load`，把 `unload` 用 `steps.step_sync` 登记成逆。内核不需要
//! 知道 QuickJS 存在。
//!
//! 分开的实际理由与 `spatiotemporal-wasm` 相同：内核有 5 个依赖、零 IO、MSRV 1.85；
//! rquickjs 要编一段 C、MSRV 1.87。想读演算的人不该被迫链接一个 JS 引擎。
//!
//! # 与 wasm 适配器的差别
//!
//! wasm 的 guest 是编译好的 `.wasm` 文件；脚本的 guest 是**字符串**——这正是
//! 「模型这一轮现写一段代码」那条自进化路径。能力面是同一张形状：宿主注入
//! `host.log` / `host.capability`，guest 导出 `load` / `unload`。
//!
//! # 内核为此让出的三处
//!
//! - [`spatiotemporal::Component::name`] 返回 `&str`：名字由调用方给出，模型现写的
//!   代码没有编译期名字。
//! - [`spatiotemporal::KeyRegistry`]：guest 只能用字符串声明依赖。
//! - [`spatiotemporal::Spawn`]：脚本这边用不到异步宿主调用，但口子在。
//!
//! # 三条不会因为写得好而消失的限制
//!
//! 1. **guest 不能引入新的 coeffect 种类给原生插件消费。** 投影表里没有的东西，
//!    guest 连名字都报不出来。
//! 2. **跨边界的值只能是字符串。** 叶子工具是甜点区。
//! 3. **guest 的逆必须可抢占。** QuickJS 没有 wasmtime 那种指令燃料，用的是
//!    [`Runtime::set_interrupt_handler`]：引擎定期问一次「该停了吗」。一个
//!    `unload` 里死循环的脚本会被它打断——这件事适配器自己做得完。

mod component;
mod host;

pub use component::ScriptPlugin;
pub use host::Capabilities;
