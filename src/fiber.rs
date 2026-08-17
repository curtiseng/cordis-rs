use std::collections::HashMap;
use std::rc::Rc;

use futures::future::{LocalBoxFuture, Shared};

use crate::component::Component;
use crate::effect::{DisposerList, GroupId};
use crate::key::{KeyId, RealmId};

slotmap::new_key_type! {
    /// fiber 的标识。
    ///
    /// 对应论文的 `fiber.uid`（$n : \mathfrak{N}$）。用代际索引而不是自增整数，
    /// 是因为论文要求 uid「新鲜取得且永不复用」——slotmap 的代际正是这个语义，
    /// 于是「被替换的提供者不会与它所替换的那个被混同」由类型系统兜住。
    pub struct FiberKey;
}

/// fiber 的生命周期状态。
///
/// 对应论文定义 44 的 $\theta$。`Loading` 即 $\mathsf{Reloading}$，
/// `Failed` 即带错误的 $\mathsf{Inactive}(\xi)$。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Inactive,
    Loading,
    Active,
    Unloading,
    Failed,
}

impl State {
    /// 是否已停止提供服务。
    ///
    /// `Unloading` 也算——这正是论文 5.1.3 节所说「撤回提前一步对依赖方可见」：
    /// 进入 `UNLOADING` 的提供者已停止提供，尽管它的绑定仍在位。
    pub fn is_settled(self) -> bool {
        matches!(self, State::Inactive | State::Failed)
    }
}

/// 已提交视图里的一条绑定：由谁提供、解析到哪个 realm。
pub(crate) type CommittedEntry = (FiberKey, RealmId);

pub(crate) struct Fiber {
    /// 诊断用的名字。`Rc<str>` 而不是 `&'static str`：动态基质（wasm 文件、模型
    /// 现写的一段代码）的名字只有运行时才知道。
    pub name: Rc<str>,
    pub parent: Option<FiberKey>,
    /// 组件的 coeffect 规格 $d$。
    pub inject: Vec<KeyId>,
    /// 该 fiber 上下文继承到的 realm 表 $\rho$。
    pub realms: Rc<HashMap<KeyId, RealmId>>,
    /// 已套入配置的 effect 函数 $e$；根 fiber 没有。
    pub component: Option<Rc<dyn Component>>,
    pub state: State,
    /// `target(γ, n)`（定义 46）的摘要：各被声明键当前的提供者。`None` 即 ⊥。
    pub target: Option<Vec<FiberKey>>,
    /// 已提交视图 $\omega$（定义 44）。
    pub committed: HashMap<KeyId, CommittedEntry>,
    /// 由该 fiber 安装的那些绑定。
    pub provided: Vec<(KeyId, RealmId)>,
    /// 累积器 $g$：待运行的逆。
    pub disposers: DisposerList,
    /// 飞行中转换的句柄（$\mathsf{Future}$，4.3.3 节的惯性）。
    ///
    /// 用 `Shared` 而不是 `JoinHandle`，因为算法 5 第 25 行要求**多个**依赖方
    /// 同时等待同一次转换，而 Rust 的 future 默认是单消费者。
    pub inertia: Option<Shared<LocalBoxFuture<'static, ()>>>,
    pub error: Option<String>,
    /// 该 fiber 在其父级 disposer 列表里的分组，用于 `FiberHandle::dispose`。
    pub group: Option<GroupId>,
    /// 这次实例化是否已被撤回（**O-Retire**）。
    ///
    /// 撤回之后 `target(γ, n)` 恒为 ⊥——论文里这一点由「fiber 不再属于
    /// $\mathrm{dom}(F_\gamma)$」自动成立，而这里 fiber 要等到卸载完成才能
    /// 从竞技场里移除（**O-Remove**），所以需要显式记一笔。
    pub retired: bool,
}

impl Fiber {
    pub fn realm_of(&self, key: KeyId) -> RealmId {
        self.realms.get(&key).copied().unwrap_or(RealmId::ROOT)
    }
}
