//! 动态基质需要内核让出的三件事。
//!
//! 原生组件是编译期的：名字是字面量，依赖是 `KeyId::of::<K>()`，执行器是库自带
//! 的那个。一个 wasm 组件、一段模型现写的代码、一个子进程都不是——它们的名字来自
//! 运行时，依赖只能用字符串说，而要让子进程成为一等 fiber，宿主得带自己的执行器。
//!
//! 这三条都是内核自己的缺口，跟具体是哪种基质无关。

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{Log, Probe, Service, Tagged};
use cordis::{
    App, Component, Entry, FnComponent, Kernel, Key, KeyRegistry, Loader, Registry, Spawn, State,
    Value,
};
use futures::future::LocalBoxFuture;

/* ------------------------------------------------------------------ */
/* 一、名字可以是运行时才知道的                                        */
/* ------------------------------------------------------------------ */

/// 组件的名字来自配置，而不是编译期字面量。
///
/// `tool-fs.wasm` 的名字在文件里，模型现写的那段代码根本没有编译期名字。
#[test]
fn a_component_can_be_named_at_runtime() {
    let mut registry = Registry::new();
    registry.add("wasm-host", |config: &Value| {
        let path = config
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("<未指定>")
            .to_owned();
        // 名字是拼出来的：这一行在改动之前编译不过。
        Ok(
            Rc::new(FnComponent::new(format!("wasm:{path}"), |_ctx, _steps| {
                Box::pin(async { Ok(()) })
            })) as Rc<dyn Component>,
        )
    });

    let mut app = App::new();
    let loader = Loader::new(app.root(), registry);
    app.block_on(
        loader
            .apply(vec![Entry::new("tool-fs", "wasm-host").with_config(
                serde_json::json!({ "path": "plugins/tool-fs.wasm" }),
            )]),
    )
    .expect("应当装上");

    let handle = loader.context_of("tool-fs");
    assert!(handle.is_some());
    assert_eq!(loader.state("tool-fs"), Some(State::Active));
}

/// 名字一路带到 fiber 上，诊断看得到。
#[test]
fn the_runtime_name_reaches_the_fiber() {
    let mut app = App::new();
    let name = format!("dyn-{}", 3);
    let handle = app
        .root()
        .use_component(Rc::new(FnComponent::new(name.clone(), |_ctx, _steps| {
            Box::pin(async { Ok(()) })
        })));
    app.settle();

    assert_eq!(&*handle.name(), "dyn-3");
    assert_eq!(handle.state(), State::Active);
}

/* ------------------------------------------------------------------ */
/* 二、依赖可以用字符串声明                                            */
/* ------------------------------------------------------------------ */

enum Db {}
impl Key for Db {
    type Api = dyn Service;
    const NAME: &'static str = "db";
}

/// guest 报上来的字符串变成真的 `inject`，激活语义完全照常。
///
/// 这是 WIT 导入、或者一段模型现写的代码声明 `needs: ["db"]` 时走的那条路。
#[test]
fn a_dependency_declared_by_name_behaves_like_any_other() {
    let log = Log::new();
    let mut keys = KeyRegistry::new();
    keys.add::<Db>();

    // 名字 → KeyId。原生组件写 `KeyId::of::<Db>()`，动态基质只能走这里。
    let declared = keys.resolve_all(&["db"]).expect("已登记");

    let mut app = App::new();
    let root = app.root();
    let consumer = {
        let log = log.clone();
        root.use_component(Probe::needs("guest", declared, move |ctx, _steps| {
            let log = log.clone();
            Box::pin(async move {
                log.push(format!("guest 拿到 db={}", ctx.resolve::<Db>()?.tag()));
                Ok(())
            })
        }))
    };
    app.settle();
    assert_eq!(consumer.state(), State::Inactive, "依赖还没到");

    let provider = root.use_component(Probe::new("provider", |ctx, _steps| {
        Box::pin(async move {
            ctx.set::<Db>(Rc::new(Tagged("pg")));
            Ok(())
        })
    }));
    app.settle();

    assert_eq!(consumer.state(), State::Active);
    assert!(log.contains("guest 拿到 db=pg"));

    // 撤回提供者，按名字声明的依赖方同样去激活。
    app.block_on(provider.dispose());
    assert_eq!(consumer.state(), State::Inactive);
}

/// 没登记的名字查不到。
///
/// 这条就是能力面的边界：**guest 说不出宿主没登记的键**，所以宿主始终掌握着
/// 可被动态声明的那张表。
#[test]
fn an_unregistered_name_cannot_be_declared() {
    let mut keys = KeyRegistry::new();
    keys.add::<Db>();

    let error = keys.resolve("sandbox").expect_err("没登记");
    assert_eq!(error, cordis::Error::UnknownKey("sandbox".into()));
    assert_eq!(keys.names(), vec!["db"]);
}

/// 一串名字里有一个查不到，整体失败。
///
/// 全有或全无：少了一项依赖的组件会被激活得太早。
#[test]
fn resolving_a_list_is_all_or_nothing() {
    let mut keys = KeyRegistry::new();
    keys.add::<Db>();

    assert!(keys.resolve_all(&["db", "sandbox"]).is_err());
    assert!(keys.resolve_all(&["db"]).is_ok());
}

/// 同名不同键会 panic，而不是让后来者顶掉前一个。
#[test]
#[should_panic(expected = "能力键名冲突")]
fn a_name_collision_is_refused() {
    enum Other {}
    impl Key for Other {
        type Api = dyn Service;
        const NAME: &'static str = "db";
    }

    let mut keys = KeyRegistry::new();
    keys.add::<Db>();
    keys.add::<Other>();
}

/* ------------------------------------------------------------------ */
/* 三、执行器可以是宿主自己的                                          */
/* ------------------------------------------------------------------ */

/// 一个最小的宿主执行器：把任务攒起来，由宿主决定什么时候跑。
#[derive(Default, Clone)]
struct Queue(Rc<RefCell<Vec<LocalBoxFuture<'static, ()>>>>);

impl Spawn for Queue {
    fn spawn(&self, task: LocalBoxFuture<'static, ()>) {
        self.0.borrow_mut().push(task);
    }
}

impl Queue {
    fn len(&self) -> usize {
        self.0.borrow().len()
    }

    /// 宿主排空自己的队列。
    fn drain(&self) {
        loop {
            let task = self.0.borrow_mut().pop();
            match task {
                Some(task) => futures::executor::block_on(task),
                None => break,
            }
        }
    }
}

/// 转换只在宿主真的驱动任务之后才推进。
///
/// 论文注 2 说的就是这件事：Rust 的 future 是惰性的，任务创建是宿主的职责。
/// 把它做成可注入的，是为了让子进程、套接字之类的东西能成为一等 fiber——那需要
/// 一个带 IO 的执行器，而内核本身不含任何 IO。
#[test]
fn transitions_advance_only_when_the_host_drives_them() {
    let queue = Queue::default();
    let kernel = Kernel::new(Rc::new(queue.clone()));

    let handle = kernel
        .root()
        .use_component(Rc::new(FnComponent::new("guest", |_ctx, _steps| {
            Box::pin(async { Ok(()) })
        })));

    assert_eq!(handle.state(), State::Loading, "任务已交出，宿主还没跑");
    assert_eq!(queue.len(), 1);

    queue.drain();
    assert_eq!(handle.state(), State::Active);
}

/// 等待者会就地驱动它所等的那次转换。
///
/// `inertia` 是一个 `Shared`，谁 await 谁就承担 poll。所以 `quiesce` 在宿主执行器
/// 之外也能把系统推到静止——这正是它作为同步原语可靠的原因。
#[test]
fn awaiting_drives_the_transition_in_place() {
    let queue = Queue::default();
    let kernel = Kernel::new(Rc::new(queue.clone()));
    let root = kernel.root();

    let handle = root.use_component(Rc::new(FnComponent::new("guest", |_ctx, _steps| {
        Box::pin(async { Ok(()) })
    })));
    assert_eq!(handle.state(), State::Loading);

    futures::executor::block_on(root.quiesce());

    assert_eq!(handle.state(), State::Active, "没排空队列也到位了");
}
