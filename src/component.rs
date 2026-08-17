use std::rc::Rc;

use futures::future::LocalBoxFuture;

use crate::context::Context;
use crate::effect::Steps;
use crate::error::Result;
use crate::key::{Key, KeyId};

/// 一个可被实例化的组件。
///
/// 对应论文定义 40 的 $\mathfrak{C}_\Gamma$：一个组件把 coeffect 规格 $d$
/// （[`Component::inject`]）与 effect 函数 $e$（[`Component::apply`]）配对。
/// 配置住在组件值自己身上——Rust 里没必要像 TypeScript 那样把 config 单独套进去。
pub trait Component: 'static {
    fn name(&self) -> &'static str;

    /// coeffect 规格：这个组件需要哪些键才能被激活。
    fn inject(&self) -> Vec<KeyId> {
        Vec::new()
    }

    /// 组件的 effect 函数。每完成一小步就用 `steps.step(..)?` 登记它的逆。
    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>>;
}

/// 用闭包写一个组件。
///
/// ```
/// use cordis::{App, FnComponent, State};
/// use std::rc::Rc;
///
/// let mut app = App::new();
/// let handle = app.root().use_component(Rc::new(FnComponent::new("noop", |_ctx, _steps| {
///     Box::pin(async { Ok(()) })
/// })));
/// app.settle();
/// assert_eq!(handle.state(), State::Active);
/// ```
pub struct FnComponent<F> {
    name: &'static str,
    inject: Vec<KeyId>,
    apply: F,
}

impl<F> FnComponent<F>
where
    F: Fn(Context, Steps) -> LocalBoxFuture<'static, Result<()>> + 'static,
{
    pub fn new(name: &'static str, apply: F) -> Self {
        FnComponent {
            name,
            inject: Vec::new(),
            apply,
        }
    }

    /// 声明一项依赖。
    pub fn needs<K: Key>(mut self) -> Self {
        self.inject.push(KeyId::of::<K>());
        self
    }
}

impl<F> Component for FnComponent<F>
where
    F: Fn(Context, Steps) -> LocalBoxFuture<'static, Result<()>> + 'static,
{
    fn name(&self) -> &'static str {
        self.name
    }

    fn inject(&self) -> Vec<KeyId> {
        self.inject.clone()
    }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        (self.apply)(ctx, steps)
    }
}

/// 把组件包成可实例化的共享值。
pub fn shared<C: Component>(component: C) -> Rc<dyn Component> {
    Rc::new(component)
}
