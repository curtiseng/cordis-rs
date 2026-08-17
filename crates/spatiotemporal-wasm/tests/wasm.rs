//! 一个 wasm 组件当 fiber 用，跟原生组件受同一套规则约束。
//!
//! 这些测试要真的 `.wasm` 产物。找不到就直接失败并让你去跑 `scripts/build-guests.sh`
//! ——比静默跳过好，跳过会得到一个「绿了但什么都没测」的测试。

use std::path::{Path, PathBuf};
use std::rc::Rc;

use spatiotemporal::{App, Context, Error, FnComponent, Key, Result, State};
use spatiotemporal_wasm::{Capabilities, WasmPlugin};

trait Database {
    fn dsn(&self) -> String;
}

struct Postgres(&'static str);
impl Database for Postgres {
    fn dsn(&self) -> String {
        self.0.to_owned()
    }
}

enum Db {}
impl Key for Db {
    type Api = dyn Database;
    const NAME: &'static str = "db";
}

/// 宿主登记过、但下面的测试不授予给 guest 的一项能力。
trait Secrets {
    fn token(&self) -> String;
}
enum SecretsKey {}
impl Key for SecretsKey {
    type Api = dyn Secrets;
    const NAME: &'static str = "secrets";
}

fn guest(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/guests")
        .join(format!("{name}.wasm"));
    assert!(
        path.exists(),
        "找不到 {}。先跑 crates/spatiotemporal-wasm/scripts/build-guests.sh",
        path.display()
    );
    path
}

/// 宿主暴露 db 和 secrets 两项能力——暴露不等于授予。
fn capabilities() -> Rc<Capabilities> {
    let mut caps = Capabilities::new();
    caps.expose::<Db, _>(|db| db.dsn());
    caps.expose::<SecretsKey, _>(|s| s.token());
    Rc::new(caps)
}

fn postgres() -> Rc<dyn spatiotemporal::Component> {
    Rc::new(FnComponent::new("pg", |ctx: Context, _steps| {
        Box::pin(async move {
            ctx.set::<Db>(Rc::new(Postgres("postgres://localhost/app")));
            Ok(())
        })
    }))
}

/// 一个 wasm 组件走完整套生命周期：装上、活跃、拆掉，逆确实到了 guest 里。
#[test]
fn a_wasm_component_lives_and_dies_like_any_fiber() {
    let mut app = App::new();
    let root = app.root();
    root.use_component(postgres());
    app.settle();

    let plugin = Rc::new(
        WasmPlugin::open(guest("probe"), capabilities(), vec!["db".to_owned()])
            .expect("probe 应该能编"),
    );
    let handle = root.use_component(plugin.clone());
    app.settle();

    assert_eq!(handle.state(), State::Active);
    assert!(plugin.logs().contains(&"probe 装上了".to_owned()));

    app.block_on(handle.dispose());
    assert!(
        plugin.logs().contains(&"probe 拆掉了".to_owned()),
        "guest 的 unload 就是这个 fiber 的逆，日志是：{:?}",
        plugin.logs()
    );
}

/// 名字来自 `.wasm` 文件——这正是内核把 `Component::name` 放宽成 `&str` 的用处。
#[test]
fn the_name_comes_from_the_file() {
    let mut app = App::new();
    let plugin = Rc::new(
        WasmPlugin::open(guest("probe"), capabilities(), Vec::new()).expect("probe 应该能编"),
    );
    let handle = app.root().use_component(plugin);
    app.settle();

    assert_eq!(&*handle.name(), "probe");
}

/// 授予的读得到，没授予的读不到——即使 guest 主动去要。
#[test]
fn only_granted_capabilities_are_visible() {
    let mut app = App::new();
    let root = app.root();
    root.use_component(postgres());
    app.settle();

    let plugin = Rc::new(
        WasmPlugin::open(guest("probe"), capabilities(), vec!["db".to_owned()])
            .expect("probe 应该能编"),
    );
    root.use_component(plugin.clone());
    app.settle();

    let logs = plugin.logs();
    assert!(
        logs.contains(&"db = postgres://localhost/app".to_owned()),
        "授予了 db 就该读到，日志是：{logs:?}"
    );
    assert!(
        logs.iter().any(|line| line.starts_with("secrets 读不到")),
        "secrets 宿主暴露过但没授予，guest 主动要也该拿不到，日志是：{logs:?}"
    );
}

/// 授予一个宿主没暴露的名字：整体拒绝，而不是静默丢掉那一项。
#[test]
fn granting_an_unexposed_capability_is_refused() {
    let error = WasmPlugin::open(
        guest("probe"),
        capabilities(),
        vec!["db".to_owned(), "并不存在的能力".to_owned()],
    )
    .expect_err("没暴露的能力不该能被授予");

    assert_eq!(error, Error::UnknownKey("并不存在的能力".into()));
}

/// 依赖不在就保持非活动，依赖一到就激活——wasm 插件在这件事上不特殊。
#[test]
fn a_wasm_plugin_waits_for_its_dependency() {
    let mut app = App::new();
    let root = app.root();

    let plugin = Rc::new(
        WasmPlugin::open(guest("probe"), capabilities(), vec!["db".to_owned()])
            .expect("probe 应该能编"),
    );
    let handle = root.use_component(plugin.clone());
    app.settle();

    assert_eq!(
        handle.state(),
        State::Inactive,
        "db 还没人提供，此时不该激活，也不该算失败"
    );
    assert_eq!(handle.error(), None);
    assert!(plugin.logs().is_empty(), "没激活就不该实例化 guest");

    let provider = root.use_component(postgres());
    app.settle();
    assert_eq!(handle.state(), State::Active);
    assert!(plugin.logs().contains(&"probe 装上了".to_owned()));

    // 提供者一走，wasm 插件跟着去激活，guest 的逆照样跑。
    app.block_on(provider.dispose());
    assert_eq!(handle.state(), State::Inactive);
    assert!(plugin.logs().contains(&"probe 拆掉了".to_owned()));
}

/// 一个赖着不走的 guest：`unload` 死循环。
///
/// 论文承诺逆**会被调用**，没承诺逆自己规矩。所以适配器必须给它设上限，否则一个
/// 卡住的 guest 会把整次卸载拖死——而卸载没有别的出路。这个测试能跑完本身就是结论。
#[test]
fn a_runaway_inverse_is_bounded() {
    let mut app = App::new();
    let plugin = Rc::new(
        WasmPlugin::open(guest("runaway"), capabilities(), Vec::new())
            .expect("runaway 应该能编")
            .with_fuel(1_000_000),
    );
    let handle = app.root().use_component(plugin.clone());
    app.settle();
    assert_eq!(handle.state(), State::Active);

    app.block_on(handle.dispose());

    let logs = plugin.logs();
    assert!(
        logs.iter().any(|line| line.contains("unload 陷入了")),
        "死循环的 unload 该被燃料打断并记下来，日志是：{logs:?}"
    );
    assert_eq!(
        handle.state(),
        State::Inactive,
        "逆陷入了，但这个 fiber 依然完成了卸载"
    );
}

/// guest 报错就是加载失败，且是普通的组件失败，不是运行时崩溃。
#[test]
fn the_adapter_reports_errors_as_component_failures() {
    fn assert_component_error(error: Error) {
        assert!(
            matches!(error, Error::Component(_)),
            "wasm 侧的问题该以组件失败的形式出现，实际是：{error:?}"
        );
    }

    let missing: Result<WasmPlugin> = WasmPlugin::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("target/guests/根本没有这个文件.wasm"),
        capabilities(),
        Vec::new(),
    );
    assert_component_error(missing.expect_err("文件不存在该报错"));
}
