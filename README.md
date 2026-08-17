# spatiotemporal

时空可组合性演算的 Rust 实现。

论文《[A Programming Paradigm for Spatiotemporal Composability](https://github.com/cordiverse/paper)》第 5 章给出了一个核心库，把可撤销 effect 与响应式 coeffect 实现成可用的编程抽象；原文的参考实现 [Cordis](https://github.com/cordiverse/cordis) 是 TypeScript 写的。这里是同一套语义的 Rust 版本：**核心库（5.1 节）加声明式配置层（5.2.1 节）**。

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
spatiotemporal = { git = "https://github.com/curtiseng/cordis-rs" }
```

完整的最小例子见 `src/lib.rs` 顶部的文档，`cargo test --doc` 会真的把它跑一遍。

包名叫 `spatiotemporal` 而仓库还叫 `cordis-rs`，是因为 crates.io 上的 `cordis-rs` 已经属于[另一个项目](https://github.com/dshbox/cordis-rs)——那是 Cordis **框架 API 面**的移植（service、events、reflect、logger），这里是**演算本身**的实现，测试逐条对着定理写。两件不同的事，名字撞了而已。

## 仓库结构

```
Cargo.toml                      # 内核，同时是 workspace 根
src/ tests/ examples/
crates/spatiotemporal-wasm/     # wasm 基质适配器，独立版本、独立发布
```

`default-members` 只含内核，所以 `cargo test` 不会去编一个 wasm 运行时——内核有 5 个依赖、MSRV 1.85，而 wasmtime 一家就带上百个、要 1.94。但 **lockfile 是整个 workspace 共享的**（`default-members` 只影响编译，不影响解析），所以卫星 crate 理论上能把某个共享依赖的版本拉高、把内核的实际 MSRV 悄悄推上去。CI 里有一个钉在 1.85 上只编内核的 job 专门守这件事。

## 配置热重载

`Loader` 把一棵配置树对账成一组活着的 fiber。配置变了就把差异增量地施加上去——**没有任何新机制**：每次变更最终都是 `use_component` 与 `dispose`，所以配置热重载是可撤销 effect 的一个应用，而不是它之外的另一套东西。

一份基础配置加一层用户 patch，形状照 dsh 的 `cordis.yml` + `cordis.patch.yml`：

```yaml
# cordis.yml
- id: sandbox
  name: dsh-sandbox-local

- id: tool-bash        # 它声明了 sandbox，但不知道是谁在提供
  name: dsh-tool-bash

- id: tool-web
  name: dsh-tool-web
  config:
    fetch: false
```

```yaml
# cordis.patch.yml —— 用户层，叠加在上面
- id: tool-web
  config:
    fetch: true              # 按 id 的 patch 替换整个 config，没改的字段也要重述
    searchTimeoutMs: 60000

- id: sandbox
  disabled: true             # 换实现 = 关掉旧行

- insert:
    - id: sandbox-remote     # 加上新行
      name: dsh-sandbox-remote
```

保存之后，`tool-web` 那一行重挂，`sandbox` 换成远端，而 `tool-bash` **自己**去激活再重新激活——它没有写任何重连逻辑。`cargo run --example watch_config` 用 `notify` 把这套跑起来，包括「写坏配置不会杀死运行中的树」那一支。文件监听刻意留在库外面：它属于宿主的职责。

这一层有五个值得单独说的决定：

**先构造，再拆除。** 注册表里没有的名字、不合法的配置，都在任何 fiber 被动过**之前**同步失败。所以一次写坏的编辑不会先把系统拆一半——这是 dsh「先导入变化后的模块名，再 dispose 活动 fiber」的同一条。

**补偿事务，不是不可见的原子替换。** 任意组件的 effect 无法快照，所以不承诺中间状态不可见。候选失败时拆掉候选、重建先前的行，并如实报告 `Error::Rollback` 而不是声称树被完好保留。重建出来的是**新的** fiber：可撤销性保证逆会被运行，不保证时间被倒流。

**串行且合并。** 同一个 loader 上的并发 `apply` 不会交错，第二个调用只更新「期望状态」然后立刻返回，由正在跑的那一轮拾取。这不是吞吐取舍而是正确性要求：对账过程中会 await，两轮交错的对账会在同一行上交叉执行 create 与回滚。dsh 在这里踩过一个三方死锁（回滚等 HMR 拆卸、HMR 等自己的 refresh、refresh 排在正在回滚的 apply 后面），`tests/loader.rs` 里那条测试就是钉这个的。

**patch 里的 `name` 是断言而不是赋值。** 对不上就整条跳过并留一条警告。理由是一层 patch 可能是为另一套组合写的，而 id 撞车时静默地重配了另一个插件，比这条 patch 不生效危险得多。这条也照 dsh。

**注册表必须显式建立。** Rust 没有运行时模块注册表（论文 6.4 节把这条列为原生语言的固有差异），所以 `name → 构造器` 这张表要手写。交换条件是：加一个新组件要动宿主一行代码并重新编译，但**已注册组件的开关、重配、插入、移除全都不需要重启**——而那正是配置热重载所要的全部。

## 给动态基质留的三个口子

原生组件全是编译期的：名字是字面量，依赖写成 `KeyId::of::<K>()`，执行器用库自带的那个。一个 wasm 组件、一段模型现写的代码、一个子进程都不是。

要紧的是**基质不需要是内核概念**。`Registry` 已经是 `name → 构造器`，wasm、script、remote 插件都只是 `Component` 的不同实现——各自的 `apply` 去调 wasmtime、QuickJS 或子进程，把注册动作用 `steps.step` 登记逆。所以适配器属于独立的 crate（wasmtime 一家就带上百个依赖，而这个 crate 现在只有四个），内核只让出三处：

**名字可以是运行时的。** `Component::name()` 返回 `&str` 而不是 `&'static str`。静态名字照常写字面量，动态的可以来自 wasm 文件名或直接拼出来。

**依赖可以用字符串声明。** `KeyId` 的同一性依据是 `TypeId`，运行时凭字符串构造不出来，所以有一张 `KeyRegistry` 做翻译：

```rust
let mut keys = KeyRegistry::new();
keys.add::<Tools>().add::<Shell>();

// guest 的 WIT 导入报上来的字符串，在这里变成能填进 inject 的键
let declared = keys.resolve_all(&["tools", "shell"])?;
```

这张表跟 `Registry` 是一对：那张说「哪些组件可以被装上」，这张说「哪些能力可以被按名字声明」。两张都由宿主显式建立，于是**guest 说不出宿主没登记的键**——能力面的边界就在这里，而不在 guest 的诚实程度上。同名不同键会 panic，因为静默顶掉前一个等于让一个 guest 拿到别人的能力。

**执行器可以是宿主自己的。** 论文注 2 说任务创建是宿主的职责，现在它是一个可注入的 `Spawn`。内核本身不含任何 IO，要让子进程或套接字成为一等 fiber，就得把带 IO 的执行器接进来：

```rust
struct LocalSetSpawner(tokio::task::LocalSet);   // 示意

impl Spawn for LocalSetSpawner {
    fn spawn(&self, task: LocalBoxFuture<'static, ()>) {
        self.0.spawn_local(task);
    }
}

let kernel = Kernel::new(Rc::new(spawner));      // App 是自带执行器的那个便利壳
```

一处值得知道的实现细节：`inertia` 是 `Shared`，谁 await 谁承担 poll，所以 `quiesce()` 会把它等的那次转换就地驱动完，不依赖宿主执行器是否勤快。丢弃任务的实际后果是「没有任何依赖方去等的转换会一直停在飞行中」，而不是死锁。

## wasm 插件

`crates/spatiotemporal-wasm` 把这三个口子用上了：一个 WebAssembly 组件成为一等 fiber，跟原生组件受同一套规则约束。

```rust
// 宿主暴露它愿意让 guest 看见的能力，每一项都要写出投影。
let mut caps = Capabilities::new();
caps.expose::<Db, _>(|db| db.dsn());

// 授予哪些能力由宿主的配置决定，不是 guest 报上来的。
let plugin = WasmPlugin::open("plugins/tool-fs.wasm", Rc::new(caps), vec!["db".into()])?;
let handle = ctx.use_component(Rc::new(plugin));
```

装上时 guest 的 `load` 跑一遍，`unload` 被登记成这个 fiber 的逆——于是它跟原生组件的逆排在同一个 LIFO 序列里，由同一套惯性状态机调度。`db` 的提供者被换掉，这个 wasm 插件自己会去激活再重新激活，跟原生组件的行为逐字相同。

四个决定值得单独说：

**能力由宿主授予，不由 guest 申请。** `granted` 来自配置，而不是去问 guest 要什么。这样 `inject` 属于配置的一部分、能被静态检视，不必先把 guest 跑起来才知道它要什么；也让它成为一个授权模型——第三方送来一个 `.wasm`，是运维决定它能看见什么。授予了一个宿主没暴露的名字就整体拒绝，绝不静默丢掉那一项。

**能力在加载时刻取一次快照。** 这不是对动态性的妥协，恰好就是这套语义：论文里一个 fiber 的 committed view 在它整段活跃期内是固定的，依赖一变它就被重载。所以「装上时取一次」和「每次调用都去查」在可观察行为上没有差别。快照顺带挡掉了重入——store 里不放 `Context`，guest 就没法在自己的转换还没结束时回头调内核。这一条还是被类型系统逼出来的：`wasmtime_wasi` 的 `WasiView: Send`，而 `Rc` 不是 `Send`。

**每一项能力都要宿主写出投影。** 跨 WIT 边界的值只能是 WIT 类型，而原生 coeffect 是 `Rc<dyn Trait>`。所以「guest 不能引入新的 coeffect 种类」这条限制，在代码里就落成了 `Capabilities` 那张投影表——表里没有的东西，guest 连名字都报不出来。

**guest 的逆有燃料上限。** 论文承诺逆**会被调用**，可没承诺逆自己规矩。一个 `unload` 里死循环的 guest 会把整次卸载拖死，而卸载没有别的出路。用燃料而不是墙钟期限，是因为它不需要另起线程去推 epoch，而且确定性——同一个 guest 每次都在同一条指令上耗尽。`guests/runaway` 就是这么一个赖着不走的 guest，对应的测试能跑完本身就是结论。

测试要真的 `.wasm` 产物：

```bash
cd crates/spatiotemporal-wasm
./scripts/build-guests.sh          # 需要 rustup target add wasm32-wasip2
cargo test -p spatiotemporal-wasm
```

产物不入库——预编译的二进制没法评审也没法复现。测试找不到它会直接失败并让你回来跑这个脚本，而不是静默跳过；跳过会得到一个「绿了但什么都没测」的测试。

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
| 5.2.1 节的组件加载器 | `Loader`、`Registry`、`compose` |
| 注 2 的 `create_task` | `Spawn`、`Kernel`（宿主可自带执行器） |

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

`cargo test` 跑 54 个（内核与配置层），`cargo test -p spatiotemporal-wasm` 另跑 7 个。核心库那部分每个都指向论文的一条性质：

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

配置层那部分（`tests/loader.rs`、`tests/config.rs`）钉的是对账语义：

| 测试 | 钉的是 |
|---|---|
| `unchanged_rows_are_left_alone` | 增量对账的全部意义：改一行不该让整棵树重启 |
| `a_config_change_reloads_only_that_row` | 只有那一行动，而且先拆后装 |
| `an_unknown_name_fails_before_touching_anything` | 先构造再拆除 |
| `a_failing_row_rolls_back_to_the_previous_tree` | 补偿事务 |
| `a_row_waiting_for_its_dependency_is_not_a_failure` | 依赖不可用只是非活动，是有效的 pending 配置项 |
| `swapping_a_provider_row_reloads_its_consumers` | **改一行配置换掉实现，消费者自己跟上** |
| `concurrent_applies_are_serialized_and_coalesced` | 串行化是正确性要求 |
| `a_name_in_a_patch_is_an_assertion_not_an_assignment` | patch 的 `name` 是断言 |

`tests/dynamic.rs` 钉的是给动态基质留的那三个口子：运行时名字能一路带到 fiber 上、按字符串声明的依赖与 `KeyId::of` 声明的行为完全一致（包括提供者走了就去激活）、以及转换只在宿主真的驱动任务之后才推进。

`crates/spatiotemporal-wasm/tests/wasm.rs` 钉的是跨语言边界之后这些性质还在：

| 测试 | 钉的是 |
|---|---|
| `a_wasm_component_lives_and_dies_like_any_fiber` | guest 的 `unload` 就是这个 fiber 的逆 |
| `the_name_comes_from_the_file` | 运行时名字的实际用处 |
| `only_granted_capabilities_are_visible` | **能力边界不在 guest 的诚实程度上** |
| `granting_an_unexposed_capability_is_refused` | 全有或全无，绝不静默降级 |
| `a_wasm_plugin_waits_for_its_dependency` | 定义 26 那三种分类对 wasm 插件同样成立 |
| `a_runaway_inverse_is_bounded` | **卡住的逆会被燃料抢占，卸载仍然完成** |
| `the_adapter_reports_errors_as_component_failures` | wasm 侧的问题以组件失败出现，不是运行时崩溃 |

## 还没有做的

这是 0.3，范围到论文 5.1 节加 5.2.1 节。以下都是明确的缺口，不是疏漏：

- **单线程。** `Rc` + `RefCell` + `LocalPool`。多线程版本要把所有 disposer 与 apply 的返回 future 加上 `Send + 'static`，组件作者会明显感到约束。
- **`Context::effect` 在调用点跑到完成**，不作为并发任务。所以「飞行中的 effect 被 dispose 中止」这一支没实现；组件层（`apply`）的守卫与部分回滚是完整的。
- **没有拦截**（定义 31 的 `@@intercept`）。访问控制元数据与细粒度策略还没有对应物。
- **配置树是平的。** 没有 group／嵌套子树，因此 `insert` 只能追加到顶层。dsh 的组合包用嵌套行来把一组能力归到一个宿主行之下，那需要「一个组件持有自己的子对账器」，还没做。
- **没有热模块替换**（5.2.2 节）。这在 Rust 里没有好答案，见论文 6.4 节：原生代码没有模块注册表，`dlopen`/`dlclose` 会撞上 `TypeId` 跨编译单元不一致、卸载时悬垂 vtable 等问题；wasm 组件模型更干净但要付序列化边界的代价。**注意这跟配置热重载是两件事**——后者已经在了，而且它才是长驻进程真正需要的那件（dsh 在两个发行形态里都把宿主端模块 HMR 关掉了，却给配置层补挂一个只看配置的 watcher）。
- **没有服务代理**（6.2 节），因此没有负载均衡、滚动更新与跨进程调用。
- **没有过程宏**，`inject` 仍是运行时声明。
- **wasm 适配器只到叶子插件。** `spatiotemporal-wasm` 是 0.0.1，能力面就是 `wit/plugin.wit` 里那个 world：一条日志、一次能力读取。guest 还不能提供 coeffect 给别人消费，也不能收事件。三条不管适配器怎么写都不会变的限制：guest 不能引入**新的** coeffect 种类给原生插件消费（`Rc<dyn Trait>` 的 trait 必须在宿主编译期存在，所以 world 决定了 guest 能提供什么，而不是 guest 自己决定），大 payload 的高频事件过边界要付序列化代价（叶子工具是甜点区，流式事件不是），以及 guest 的逆必须可抢占（已经用燃料做了）。
- **脚本与子进程基质还没做。** QuickJS（模型现写的代码）和子进程（MCP 那类）各自需要一个 crate。子进程那个还需要宿主接一个带 IO 的执行器进来，因为内核本身不含任何 IO。

## 许可

[MIT](LICENSE)。

这是对论文的一次独立实现，与论文作者、Cordis 项目均无隶属关系。概念、算法与定理编号均出自该论文（Yifan Shi、Wei Zhang、Tianyi Cui）。想先把演算本身搞懂的话，有一门配套的通俗课：[可组合性课堂](https://github.com/curtiseng/cordis-course)。
