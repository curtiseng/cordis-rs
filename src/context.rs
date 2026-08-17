use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;

use crate::component::Component;
use crate::effect::{EffectHandle, Steps, run_inverses};
use crate::error::Result;
use crate::fiber::{FiberKey, State};
use crate::key::{Key, KeyId, RealmId};
use crate::runtime::{Binding, Runtime};

/// 一等的上下文。
///
/// 对应论文 3.3.1 节的 $\Gamma_\infty$：它同时是 effect 的施加面、coeffect 的
/// 解析面，以及组件层级里的一个位置。克隆是廉价的（内部只有 `Rc` 与索引）。
#[derive(Clone)]
pub struct Context {
    rt: Rc<Runtime>,
    fiber: FiberKey,
    /// realm 表 $\rho$：被 `isolate` 派生出的子上下文覆盖，父级留在原处。
    realms: Rc<HashMap<KeyId, RealmId>>,
}

impl Context {
    pub(crate) fn for_fiber(rt: Rc<Runtime>, fiber: FiberKey) -> Context {
        let realms = rt
            .with_fiber(fiber, |f| f.realms.clone())
            .unwrap_or_else(|| Rc::new(HashMap::new()));
        Context { rt, fiber, realms }
    }

    fn realm_of(&self, key: KeyId) -> RealmId {
        self.realms.get(&key).copied().unwrap_or(RealmId::ROOT)
    }

    /* ---------------------------------------------------------------- */
    /* 算法 1：effect 追踪                                               */
    /* ---------------------------------------------------------------- */

    /// 施加一个可撤销 effect。
    ///
    /// 这是上下文被改动的**唯一**原语：coeffect 供给与组件实例化都归约到它，
    /// 因此任何经由上下文执行的操作都会在卸载时被自动回卷。
    ///
    /// 与 TypeScript 版的差别：这里的 effect 在调用点被驱动到完成，而不是
    /// 作为任务并发执行。因此「飞行中的 effect 被 dispose 中止」这一支没有实现；
    /// 组件层的守卫与部分回滚是完整的（见 [`Steps::step`]）。
    pub async fn effect<F, Fut>(&self, body: F) -> Result<EffectHandle>
    where
        F: FnOnce(Steps) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let group = self.rt.new_group();
        let armed = Rc::new(std::cell::Cell::new(true));
        let guard = {
            let armed = armed.clone();
            Rc::new(move || armed.get()) as crate::effect::Guard
        };
        let steps = Steps::new(self.rt.clone(), self.fiber, group, guard);
        match body(steps).await {
            Ok(()) => Ok(EffectHandle {
                rt: self.rt.clone(),
                fiber: self.fiber,
                group,
            }),
            Err(error) => {
                armed.set(false);
                let items = self.rt.take_group(self.fiber, group);
                run_inverses(items).await;
                Err(error)
            }
        }
    }

    /* ---------------------------------------------------------------- */
    /* 算法 2：coeffect 供给                                             */
    /* ---------------------------------------------------------------- */

    /// 提供一项 coeffect。
    ///
    /// 因为 $\mathrm{set}(k, v)$ 的类型就是 $\mathfrak{E}_\Sigma$，供给本身
    /// 是一次 effect，于是继承了自动追踪与恢复：安装它的 fiber 卸载时绑定即被移除。
    pub fn set<K: Key>(&self, value: Rc<K::Api>) -> EffectHandle {
        let kid = KeyId::of::<K>();
        let realm = self.realm_of(kid);
        let at = (kid, realm);

        self.rt.store_insert(
            at,
            Binding {
                provider: self.fiber,
                value: Box::new(value),
            },
        );
        self.rt
            .with_fiber_mut(self.fiber, |fiber| fiber.provided.push(at));
        Runtime::notify(&self.rt, &[at]);

        let group = self.rt.new_group();
        let rt = self.rt.clone();
        let fiber = self.fiber;
        self.rt.push_disposer(
            self.fiber,
            group,
            Box::new(move || {
                Box::pin(async move {
                    rt.store_remove(&at);
                    rt.with_fiber_mut(fiber, |f| f.provided.retain(|entry| *entry != at));
                    for dependent in Runtime::notify(&rt, &[at]) {
                        Runtime::settle(rt.clone(), dependent).await;
                    }
                })
            }),
        );

        EffectHandle {
            rt: self.rt.clone(),
            fiber: self.fiber,
            group,
        }
    }

    /// 读取一项**已声明**的 coeffect（算法 6）。
    ///
    /// 未声明就访问是 [`Error::Undeclared`](crate::Error::Undeclared)，
    /// 声明了但当前未提供是 [`Error::Inactive`](crate::Error::Inactive)。
    /// 这构成了论文 6.3 节所说的、基于能力的访问控制：组件只能拿到它声明过的东西。
    pub fn resolve<K: Key>(&self) -> Result<Rc<K::Api>> {
        self.rt
            .resolve_declared::<Rc<K::Api>>(self.fiber, KeyId::of::<K>(), K::NAME)
    }

    /// 直接查存储，不检查声明、永不失败。对应论文的 `ctx.get(key)`。
    pub fn lookup<K: Key>(&self) -> Option<Rc<K::Api>> {
        let kid = KeyId::of::<K>();
        self.rt.store_get::<Rc<K::Api>>(&(kid, self.realm_of(kid)))
    }

    /* ---------------------------------------------------------------- */
    /* 隔离（定义 29）                                                   */
    /* ---------------------------------------------------------------- */

    /// 派生一个子上下文，把某个键重定向到一个新的 realm。
    ///
    /// 恢复是隐式的：丢弃该子上下文即足够，没有显式的逆需要运行。
    pub fn isolate<K: Key>(&self) -> Context {
        self.isolate_in::<K>(self.rt.new_realm())
    }

    /// 把某个键重定向到指定 realm，用于让若干组件共享同一个隔离域。
    pub fn isolate_in<K: Key>(&self, realm: RealmId) -> Context {
        let mut realms = (*self.realms).clone();
        realms.insert(KeyId::of::<K>(), realm);
        Context {
            rt: self.rt.clone(),
            fiber: self.fiber,
            realms: Rc::new(realms),
        }
    }

    /// 为一个键分配新的 realm 符号，便于把多个键隔离到同一域。
    pub fn new_realm(&self) -> RealmId {
        self.rt.new_realm()
    }

    /// 等到整个系统不再有飞行中的转换。
    ///
    /// [`App::settle`](crate::App::settle) 的可 await 版本，供在异步代码里改动
    /// 完上下文后等结果——一次改动会级联到事先无法枚举的那些依赖方。
    pub async fn quiesce(&self) {
        Runtime::quiesce(self.rt.clone()).await;
    }

    /* ---------------------------------------------------------------- */
    /* 算法 4：组件实例化                                                */
    /* ---------------------------------------------------------------- */

    /// 把一个组件实例化为子 fiber。
    ///
    /// 实例化本身是父级上的一个普通被追踪 effect（**O-Insert**），它的逆
    /// （**O-Retire**）把子代的 target 强制为 ⊥ 并卸载它——所以卸载父级会级联到子代。
    pub fn use_component(&self, component: Rc<dyn Component>) -> FiberHandle {
        let child = Runtime::insert_fiber(&self.rt, self.fiber, component, self.realms.clone());
        let group = self.rt.new_group();
        self.rt
            .with_fiber_mut(child, |fiber| fiber.group = Some(group));

        let rt = self.rt.clone();
        self.rt.push_disposer(
            self.fiber,
            group,
            Box::new(move || {
                Box::pin(async move {
                    // 标记撤回即让 target 恒为 ⊥，随后由 refresh 发起卸载；
                    // 若此刻正有转换在飞，惯性会让它先跑完再链接进卸载。
                    rt.with_fiber_mut(child, |fiber| fiber.retired = true);
                    Runtime::refresh(&rt, child);
                    Runtime::settle(rt.clone(), child).await;
                })
            }),
        );

        Runtime::refresh(&self.rt, child);

        FiberHandle {
            rt: self.rt.clone(),
            parent: self.fiber,
            key: child,
            group,
        }
    }
}

/// 指向一个已实例化 fiber 的句柄。
pub struct FiberHandle {
    rt: Rc<Runtime>,
    parent: FiberKey,
    key: FiberKey,
    group: crate::effect::GroupId,
}

impl FiberHandle {
    pub fn state(&self) -> State {
        self.rt.state_of(self.key)
    }

    pub fn name(&self) -> &'static str {
        self.rt.name_of(self.key)
    }

    /// 当前挂在这个 fiber 上、等待回卷的逆有多少个。
    ///
    /// 卸载后应当归零——这是「没有 effect 泄漏」的一个直接观测口。
    pub fn tracked_effects(&self) -> usize {
        self.rt.tracked_effects(self.key)
    }

    /// 组件失败时记在 fiber 上的错误（L-Raise）。
    pub fn error(&self) -> Option<String> {
        self.rt.error_of(self.key)
    }

    /// 该 fiber 自己的上下文，用于在它之下再实例化组件。
    pub fn context(&self) -> Context {
        Context::for_fiber(self.rt.clone(), self.key)
    }

    /// 等到它不再有飞行中的转换。
    pub async fn settle(&self) {
        Runtime::settle(self.rt.clone(), self.key).await;
    }

    /// 撤回这次实例化（**O-Retire**）。
    pub async fn dispose(&self) {
        let items = self.rt.take_group(self.parent, self.group);
        run_inverses(items).await;
    }
}
