# cordis-rs

时空可组合性演算的 Rust 实现。

论文《[A Programming Paradigm for Spatiotemporal Composability](https://github.com/cordiverse/paper)》第 5.1 节给出了一个核心库，把可撤销 effect 与响应式 coeffect 实现成可用的编程抽象；原文的参考实现 [Cordis](https://github.com/cordiverse/cordis) 是 TypeScript 写的。这里是同一套语义的 Rust 版本，**只做核心库这一层**。

一句话概括它解决什么问题：**让组件可以在运行中被装上和拆掉，而拆得干净这件事由抽象保证，不依赖每个组件作者的勤谨程度。**

```rust
// 组件只声明「我需要 storage」，不关心是谁在提供。
impl Component for Worker {
    fn inject(&self) -> Vec<KeyId> { vec![KeyId::of::<StorageKey>()] }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let place = ctx.resolve::<StorageKey>()?.place();
            steps.step(move || async move { leave(place).await })?;   // 登记这一步的逆
            Ok(())
        })
    }
}
```

提供者被换掉时，这个组件自己会去激活、重新激活，且顺序有保证——它不需要写任何卸载路径，也不需要监听任何事件。`cargo run --example swap_provider` 能看到全过程。

## 三个概念

**可撤销 effect。** 每个改动上下文的动作都配一个逆，逆是值（`Inverse`），卸载时按 LIFO 回放。这是上下文被改动的唯一原语：coeffect 供给与组件实例化都归约到它，所以经由上下文做的任何事都被自动追踪。

**响应式 coeffect。** 组件声明它需要哪些键；提供者出现即激活它，提供者离开即去激活它，与它无关的变化不打扰它。依赖不可用不是错误，只是保持非活动。

**惯性。** 一次转换（加载或卸载）一旦开始就跑到完成，期间的目标变化被记下但不打断它；完成时若目标已变，立刻链接进下一次转换。这是并发正确性的来源，也是最容易实现错的一处。

## 快速开始

```toml
[dependencies]
cordis-rs = { git = "https://github.com/curtiseng/cordis-rs" }
```

crate 名是 `cordis`（包名跟仓库对齐）。完整的最小例子见 `src/lib.rs` 顶部的文档，`cargo test --doc` 会真的把它跑一遍。

## 与论文的对应

| 论文 | 这里 |
|---|---|
| $\Gamma_\infty$，一等上下文 | `Context` |
| $\mathrm{effect}_\Gamma(e)$（算法 1） | `Context::effect` |
| $\mathfrak{E}^{\mathrm{iter}}_\Gamma$（定义 51） | `Steps::step` |
| $\mathrm{set}(k,v)$、$\mathrm{get}(k)$（算法 2） | `Context::set`、`Context::lookup` |
| `notify`（算法 3） | `Runtime::notify` |
| $\mathrm{isolate}(k,r)$（定义 29） | `Context::isolate` |
| `ctx.use`（算法 4） | `Context::use_component` |
| `refresh` / `reload` / `unload`（算法 5） | 同名私有函数 |
| proxy 中介的上下文访问（算法 6） | `Context::resolve` |
| `fiber.state`（定义 44 的 $\theta$） | `State` |
| `fiber.uid`、`fiber.committed`、`fiber.inertia` | 代际索引、已提交视图、`Shared` future |
| **O-Insert** / **O-Retire** / **O-Remove** | `use_component` / `FiberHandle::dispose` / 卸载完成后移出竞技场 |
| **L-Leave** 与 **L-Unload** 上的守卫 | `refresh` 先标记 `Unloading`，`unload` 先排空依赖方 |

## Rust 里的七个设计决定

论文 6.4 节说这套范式与语言无关，但要求宿主语言在两个维度上满足若干条件。Rust 满足它们，只是路径和 TypeScript 不同。

**1. 逆用 `FnOnce`，于是「恢复至多一次」是类型保证。** TypeScript 版需要一个 `armed` 布尔来防止逆被跑两次；这里所有权移动本身就是那个保证。

**2. fiber 存在竞技场里，互相只存 key。** 不用 `Rc<RefCell<Fiber>>`，原因有两个：父子双向引用会构成 `Rc` 环，而算法 3 的 `notify` 要在遍历 fiber 的同时改它们，共享可变借用必然在运行时炸。竞技场把两个问题一起消掉，代价是所有访问都要过 `with_fiber`。

**3. `uid` 用代际索引。** 论文要求 uid「新鲜取得且永不复用」，好让被替换的提供者不与替换者混同。`slotmap` 的代际正是这个语义，于是这条要求由类型系统兜住，而不是靠自增计数器的纪律。

**4. 惯性句柄用 `Shared` 而不是 `JoinHandle`。** 算法 5 第 25 行要求多个依赖方同时等待同一次转换，而 Rust 的 future 是单消费者、`JoinHandle` 不能克隆。JavaScript 的 promise 可以被任意多次 await，这个差异必须显式处理。

**5. 取消是协作式的，绝不 abort。** 守卫在每个 step 边界检查 target，语义是「停在边界、保留已累积的逆」。在任意 await 点砍断任务会让已产出的逆丢失，直接违反定理 64 的部分回滚。

**6. 效应迭代器改成登记面。** `gen` / `async gen` 块至今仍是 nightly，所以不用「yield 出逆」，而是让组件往 `Steps` 上推——每次 `step` 就是一个步骤边界。这里比论文更保守一点：先登记逆再检查守卫，因此被中断的那一步同样会被回滚。

**7. 没有 `Proxy`，所以访问走类型化访问器。** 论文算法 6 用 JavaScript 的 `Proxy` 中介 `ctx[key]`，Rust 没有对等物。`Context::resolve::<K>()` 做同一件事：沿 fiber 链向上走，在第一个已提交该键的 fiber 处授权，未声明就是 `Undeclared`。论文 6.4 节预言的另一条路——用过程宏把 `inject` 声明提升到编译期检查——尚未实现，但接口是照着那个方向留的。

另外一处不得不显式化：**Rust 没有稳定的 async Drop**（`async_drop` 仍在 nightly，`dyn` 支持正是当前的阻塞项），而 `unload` 必须 await 各个逆。所以撤回是显式的 `dispose().await`，不能挂在 `Drop` 上。

## 测试对着定理写

`cargo test` 跑 19 个测试，每个都指向论文的一条性质：

| 测试 | 论文 |
|---|---|
| `inverses_run_in_lifo_order` | 定理 61，恢复精确性 |
| `recovery_is_at_most_once` | 算法 1 的自我释放 |
| `failure_rolls_back_completed_steps` | 4.3.4 节失败，L-Raise |
| `disposing_parent_cascades_to_children` | 定义 47，实例化是父级的普通 effect |
| `transition_is_inert_while_in_flight` | 4.3.3 节惯性 + 定理 64 终结恢复 |
| `activation_follows_the_provider` | 定义 26，变化的三种分类 |
| `switching_provider_reloads_the_consumer` | 定义 46，用提供者而非值标识绑定 |
| `isolate_splits_the_binding` | 定义 29，隔离 |
| `access_is_mediated_by_the_declaration` | 5.1.4 节 + 6.3 节，基于能力的访问控制 |
| `dependency_is_readable_during_own_teardown` | **定理 63，coeffect 定序** |
| `teardown_cascades_from_the_far_end` | 定理 66，终止性 |
| `insertion_order_does_not_affect_the_end_state` | **定理 68，合流性** |
| `dependency_cycle_leaves_both_inactive` | 6.5 节，环只是永不被满足 |

其中定理 63 那条最值得看：一个「因为依赖走了才被拆解」的组件，在自己的拆解过程中仍然能读到那个正在离去的依赖。论文说这条性质由三行代码的**位置**保证，测试就是在钉这三行的位置。

## 还没有做的

这是 0.1，范围只到论文 5.1 节。以下都是明确的缺口，不是疏漏：

- **单线程。** `Rc` + `RefCell` + `LocalPool`。多线程版本要把所有 disposer 与 apply 的返回 future 加上 `Send + 'static`，组件作者会明显感到约束。
- **`Context::effect` 在调用点跑到完成**，不作为并发任务。所以「飞行中的 effect 被 dispose 中止」这一支没实现；组件层（`apply`）的守卫与部分回滚是完整的。
- **没有拦截**（定义 31 的 `@@intercept`）。访问控制元数据与细粒度策略还没有对应物。
- **没有组件加载器**（5.2 节）：声明式配置层与热模块替换都不在。HMR 在 Rust 里没有好答案，见论文 6.4 节：原生代码没有模块注册表，`dlopen`/`dlclose` 会撞上 `TypeId` 跨编译单元不一致、卸载时悬垂 vtable 等问题；wasm 组件模型更干净但要付序列化边界的代价。
- **没有服务代理**（6.2 节），因此没有负载均衡、滚动更新与跨进程调用。
- **没有过程宏**，`inject` 仍是运行时声明。

## 许可

[MIT](LICENSE)。

这是对论文的一次独立实现，与论文作者、Cordis 项目均无隶属关系。概念、算法与定理编号均出自该论文（Yifan Shi、Wei Zhang、Tianyi Cui）。想先把演算本身搞懂的话，有一门配套的通俗课：[可组合性课堂](https://github.com/curtiseng/cordis-course)。
