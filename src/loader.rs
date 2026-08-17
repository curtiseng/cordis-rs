use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::context::{Context, FiberHandle};
use crate::entry::Entry;
use crate::error::{Error, Result};
use crate::fiber::State;
use crate::registry::Registry;

struct Mounted {
    entry: Entry,
    handle: Rc<FiberHandle>,
}

/// 一次对账的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// 新装上的行。
    pub created: Vec<String>,
    /// 拆掉且没有重装的行。
    pub removed: Vec<String>,
    /// 拆掉又重装的行（`name` 或 `config` 变了，或者从关到开）。
    pub updated: Vec<String>,
    /// 这次调用被合并进了正在进行的那一轮，它自己什么都没做。
    pub coalesced: bool,
    /// 这次调用实际跑了几轮对账。大于 1 说明期间有别的调用被合并进来了。
    pub passes: usize,
}

impl Applied {
    fn coalesced() -> Self {
        Applied {
            coalesced: true,
            ..Default::default()
        }
    }

    /// 这一轮什么都没改。
    pub fn is_noop(&self) -> bool {
        self.created.is_empty() && self.removed.is_empty() && self.updated.is_empty()
    }
}

/// 一份对账计划。只读地算出来，因此算的过程中不会动任何 fiber。
struct Plan {
    retire: Vec<String>,
    create: Vec<Entry>,
}

impl Plan {
    fn compute(mounted: &[Mounted], desired: &[Entry]) -> Plan {
        let mut retire = Vec::new();
        for current in mounted {
            let keep = desired.iter().any(|entry| {
                entry.id == current.entry.id
                    && !entry.disabled
                    && entry.name == current.entry.name
                    && entry.config == current.entry.config
            });
            if !keep {
                retire.push(current.entry.id.clone());
            }
        }
        let create = desired
            .iter()
            .filter(|entry| !entry.disabled)
            // 已经装着、又没被判为拆除的行，就是「保持不动」。
            .filter(|entry| {
                let already = mounted.iter().any(|current| current.entry.id == entry.id);
                !already || retire.contains(&entry.id)
            })
            .cloned()
            .collect();
        Plan { retire, create }
    }
}

/// 声明式配置层。
///
/// 对应论文 5.2.1 节：把一棵配置树对账成一组活着的 fiber，并在配置变化时把差异
/// 增量地施加上去。它自己不含任何新机制——每一次变更都归约成
/// [`Context::use_component`] 与 [`FiberHandle::dispose`]，也就是说**配置热重载
/// 完全是可撤销 effect 的一个应用**，而不是它之外的另一套东西。
///
/// # 三条被刻意保留的性质
///
/// **先构造，再拆除。** 不合法的配置、注册表里没有的名字，都在任何 fiber 被动过
/// 之前同步地失败。所以一次写坏的编辑不会先把系统拆一半。
///
/// **补偿事务，而不是不可见的原子替换。** 任意组件的 effect 无法快照，所以这里
/// 不承诺中间状态不可见：候选失败时拆掉候选、重建先前的行，并如实报告
/// [`Error::Rollback`] 而不是声称树被完好保留。
///
/// **串行且合并。** 同一个 loader 上的并发 `apply` 不会交错——第二个调用只是
/// 更新「期望状态」然后立刻返回（`coalesced`），由正在跑的那一轮拾取。对配置
/// 来说合并是正确的：只有最新那份期望状态有意义。
///
/// 最后一条不是吞吐取舍而是正确性要求：对账过程中会 await，而两轮交错的对账会在
/// 同一行上交叉执行 create 与回滚。
pub struct Loader {
    ctx: Context,
    registry: Registry,
    mounted: RefCell<Vec<Mounted>>,
    desired: RefCell<Option<Vec<Entry>>>,
    running: Cell<bool>,
}

impl Loader {
    pub fn new(ctx: Context, registry: Registry) -> Loader {
        Loader {
            ctx,
            registry,
            mounted: RefCell::new(Vec::new()),
            desired: RefCell::new(None),
            running: Cell::new(false),
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// 把配置树对账到期望状态。
    ///
    /// 若已有一轮对账在飞，这次调用只登记期望状态并立刻返回
    /// `Applied { coalesced: true, .. }`；正在跑的那一轮会拾取它。
    pub async fn apply(&self, entries: Vec<Entry>) -> Result<Applied> {
        *self.desired.borrow_mut() = Some(entries);
        if self.running.get() {
            return Ok(Applied::coalesced());
        }

        self.running.set(true);
        let mut passes = 0usize;
        let mut outcome = Ok(Applied::default());
        loop {
            let next = self.desired.borrow_mut().take();
            let Some(next) = next else { break };
            passes += 1;
            outcome = self.reconcile(next).await;
        }
        self.running.set(false);

        outcome.map(|mut applied| {
            applied.passes = passes;
            applied
        })
    }

    async fn reconcile(&self, desired: Vec<Entry>) -> Result<Applied> {
        crate::entry::check_unique(&desired)?;

        // 第一步：只读地算计划，并把所有新组件构造出来。名字不认识、配置不合法
        // 都在这里失败，此刻还没有任何 fiber 被动过。
        let plan = {
            let mounted = self.mounted.borrow();
            Plan::compute(&mounted, &desired)
        };
        let mut built = Vec::with_capacity(plan.create.len());
        for entry in &plan.create {
            let component = self.registry.build(&entry.name, &entry.config)?;
            built.push((entry.clone(), component));
        }

        // 第二步：按 LIFO 拆除。
        //
        // 必须先拆后建，不能反过来：同一个键的新旧提供者若同时在场，旧提供者的
        // 逆会把新提供者刚写进存储的那条绑定删掉。
        let mut retired: Vec<Entry> = Vec::new();
        for id in plan.retire.iter().rev() {
            if let Some(mounted) = self.detach(id) {
                mounted.handle.dispose().await;
                retired.push(mounted.entry);
            }
        }

        // 第三步：装上新的。
        let mut created: Vec<String> = Vec::new();
        for (entry, component) in built {
            let handle = Rc::new(self.ctx.use_component(component));
            created.push(entry.id.clone());
            self.mounted.borrow_mut().push(Mounted { entry, handle });
        }

        // 第四步：等系统静止，再看有没有行结算为失败。
        //
        // 必须等**整个系统**而不只是这一轮新建的行：装上一个提供者会让它的依赖方
        // 各自发起转换，而那些行属于先前的配置。检查也覆盖全部行，因为「换掉一个
        // 提供者导致它的消费者失败」同样是这次变更的后果。
        //
        // 「还在等某个依赖」不算失败——那是一条有效的 pending 配置项，正是空间
        // 可组合性想要的行为（论文说依赖不可用只是保持非活动，不是错误）。
        self.ctx.quiesce().await;
        let failures = self.failures();
        if !failures.is_empty() {
            return Err(self.roll_back(created, retired, failures).await);
        }

        // 第五步：让内部顺序与配置一致，后续的 LIFO 拆除才是按配置逆序。
        self.sort_to(&desired);

        // 同一行既被拆又被建，就是一次「更新」而不是一去一来。
        let updated: Vec<String> = created
            .iter()
            .filter(|id| retired.iter().any(|entry| &entry.id == *id))
            .cloned()
            .collect();
        Ok(Applied {
            created: created
                .into_iter()
                .filter(|id| !updated.contains(id))
                .collect(),
            removed: retired
                .into_iter()
                .map(|entry| entry.id)
                .filter(|id| !updated.contains(id))
                .collect(),
            updated,
            coalesced: false,
            passes: 0,
        })
    }

    /// 已结算为失败的行。系统静止之后才有意义。
    fn failures(&self) -> Vec<String> {
        self.mounted
            .borrow()
            .iter()
            .filter(|item| item.handle.state() == State::Failed)
            .map(|item| {
                let reason = item
                    .handle
                    .error()
                    .unwrap_or_else(|| "未给出原因".to_owned());
                format!("{}：{reason}", item.entry.id)
            })
            .collect()
    }

    /// 补偿：拆掉这一轮建起来的，把这一轮拆掉的重新装回去。
    ///
    /// 恢复出来的是**新的** fiber（新的代际索引），不是原来那个。这是诚实的：
    /// 论文的可撤销性保证逆会被运行，不保证时间被倒流回去。
    async fn roll_back(
        &self,
        created: Vec<String>,
        retired: Vec<Entry>,
        failures: Vec<String>,
    ) -> Error {
        let mut trouble = Vec::new();

        for id in created.iter().rev() {
            if let Some(mounted) = self.detach(id) {
                mounted.handle.dispose().await;
            }
        }

        // `retired` 是按拆除顺序（配置逆序）攒的，所以反过来装回去。
        for entry in retired.into_iter().rev() {
            match self.registry.build(&entry.name, &entry.config) {
                Ok(component) => {
                    let handle = Rc::new(self.ctx.use_component(component));
                    self.mounted.borrow_mut().push(Mounted { entry, handle });
                }
                Err(error) => trouble.push(format!("{} 无法重建：{error}", entry.id)),
            }
        }
        self.ctx.quiesce().await;
        for message in self.failures() {
            trouble.push(format!("重建后仍失败：{message}"));
        }

        if trouble.is_empty() {
            Error::Component(format!("配置项加载失败，已回滚：{}", failures.join("；")))
        } else {
            trouble.insert(0, format!("触发回滚的失败：{}", failures.join("；")));
            Error::Rollback(trouble)
        }
    }

    /* ---------------------------------------------------------------- */
    /* 内部账本                                                          */
    /* ---------------------------------------------------------------- */

    fn detach(&self, id: &str) -> Option<Mounted> {
        let mut mounted = self.mounted.borrow_mut();
        let index = mounted.iter().position(|item| item.entry.id == id)?;
        Some(mounted.remove(index))
    }

    fn handle_of(&self, id: &str) -> Option<Rc<FiberHandle>> {
        self.mounted
            .borrow()
            .iter()
            .find(|item| item.entry.id == id)
            .map(|item| item.handle.clone())
    }

    fn sort_to(&self, desired: &[Entry]) {
        let order = |id: &str| desired.iter().position(|entry| entry.id == id);
        self.mounted
            .borrow_mut()
            .sort_by_key(|item| order(&item.entry.id).unwrap_or(usize::MAX));
    }

    /* ---------------------------------------------------------------- */
    /* 观测                                                              */
    /* ---------------------------------------------------------------- */

    /// 当前装上的行的 id，按配置顺序。
    pub fn ids(&self) -> Vec<String> {
        self.mounted
            .borrow()
            .iter()
            .map(|item| item.entry.id.clone())
            .collect()
    }

    /// 某一行当前的生命周期状态。没装上的行返回 `None`。
    pub fn state(&self, id: &str) -> Option<State> {
        self.handle_of(id).map(|handle| handle.state())
    }

    /// 某一行记在 fiber 上的错误（L-Raise）。
    pub fn error(&self, id: &str) -> Option<String> {
        self.handle_of(id).and_then(|handle| handle.error())
    }

    /// 某一行当前生效的配置。
    pub fn entry(&self, id: &str) -> Option<Entry> {
        self.mounted
            .borrow()
            .iter()
            .find(|item| item.entry.id == id)
            .map(|item| item.entry.clone())
    }

    /// 某一行的上下文，用于在它之下再实例化组件。
    pub fn context_of(&self, id: &str) -> Option<Context> {
        self.handle_of(id).map(|handle| handle.context())
    }

    /// 按 LIFO 拆掉全部行。
    pub async fn unload_all(&self) {
        let ids = self.ids();
        for id in ids.iter().rev() {
            if let Some(mounted) = self.detach(id) {
                mounted.handle.dispose().await;
            }
        }
    }
}
