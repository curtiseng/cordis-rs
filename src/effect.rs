use std::future::Future;
use std::rc::Rc;

use futures::future::LocalBoxFuture;

use crate::error::{Error, Result};
use crate::fiber::FiberKey;
use crate::runtime::Runtime;

/// 一个 effect 的逆。
///
/// 对应论文 3.1 节：可撤销 effect 把动作与逆配对，而逆必须能被捕获为一个**值**。
/// `FnOnce` 是这里的关键选择——它使「恢复至多触发一次」成为类型系统保证，
/// 而不是 TypeScript 实现里那个 `armed` 布尔标志。
pub type Inverse = Box<dyn FnOnce() -> LocalBoxFuture<'static, ()>>;

/// 一次 [`Context::effect`](crate::Context::effect) 调用所登记的那组逆。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GroupId(pub(crate) u64);

pub(crate) struct Disposer {
    pub group: GroupId,
    pub inverse: Inverse,
}

/// 逆的累积器。
///
/// 对应论文算法 1 的 $f \circ g$：每个新逆被**前置**，因此恢复是 LIFO。
/// 这里用一个 `Vec` 加反向取出实现，避免闭包层层嵌套。
#[derive(Default)]
pub(crate) struct DisposerList {
    items: Vec<Disposer>,
}

impl DisposerList {
    pub fn push(&mut self, group: GroupId, inverse: Inverse) {
        self.items.push(Disposer { group, inverse });
    }

    /// 按 LIFO 取出全部逆。
    pub fn take_all(&mut self) -> Vec<Disposer> {
        let mut items = std::mem::take(&mut self.items);
        items.reverse();
        items
    }

    /// 按 LIFO 取出某一组的逆，其余留在原处。
    pub fn take_group(&mut self, group: GroupId) -> Vec<Disposer> {
        let mut taken = Vec::new();
        let mut kept = Vec::new();
        for item in std::mem::take(&mut self.items) {
            if item.group == group {
                taken.push(item);
            } else {
                kept.push(item);
            }
        }
        self.items = kept;
        taken.reverse();
        taken
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

pub(crate) async fn run_inverses(items: Vec<Disposer>) {
    for item in items {
        (item.inverse)().await;
    }
}

pub(crate) type Guard = Rc<dyn Fn() -> bool>;

/// 一个正在执行中的 effect 的登记面。
///
/// 组件每完成一小步，就用 [`Steps::step`] 登记这一步的逆。这是论文
/// $\mathfrak{E}^{\mathrm{iter}}_\Gamma$（定义 51）的可用编码：Rust 的
/// `gen` / `async gen` 块至今仍未稳定，因此这里用「往登记面推」代替「yield 出逆」，
/// 语义等价——每次 `step` 就是一个步骤边界。
///
/// `Steps` 内部只有 `Rc` 与 `Copy` 字段，因此按值传递即可，不需要 `&mut`。
#[derive(Clone)]
pub struct Steps {
    rt: Rc<Runtime>,
    fiber: FiberKey,
    group: GroupId,
    guard: Guard,
}

impl Steps {
    pub(crate) fn new(rt: Rc<Runtime>, fiber: FiberKey, group: GroupId, guard: Guard) -> Self {
        Steps {
            rt,
            fiber,
            group,
            guard,
        }
    }

    /// 登记刚刚完成的那一步的逆，并检查这次转换是否还有效。
    ///
    /// 返回 [`Error::Aborted`] 表示守卫已失效（target 变了、或该 effect 已被撤销），
    /// 组件应当直接 `?` 传播出去。此时**这一步的逆已经登记**，所以运行时会把
    /// 它连同之前各步一起回卷。
    ///
    /// 这比论文算法 1 的粒度更保守一点：算法在执行下一步**之前**查守卫，
    /// 而这里在登记之后查，因此被中断的那一步同样会被回滚。方向一致，
    /// 只是永不遗漏。
    pub fn step<F, Fut>(&self, undo: F) -> Result<()>
    where
        F: FnOnce() -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        self.push(Box::new(move || Box::pin(undo())));
        self.check()
    }

    /// [`Steps::step`] 的同步版本，用于不需要 await 的清理。
    pub fn step_sync<F>(&self, undo: F) -> Result<()>
    where
        F: FnOnce() + 'static,
    {
        self.push(Box::new(move || {
            undo();
            Box::pin(std::future::ready(()))
        }));
        self.check()
    }

    /// 这次转换是否已经过期。
    pub fn is_stale(&self) -> bool {
        !(self.guard)()
    }

    fn check(&self) -> Result<()> {
        if self.is_stale() {
            Err(Error::Aborted)
        } else {
            Ok(())
        }
    }

    fn push(&self, inverse: Inverse) {
        self.rt.push_disposer(self.fiber, self.group, inverse);
    }
}

/// 撤销某一次 effect 的句柄。
///
/// 对应论文算法 1 第 9-16 行返回的 `dispose`。`revert` 取走那一组逆再运行，
/// 因此第二次调用是无操作——「至多触发一次」由所有权保证。
pub struct EffectHandle {
    pub(crate) rt: Rc<Runtime>,
    pub(crate) fiber: FiberKey,
    pub(crate) group: GroupId,
}

impl EffectHandle {
    pub async fn revert(&self) {
        let items = self.rt.take_group(self.fiber, self.group);
        run_inverses(items).await;
    }
}
