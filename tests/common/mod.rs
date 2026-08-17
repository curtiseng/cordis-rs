//! 测试共用的脚手架。
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use cordis::{Component, Context, KeyId, Result, Steps};
use futures::future::LocalBoxFuture;

/// 观察 effect 与逆的执行顺序。
#[derive(Clone, Default)]
pub struct Log(Rc<RefCell<Vec<String>>>);

impl Log {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, line: impl Into<String>) {
        self.0.borrow_mut().push(line.into());
    }

    pub fn lines(&self) -> Vec<String> {
        self.0.borrow().clone()
    }

    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.0.borrow().iter().any(|line| line == needle)
    }

    /// `a` 是否严格早于 `b`。
    pub fn before(&self, a: &str, b: &str) -> bool {
        let lines = self.0.borrow();
        let ia = lines.iter().position(|line| line == a);
        let ib = lines.iter().position(|line| line == b);
        match (ia, ib) {
            (Some(ia), Some(ib)) => ia < ib,
            _ => false,
        }
    }
}

/// 一个用闭包描述 apply 的测试组件。
pub struct Probe<F> {
    name: &'static str,
    inject: Vec<KeyId>,
    body: F,
}

impl<F> Probe<F>
where
    F: Fn(Context, Steps) -> LocalBoxFuture<'static, Result<()>> + 'static,
{
    pub fn new(name: &'static str, body: F) -> Rc<Self> {
        Rc::new(Probe {
            name,
            inject: Vec::new(),
            body,
        })
    }

    pub fn needs(name: &'static str, inject: Vec<KeyId>, body: F) -> Rc<Self> {
        Rc::new(Probe { name, inject, body })
    }
}

impl<F> Component for Probe<F>
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
        (self.body)(ctx, steps)
    }
}

/// 把控制权交回执行器一次。
///
/// 用来在测试里造出一个确定的 await 点：没有它，一次加载会在调用点被同步驱动
/// 到底，于是「对账进行中又来了一次 apply」这种交错根本无法被观察到。
pub async fn yield_once() {
    let mut yielded = false;
    futures::future::poll_fn(move |cx| {
        if yielded {
            std::task::Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await
}

/// 测试里当作 coeffect 用的最小服务。
pub trait Service {
    fn tag(&self) -> &'static str;
}

pub struct Tagged(pub &'static str);

impl Service for Tagged {
    fn tag(&self) -> &'static str {
        self.0
    }
}
