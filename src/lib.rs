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
//! use cordis::{App, Component, Context, Key, Result, State, Steps};
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
//!     fn inject(&self) -> Vec<cordis::KeyId> { vec![cordis::KeyId::of::<Hello>()] }
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
//!
//! 详细的取舍与尚未实现的部分见仓库 README。
//!
//! [Cordis]: https://github.com/cordiverse/cordis

mod component;
mod context;
mod effect;
mod error;
mod fiber;
mod key;
mod runtime;

pub use component::{Component, FnComponent, shared};
pub use context::{Context, FiberHandle};
pub use effect::{EffectHandle, Inverse, Steps};
pub use error::{Error, Result};
pub use fiber::State;
pub use key::{Key, KeyId, RealmId};
pub use runtime::App;
