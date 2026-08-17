//! 一段 QuickJS 脚本当 fiber 用，跟原生组件受同一套规则约束。
//!
//! 脚本就是字符串，不需要预编译产物——这正是「模型这一轮现写」和 wasm 文件
//! 的差别。测试找不到语法错误会在 `from_source` 当场失败，而不是静默跳过。

use std::rc::Rc;

use spatiotemporal::{App, Context, Error, FnComponent, Key, State};
use spatiotemporal_script::{Capabilities, ScriptPlugin};

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

trait Secrets {
    fn token(&self) -> String;
}
enum SecretsKey {}
impl Key for SecretsKey {
    type Api = dyn Secrets;
    const NAME: &'static str = "secrets";
}

const PROBE: &str = r#"
export function load() {
    host.log("probe 装上了");
    try {
        host.log("db = " + host.capability("db"));
    } catch (e) {
        host.log("db 读不到：" + String(e));
    }
    try {
        host.log("secrets = " + host.capability("secrets") + "（本该拿不到！）");
    } catch (e) {
        host.log("secrets 读不到：" + String(e));
    }
}
export function unload() {
    host.log("probe 拆掉了");
}
"#;

const RUNAWAY: &str = r#"
export function load() {
    host.log("runaway 装上了");
}
export function unload() {
    host.log("runaway 开始赖着不走");
    var n = 0;
    while (true) { n = n + 1; }
}
"#;

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

fn probe(granted: Vec<String>) -> Rc<ScriptPlugin> {
    Rc::new(
        ScriptPlugin::from_source("probe", PROBE, capabilities(), granted).expect("probe 应该能编"),
    )
}

/// 一段脚本走完整套生命周期：装上、活跃、拆掉，逆确实到了 guest 里。
#[test]
fn a_script_lives_and_dies_like_any_fiber() {
    let mut app = App::new();
    let root = app.root();
    root.use_component(postgres());
    app.settle();

    let plugin = probe(vec!["db".to_owned()]);
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

/// 名字由调用方给出——模型现写的代码没有文件名。
#[test]
fn the_name_is_whatever_the_host_called_it() {
    let mut app = App::new();
    let plugin = Rc::new(
        ScriptPlugin::from_source("dyn-3", PROBE, capabilities(), Vec::new())
            .expect("probe 应该能编"),
    );
    let handle = app.root().use_component(plugin);
    app.settle();

    assert_eq!(&*handle.name(), "dyn-3");
}

/// 授予的读得到，没授予的读不到——即使 guest 主动去要。
#[test]
fn only_granted_capabilities_are_visible() {
    let mut app = App::new();
    let root = app.root();
    root.use_component(postgres());
    app.settle();

    let plugin = probe(vec!["db".to_owned()]);
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
    let error = ScriptPlugin::from_source(
        "probe",
        PROBE,
        capabilities(),
        vec!["db".to_owned(), "并不存在的能力".to_owned()],
    )
    .expect_err("没暴露的能力不该能被授予");

    assert_eq!(error, Error::UnknownKey("并不存在的能力".into()));
}

/// 依赖不在就保持非活动，依赖一到就激活——脚本在这件事上不特殊。
#[test]
fn a_script_waits_for_its_dependency() {
    let mut app = App::new();
    let root = app.root();

    let plugin = probe(vec!["db".to_owned()]);
    let handle = root.use_component(plugin.clone());
    app.settle();

    assert_eq!(
        handle.state(),
        State::Inactive,
        "db 还没人提供，此时不该激活，也不该算失败"
    );
    assert_eq!(handle.error(), None);
    assert!(plugin.logs().is_empty(), "没激活就不该跑 guest");

    let provider = root.use_component(postgres());
    app.settle();
    assert_eq!(handle.state(), State::Active);
    assert!(plugin.logs().contains(&"probe 装上了".to_owned()));

    app.block_on(provider.dispose());
    assert_eq!(handle.state(), State::Inactive);
    assert!(plugin.logs().contains(&"probe 拆掉了".to_owned()));
}

/// 一个赖着不走的 guest：`unload` 死循环。
///
/// 论文承诺逆**会被调用**，没承诺逆自己规矩。QuickJS 没有指令燃料，用的是
/// interrupt handler。这个测试能跑完本身就是结论。
#[test]
fn a_runaway_inverse_is_bounded() {
    let mut app = App::new();
    let plugin = Rc::new(
        ScriptPlugin::from_source("runaway", RUNAWAY, capabilities(), Vec::new())
            .expect("runaway 应该能编")
            .with_fuel(100),
    );
    let handle = app.root().use_component(plugin.clone());
    app.settle();
    assert_eq!(handle.state(), State::Active);

    app.block_on(handle.dispose());

    let logs = plugin.logs();
    assert!(
        logs.iter().any(|line| line.contains("unload 陷入了")),
        "死循环的 unload 该被中断并记下来，日志是：{logs:?}"
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
    let error = ScriptPlugin::from_source(
        "坏的",
        "this is not { javascript",
        capabilities(),
        Vec::new(),
    )
    .expect_err("语法错误该在构造时失败");
    assert!(
        matches!(error, Error::Component(_)),
        "脚本侧的问题该以组件失败的形式出现，实际是：{error:?}"
    );
}

/// 没有导出 load 的脚本，装上即失败。
#[test]
fn a_script_without_load_fails_as_a_component() {
    let mut app = App::new();
    let plugin = Rc::new(
        ScriptPlugin::from_source(
            "empty",
            "export function idle() {}",
            capabilities(),
            Vec::new(),
        )
        .expect("语法是合法的"),
    );
    let handle = app.root().use_component(plugin);
    app.settle();

    assert_eq!(handle.state(), State::Failed);
    let error = handle.error().expect("该有一条组件失败");
    assert!(error.contains("没有导出 load"), "实际是：{error}");
}
