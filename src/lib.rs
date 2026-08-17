//! 时空可组合性的 Rust 实现。
//!
//! 这是论文《A Programming Paradigm for Spatiotemporal Composability》第 5.1 节
//! 那个核心库的 Rust 版本——原文的参考实现 [Cordis] 是 TypeScript 写的。
//! 本 crate 只做核心库这一层：可撤销 effect、响应式 coeffect、fiber 的惯性
//! 生命周期状态机，以及类型化的上下文访问。
//!
//! # 三个概念
//!
//! - **可撤销 effect**：每个改动上下文的动作都配一个逆。逆是值（[`Inverse`]），
//!   在卸载时按 LIFO 回放。组件作者不写卸载路径。
//! - **响应式 coeffect**：组件声明它需要哪些键（[`Component::inject`]）；
//!   提供者出现即激活它，提供者离开即去激活它。
//! - **惯性**：一次转换（加载或卸载）一旦开始就运行至完成，期间的目标变化被
//!   记下但不打断它；完成时若目标已变，就链接进下一次转换。
//!
//! # 一个例子
//!
//! ```
//! use spatiotemporal::{App, Component, Context, Key, Result, State, Steps};
//! use futures::future::LocalBoxFuture;
//! use std::cell::RefCell;
//! use std::rc::Rc;
//!
//! // 一项 coeffect：键是标记类型，接口是 trait 对象。
//! trait Greeter {
//!     fn greet(&self) -> String;
//! }
//! enum Hello {}
//! impl Key for Hello {
//!     type Api = dyn Greeter;
//!     const NAME: &'static str = "hello";
//! }
//!
//! // 提供者：把实现装到上下文上，卸载时自动摘掉。
//! struct English;
//! impl Greeter for English {
//!     fn greet(&self) -> String {
//!         "hi".into()
//!     }
//! }
//! struct Provider;
//! impl Component for Provider {
//!     fn name(&self) -> &'static str { "provider" }
//!     fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
//!         Box::pin(async move {
//!             ctx.set::<Hello>(Rc::new(English));
//!             Ok(())
//!         })
//!     }
//! }
//!
//! // 消费者：声明依赖，被激活时才跑。
//! struct Consumer(Rc<RefCell<Vec<String>>>);
//! impl Component for Consumer {
//!     fn name(&self) -> &'static str { "consumer" }
//!     fn inject(&self) -> Vec<spatiotemporal::KeyId> { vec![spatiotemporal::KeyId::of::<Hello>()] }
//!     fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
//!         let log = self.0.clone();
//!         Box::pin(async move {
//!             log.borrow_mut().push(ctx.resolve::<Hello>()?.greet());
//!             Ok(())
//!         })
//!     }
//! }
//!
//! let log = Rc::new(RefCell::new(Vec::new()));
//! let mut app = App::new();
//! let root = app.root();
//!
//! // 先挂消费者：依赖还不在，它保持非活动且不报错。
//! let consumer = root.use_component(Rc::new(Consumer(log.clone())));
//! app.settle();
//! assert_eq!(consumer.state(), State::Inactive);
//!
//! // 提供者一到，消费者自己就活了。
//! let provider = root.use_component(Rc::new(Provider));
//! app.settle();
//! assert_eq!(consumer.state(), State::Active);
//! assert_eq!(&*log.borrow(), &["hi".to_string()]);
//!
//! // 提供者一走，消费者又自己退回非活动。
//! app.block_on(provider.dispose());
//! assert_eq!(consumer.state(), State::Inactive);
//! ```
//!
//! # 声明式配置层
//!
//! [`Loader`] 是论文 5.2.1 节那一层：一棵配置树对账成一组活着的 fiber，配置变了
//! 就把差异增量地施加上去。它不含任何新机制——每次变更都归约成
//! [`Context::use_component`] 与 [`FiberHandle::dispose`]，所以**配置热重载是
//! 可撤销 effect 的一个应用，而不是它之外的另一套东西**。
//!
//! ```
//! use spatiotemporal::{App, Entry, FnComponent, Loader, Registry, State};
//! use std::rc::Rc;
//!
//! let mut registry = Registry::new();
//! registry.add("noop", |_config| {
//!     Ok(Rc::new(FnComponent::new("noop", |_ctx, _steps| {
//!         Box::pin(async { Ok(()) })
//!     })) as Rc<dyn spatiotemporal::Component>)
//! });
//!
//! let mut app = App::new();
//! let loader = Loader::new(app.root(), registry);
//!
//! // 一份配置 → 一组 fiber。
//! app.block_on(loader.apply(vec![Entry::new("a", "noop"), Entry::new("b", "noop")])).unwrap();
//! assert_eq!(loader.state("a"), Some(State::Active));
//!
//! // 关掉一行：只有那一行被拆，别的不动。
//! let applied = app
//!     .block_on(loader.apply(vec![Entry::new("a", "noop").disabled(), Entry::new("b", "noop")]))
//!     .unwrap();
//! assert_eq!(applied.removed, vec!["a"]);
//! assert_eq!(loader.ids(), vec!["b"]);
//! ```
//!
//! 文件监听刻意留在库外面——它属于宿主的职责。`cargo run --example watch_config`
//! 有一个用 `notify` 接上的完整演示，包括「写坏配置不会杀死运行中的树」。
//!
//! # 给动态基质留的三个口子
//!
//! 原生组件全是编译期的：名字是字面量，依赖写成 `KeyId::of::<K>()`，执行器用库
//! 自带的那个。一个 wasm 组件、一段模型现写的代码、一个子进程都不是。内核为此
//! 让出三处，**但基质本身不是内核概念**——它们都只是 [`Component`] 的不同实现，
//! 由 [`Registry`] 里的构造器装出来，所以适配器属于独立的 crate。
//!
//! - [`Component::name`] 返回 `&str` 而非 `&'static str`：名字可以来自 wasm 文件
//!   或运行时拼出来。
//! - [`KeyRegistry`] 把名字翻成 [`KeyId`]：guest 只能用字符串说「我需要 tools」，
//!   而这张表由宿主建立，所以 **guest 说不出宿主没登记的能力**。
//! - [`Spawn`] 与 [`Kernel`]：宿主可以带自己的执行器。内核本身不含任何 IO，要让
//!   子进程或套接字成为一等 fiber，就得把带 IO 的执行器接进来。
//!
//! # 与论文的对应
//!
//! | 论文 | 这里 |
//! |---|---|
//! | $\Gamma_\infty$ | [`Context`] |
//! | $\mathrm{effect}_\Gamma(e)$ | [`Context::effect`] |
//! | $\mathfrak{E}^{\mathrm{iter}}_\Gamma$ | [`Steps::step`]（`gen` 块未稳定，改用登记面） |
//! | $\mathrm{set}(k,v)$、$\mathrm{get}(k)$ | [`Context::set`]、[`Context::lookup`] |
//! | $\mathrm{isolate}(k,r)$ | [`Context::isolate`] |
//! | 算法 6 的 proxy 中介访问 | [`Context::resolve`]（Rust 没有 `Proxy`，改用类型化访问器） |
//! | fiber、`fiber.uid` | [`FiberHandle`]、代际索引 |
//! | `fiber.state`（定义 44 的 $\theta$） | [`State`] |
//! | `fiber.inertia` | `Shared` future |
//! | **O-Insert** / **O-Retire** | [`Context::use_component`] / [`FiberHandle::dispose`] |
//! | 5.2.1 节的组件加载器 | [`Loader`]、[`Registry`]、[`compose`] |
//! | 注 2 的 `create_task` | [`Spawn`]、[`Kernel`]（宿主可自带执行器） |
//!
//! 详细的取舍与尚未实现的部分见仓库 README。
//!
//! [Cordis]: https://github.com/cordiverse/cordis

mod component;
mod context;
mod effect;
mod entry;
mod error;
mod fiber;
mod key;
mod loader;
mod registry;
mod runtime;

pub use component::{Component, FnComponent, shared};
pub use context::{Context, FiberHandle};
pub use effect::{EffectHandle, Inverse, Steps};
pub use entry::{Composed, Entry, Patch, compose, parse_entries, parse_patches};
pub use error::{Error, Result};
pub use fiber::State;
pub use key::{Key, KeyId, KeyRegistry, RealmId};
pub use loader::{Applied, Loader};
pub use registry::{Factory, Registry};
pub use runtime::{App, Kernel, Spawn};

/// 重新导出：配置值的类型出现在 [`Entry::config`] 与 [`Factory`] 的签名里，
/// 调用方不该为了写一行配置去猜该配哪个版本的 serde_json。
pub use serde_json::Value;
