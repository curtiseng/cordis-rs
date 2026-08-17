use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use futures::executor::{LocalPool, LocalSpawner};
use futures::future::{FutureExt, LocalBoxFuture};
use futures::task::LocalSpawnExt;
use slotmap::SlotMap;

use crate::component::Component;
use crate::context::Context;
use crate::effect::{Disposer, GroupId, Inverse, Steps, run_inverses};
use crate::error::Error;
use crate::fiber::{Fiber, FiberKey, State};
use crate::key::{KeyId, RealmId};

pub(crate) struct Binding {
    pub provider: FiberKey,
    /// 装的是 `Rc<K::Api>`。`Rc<dyn Trait>` 自身是 `Sized + 'static`，
    /// 所以可以装进 `dyn Any` 再取回来——这是 TypeScript 那个异质 store
    /// （$\sigma : (r : R) \rightharpoonup \mathcal{V}_r$）在 Rust 里的对应物。
    pub value: Box<dyn Any>,
}

/// 运行中系统的全部状态。
///
/// fiber 存在一个竞技场里、互相只存 key，而不是 `Rc<RefCell<Fiber>>`：
/// 一是父子双向引用会构成 `Rc` 环，二是算法 3 的 `notify` 要在遍历 fiber 的
/// 同时改它们，共享可变借用必然在运行时炸掉。竞技场把这两个问题一起消掉。
pub struct Runtime {
    fibers: RefCell<SlotMap<FiberKey, Fiber>>,
    /// 值存储 $\sigma$，按 (键, realm) 索引——即两层解析的第二层。
    store: RefCell<HashMap<(KeyId, RealmId), Binding>>,
    spawner: LocalSpawner,
    counter: Cell<u64>,
    root: FiberKey,
}

impl Runtime {
    pub(crate) fn new(spawner: LocalSpawner) -> Rc<Self> {
        let mut fibers = SlotMap::with_key();
        let root = fibers.insert(Fiber {
            name: "root",
            parent: None,
            inject: Vec::new(),
            realms: Rc::new(HashMap::new()),
            component: None,
            // 根 fiber 生来就是活动的，因此在它上面装的绑定立即可用。
            state: State::Active,
            target: Some(Vec::new()),
            committed: HashMap::new(),
            provided: Vec::new(),
            disposers: Default::default(),
            inertia: None,
            error: None,
            group: None,
            retired: false,
        });
        Rc::new(Runtime {
            fibers: RefCell::new(fibers),
            store: RefCell::new(HashMap::new()),
            spawner,
            counter: Cell::new(1),
            root,
        })
    }

    pub(crate) fn root(&self) -> FiberKey {
        self.root
    }

    fn next_id(&self) -> u64 {
        let id = self.counter.get();
        self.counter.set(id + 1);
        id
    }

    pub(crate) fn new_group(&self) -> GroupId {
        GroupId(self.next_id())
    }

    pub(crate) fn new_realm(&self) -> RealmId {
        RealmId(self.next_id())
    }

    /* ---------------------------------------------------------------- */
    /* fiber 与存储的短借用访问                                          */
    /* ---------------------------------------------------------------- */

    pub(crate) fn with_fiber<T>(&self, key: FiberKey, f: impl FnOnce(&Fiber) -> T) -> Option<T> {
        self.fibers.borrow().get(key).map(f)
    }

    pub(crate) fn with_fiber_mut<T>(
        &self,
        key: FiberKey,
        f: impl FnOnce(&mut Fiber) -> T,
    ) -> Option<T> {
        self.fibers.borrow_mut().get_mut(key).map(f)
    }

    pub(crate) fn state_of(&self, key: FiberKey) -> State {
        self.with_fiber(key, |f| f.state).unwrap_or(State::Inactive)
    }

    pub(crate) fn error_of(&self, key: FiberKey) -> Option<String> {
        self.with_fiber(key, |f| f.error.clone()).flatten()
    }

    pub(crate) fn name_of(&self, key: FiberKey) -> &'static str {
        self.with_fiber(key, |f| f.name).unwrap_or("<已移除>")
    }

    pub(crate) fn tracked_effects(&self, key: FiberKey) -> usize {
        self.with_fiber(key, |f| f.disposers.len()).unwrap_or(0)
    }

    fn target_of(&self, key: FiberKey) -> Option<Vec<FiberKey>> {
        self.with_fiber(key, |f| f.target.clone()).flatten()
    }

    pub(crate) fn push_disposer(&self, fiber: FiberKey, group: GroupId, inverse: Inverse) {
        self.with_fiber_mut(fiber, |f| f.disposers.push(group, inverse));
    }

    pub(crate) fn take_group(&self, fiber: FiberKey, group: GroupId) -> Vec<Disposer> {
        self.with_fiber_mut(fiber, |f| f.disposers.take_group(group))
            .unwrap_or_default()
    }

    pub(crate) fn store_insert(&self, at: (KeyId, RealmId), binding: Binding) {
        self.store.borrow_mut().insert(at, binding);
    }

    pub(crate) fn store_remove(&self, at: &(KeyId, RealmId)) {
        self.store.borrow_mut().remove(at);
    }

    /// 从存储里取出某条绑定的值。对应论文的 `ctx.get`：查询存储，永不失败。
    pub(crate) fn store_get<T: Clone + 'static>(&self, at: &(KeyId, RealmId)) -> Option<T> {
        self.store
            .borrow()
            .get(at)
            .and_then(|b| b.value.downcast_ref::<T>().cloned())
    }

    /* ---------------------------------------------------------------- */
    /* 算法 6：沿 fiber 链解析已提交视图                                  */
    /* ---------------------------------------------------------------- */

    /// 从发起访问的 fiber 出发向上行走，在第一个已提交该键的 fiber 处授权。
    ///
    /// 与直接查存储的区别正是论文 5.1.4 节强调的那点：这里读的是**视图**，
    /// 所以「其拆解正是由某项依赖的离去所触发的组件」仍能读到那项依赖。
    pub(crate) fn resolve_declared<T: Clone + 'static>(
        &self,
        from: FiberKey,
        kid: KeyId,
        name: &'static str,
    ) -> crate::error::Result<T> {
        let fibers = self.fibers.borrow();
        let store = self.store.borrow();
        let mut cursor = Some(from);
        while let Some(key) = cursor {
            let Some(fiber) = fibers.get(key) else {
                break;
            };
            if let Some((_provider, realm)) = fiber.committed.get(&kid) {
                return store
                    .get(&(kid, *realm))
                    .and_then(|binding| binding.value.downcast_ref::<T>().cloned())
                    .ok_or(Error::Inactive(name));
            }
            if fiber.inject.contains(&kid) {
                return Err(Error::Inactive(name));
            }
            cursor = fiber.parent;
        }
        Err(Error::Undeclared(name))
    }

    /* ---------------------------------------------------------------- */
    /* 目标视图（定义 46）                                               */
    /* ---------------------------------------------------------------- */

    /// 把每个被声明的键对照**活动的提供者**解析。
    ///
    /// 注意判据是提供者 fiber 处于 `ACTIVE`，而不是「存储里有这条绑定」。
    /// 这正是论文 5.1.3 节所说的、使撤回提前一步可见的机制。
    fn resolve_view(&self, key: FiberKey) -> Option<HashMap<KeyId, (FiberKey, RealmId)>> {
        let fibers = self.fibers.borrow();
        let store = self.store.borrow();
        let fiber = fibers.get(key)?;
        if fiber.retired {
            return None;
        }
        let mut view = HashMap::new();
        for kid in &fiber.inject {
            let realm = fiber.realm_of(*kid);
            let binding = store.get(&(*kid, realm))?;
            let provider_active = fibers
                .get(binding.provider)
                .is_some_and(|p| p.state == State::Active);
            if !provider_active {
                return None;
            }
            view.insert(*kid, (binding.provider, realm));
        }
        Some(view)
    }

    /// target 是「各被声明键的提供者」的摘要。
    ///
    /// 用提供者的 `uid` 而不是值来标识绑定，因此单次比较就够——代际索引保证
    /// 被替换的提供者不会与替换者相等，即使二者提供相等的值。
    fn target_from(view: &HashMap<KeyId, (FiberKey, RealmId)>, inject: &[KeyId]) -> Vec<FiberKey> {
        inject
            .iter()
            .filter_map(|kid| view.get(kid).map(|(provider, _)| *provider))
            .collect()
    }

    /* ---------------------------------------------------------------- */
    /* 算法 3：响应式通知                                                */
    /* ---------------------------------------------------------------- */

    /// 把绑定变化传播给依赖方，返回其中 target 真的变了的那些。
    ///
    /// 分两阶段：先只读地收集候选，放掉借用，再逐个 `refresh`。在 Rust 里
    /// 这不是风格问题而是必需——`refresh` 会改 fiber 状态并 spawn 任务。
    pub(crate) fn notify(rt: &Rc<Runtime>, changed: &[(KeyId, RealmId)]) -> Vec<FiberKey> {
        let candidates: Vec<FiberKey> = {
            let fibers = rt.fibers.borrow();
            fibers
                .iter()
                .filter(|(_, fiber)| {
                    changed.iter().any(|(kid, realm)| {
                        fiber.inject.contains(kid) && fiber.realm_of(*kid) == *realm
                    })
                })
                .map(|(key, _)| key)
                .collect()
        };

        let mut affected = Vec::new();
        for key in candidates {
            let before = rt.target_of(key);
            Self::refresh(rt, key);
            if rt.target_of(key) != before {
                affected.push(key);
            }
        }
        affected
    }

    /* ---------------------------------------------------------------- */
    /* 算法 5：组件生命周期                                              */
    /* ---------------------------------------------------------------- */

    /// 重算 target，必要时发起一次转换。
    pub(crate) fn refresh(rt: &Rc<Runtime>, key: FiberKey) {
        let view = rt.resolve_view(key);
        let target = {
            let fibers = rt.fibers.borrow();
            let Some(fiber) = fibers.get(key) else { return };
            view.as_ref()
                .map(|view| Self::target_from(view, &fiber.inject))
        };

        enum Next {
            Nothing,
            Reload,
            Unload,
        }

        let next = {
            let mut fibers = rt.fibers.borrow_mut();
            let Some(fiber) = fibers.get_mut(key) else {
                return;
            };
            if fiber.target == target {
                return;
            }
            fiber.target = target.clone();
            if fiber.inertia.is_some() {
                // 惯性：转换先运行至完成，系统才响应目标变化。
                Next::Nothing
            } else if target.is_some() {
                fiber.state = State::Loading;
                Next::Reload
            } else {
                // L-Leave：在任何逆被调度**之前**就退出服务。
                fiber.state = State::Unloading;
                Next::Unload
            }
        };

        match next {
            Next::Nothing => {}
            Next::Reload => Self::spawn(rt, key, Transition::Reload),
            Next::Unload => Self::spawn(rt, key, Transition::Unload),
        }
    }

    fn spawn(rt: &Rc<Runtime>, key: FiberKey, transition: Transition) {
        let future: LocalBoxFuture<'static, ()> = match transition {
            Transition::Reload => Box::pin(Self::reload(rt.clone(), key)),
            Transition::Unload => Box::pin(Self::unload(rt.clone(), key)),
        };
        let shared = future.shared();
        rt.with_fiber_mut(key, |fiber| fiber.inertia = Some(shared.clone()));
        rt.spawner
            .spawn_local(shared.map(|_| ()))
            .expect("执行器已关闭");
    }

    async fn reload(rt: Rc<Runtime>, key: FiberKey) {
        let Some(target0) = rt.target_of(key) else {
            return;
        };

        // 提交视图（算法 5 第 14 行）：此后整段加载期间——包括它自己的拆解——
        // 该 fiber 都从这份视图读依赖。
        let view = rt.resolve_view(key).unwrap_or_default();
        let installed = rt.with_fiber_mut(key, |fiber| {
            fiber.committed = view;
            fiber.error = None;
            fiber.component.clone()
        });
        let component = match installed {
            Some(Some(component)) => component,
            // 根 fiber 没有组件：没有 effect 要施加，直接就是活动的。
            Some(None) => {
                rt.with_fiber_mut(key, |fiber| {
                    fiber.state = State::Active;
                    fiber.inertia = None;
                });
                return;
            }
            None => return,
        };
        let ctx = Context::for_fiber(rt.clone(), key);

        let guard: crate::effect::Guard = {
            let rt = rt.clone();
            let target0 = target0.clone();
            Rc::new(move || rt.target_of(key).as_ref() == Some(&target0))
        };
        let group = rt.new_group();
        let steps = Steps::new(rt.clone(), key, group, guard);

        let result = component.apply(ctx, steps).await;

        let still_current = rt.target_of(key).as_ref() == Some(&target0);
        let failed = match &result {
            Ok(()) => false,
            Err(Error::Aborted) => false,
            Err(error) => {
                let message = error.to_string();
                rt.with_fiber_mut(key, |fiber| {
                    // L-Raise：错误记在 fiber 上，target 置 ⊥。
                    fiber.error = Some(message);
                    fiber.target = None;
                });
                true
            }
        };

        if still_current && !failed {
            let provided = rt
                .with_fiber_mut(key, |fiber| {
                    fiber.state = State::Active;
                    fiber.inertia = None;
                    fiber.provided.clone()
                })
                .unwrap_or_default();
            // 现在才对依赖方可用（算法 5 第 19 行）。
            Self::notify(&rt, &provided);
        } else {
            rt.with_fiber_mut(key, |fiber| fiber.state = State::Unloading);
            Self::spawn(&rt, key, Transition::Unload);
        }
    }

    async fn unload(rt: Rc<Runtime>, key: FiberKey) {
        // 第 25 行：先让依赖方排空，此时本 fiber 的绑定仍全部在位。
        let provided = rt
            .with_fiber(key, |fiber| fiber.provided.clone())
            .unwrap_or_default();
        for dependent in Self::notify(&rt, &provided) {
            Self::settle(rt.clone(), dependent).await;
        }

        // 第 26-28 行：按 LIFO 回卷，然后丢弃已提交视图。
        let items = rt
            .with_fiber_mut(key, |fiber| fiber.disposers.take_all())
            .unwrap_or_default();
        run_inverses(items).await;
        rt.with_fiber_mut(key, |fiber| {
            fiber.committed.clear();
            fiber.provided.clear();
        });

        let failed = rt.error_of(key).is_some();
        if rt.target_of(key).is_none() {
            let retired = rt
                .with_fiber_mut(key, |fiber| {
                    fiber.state = if failed {
                        State::Failed
                    } else {
                        State::Inactive
                    };
                    fiber.inertia = None;
                    fiber.retired
                })
                .unwrap_or(false);
            if retired {
                // O-Remove：uid 被清除，代际索引保证它永不被复用。
                rt.fibers.borrow_mut().remove(key);
            }
        } else {
            rt.with_fiber_mut(key, |fiber| fiber.state = State::Loading);
            Self::spawn(&rt, key, Transition::Reload);
        }
    }

    /// 等到系统里不再有任何飞行中的转换。
    ///
    /// [`App::settle`] 的可 await 版本。异步代码里需要它是因为一次变更会级联：
    /// 装上一个提供者会让它的依赖方**各自**发起转换，而那些 fiber 事先不可枚举。
    /// 只等自己碰过的那几个 fiber 是不够的。
    ///
    /// 每轮扫描只取一个飞行中的转换来 await，因此不依赖执行器去驱动其它任务；
    /// 循环能终止由论文定理 66（终止性）保证。
    pub(crate) async fn quiesce(rt: Rc<Runtime>) {
        loop {
            let inflight = {
                let fibers = rt.fibers.borrow();
                fibers.iter().find_map(|(_, fiber)| fiber.inertia.clone())
            };
            match inflight {
                Some(inertia) => inertia.await,
                None => break,
            }
        }
    }

    /// 等到某个 fiber 不再有飞行中的转换。
    ///
    /// 循环是必需的：一次转换可以在收尾时**链接**进下一次（惯性链接），
    /// 那时 `inertia` 已经被换成新任务的句柄。
    pub(crate) async fn settle(rt: Rc<Runtime>, key: FiberKey) {
        loop {
            let inertia = rt.with_fiber(key, |fiber| fiber.inertia.clone()).flatten();
            match inertia {
                Some(inertia) => inertia.await,
                None => break,
            }
        }
    }

    /* ---------------------------------------------------------------- */
    /* 算法 4：组件实例化                                                */
    /* ---------------------------------------------------------------- */

    pub(crate) fn insert_fiber(
        rt: &Rc<Runtime>,
        parent: FiberKey,
        component: Rc<dyn Component>,
        realms: Rc<HashMap<KeyId, RealmId>>,
    ) -> FiberKey {
        let fiber = Fiber {
            name: component.name(),
            parent: Some(parent),
            inject: component.inject(),
            realms,
            component: Some(component),
            state: State::Inactive,
            target: None,
            committed: HashMap::new(),
            provided: Vec::new(),
            disposers: Default::default(),
            inertia: None,
            error: None,
            group: None,
            retired: false,
        };
        rt.fibers.borrow_mut().insert(fiber)
    }
}

enum Transition {
    Reload,
    Unload,
}

/// 一个 Cordis 应用：一个执行器加上它的运行时。
///
/// 论文注 2 特意点了 Rust：future 是惰性的，宿主必须自行 spawn 任务才能让
/// 转换推进。所以 `create_task` 在这里是显式的，而 `App` 就是那个宿主。
pub struct App {
    pool: LocalPool,
    rt: Rc<Runtime>,
}

impl App {
    pub fn new() -> Self {
        let pool = LocalPool::new();
        let rt = Runtime::new(pool.spawner());
        App { pool, rt }
    }

    /// 根上下文。
    pub fn root(&self) -> Context {
        Context::for_fiber(self.rt.clone(), self.rt.root())
    }

    /// 把所有飞行中的转换推进到静止。
    ///
    /// 「静止」正是论文合流性（定理 68）所讨论的那个状态。
    pub fn settle(&mut self) {
        self.pool.run_until_stalled();
    }

    pub fn block_on<F: std::future::Future>(&mut self, future: F) -> F::Output {
        self.pool.run_until(future)
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
