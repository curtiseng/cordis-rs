//! 一个子进程 guest 当 fiber 用，跟原生组件受同一套规则约束。

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use spatiotemporal::{App, Context, Error, FnComponent, Key, State};
use spatiotemporal_process::{Capabilities, ProcessPlugin};

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

struct TokenVault(&'static str);
impl Secrets for TokenVault {
    fn token(&self) -> String {
        self.0.to_owned()
    }
}

fn guest(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("target/guests/{name}"))
}

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

fn probe(granted: Vec<String>) -> Rc<ProcessPlugin> {
    Rc::new(
        ProcessPlugin::open(guest("probe"), capabilities(), granted)
            .expect("probe 应该能打开")
            .with_name("probe"),
    )
}

/// 一个子进程走完整套生命周期：装上、活跃、拆掉，逆确实到了 guest 里。
#[test]
fn a_process_lives_and_dies_like_any_fiber() {
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

/// 名字由调用方给出——可执行文件 stem 可被 `with_name` 覆盖。
#[test]
fn the_name_is_whatever_the_host_called_it() {
    let mut app = App::new();
    let plugin = Rc::new(
        ProcessPlugin::open(guest("probe"), capabilities(), Vec::new())
            .expect("probe 应该能打开")
            .with_name("dyn-3"),
    );
    let handle = app.root().use_component(plugin);
    app.settle();
    assert_eq!(&*handle.name(), "dyn-3");
}

/// 只授予的能力可见；没授予的名字 guest 读不到。
#[test]
fn only_granted_capabilities_are_visible() {
    let mut app = App::new();
    let root = app.root();
    root.use_component(postgres());
    root.use_component(Rc::new(FnComponent::new("vault", |ctx: Context, _steps| {
        Box::pin(async move {
            ctx.set::<SecretsKey>(Rc::new(TokenVault("secret-token")));
            Ok(())
        })
    })));
    app.settle();

    let plugin = probe(vec!["db".to_owned()]);
    let handle = root.use_component(plugin.clone());
    app.settle();

    assert_eq!(handle.state(), State::Active);
    let logs = plugin.logs();
    assert!(logs.iter().any(|line| line.contains("db = postgres")));
    assert!(logs.iter().any(|line| line.contains("secrets 读不到")));
}

/// 授予了一个宿主没暴露的名字就整体拒绝。
#[test]
fn granting_an_unexposed_capability_is_refused() {
    let error = ProcessPlugin::open(
        guest("probe"),
        capabilities(),
        vec!["db".to_owned(), "sandbox".to_owned()],
    )
    .expect_err("sandbox 没暴露");
    assert!(
        matches!(error, Error::UnknownKey(ref name) if name == "sandbox"),
        "实际是：{error:?}"
    );
}

/// guest 报上来的字符串变成真的 `inject`，激活语义完全照常。
#[test]
fn a_process_waits_for_its_dependency() {
    let mut app = App::new();
    let root = app.root();
    let consumer = root.use_component(probe(vec!["db".to_owned()]));
    app.settle();
    assert_eq!(consumer.state(), State::Inactive);

    let provider = root.use_component(postgres());
    app.settle();
    assert_eq!(consumer.state(), State::Active);

    app.block_on(provider.dispose());
    assert_eq!(consumer.state(), State::Inactive);
}

/// 一个赖着不走的 guest：`unload` 不回应。
#[test]
fn a_runaway_inverse_is_bounded() {
    let mut app = App::new();
    let plugin = Rc::new(
        ProcessPlugin::open(guest("runaway"), capabilities(), Vec::new())
            .expect("runaway 应该能打开")
            .with_unload_timeout(Duration::from_millis(200)),
    );
    let handle = app.root().use_component(plugin.clone());
    app.settle();
    assert_eq!(handle.state(), State::Active);

    app.block_on(handle.dispose());

    let logs = plugin.logs();
    assert!(
        logs.iter().any(|line| line.contains("unload 陷入了")),
        "不回应的 unload 该被超时打断并记下来，日志是：{logs:?}"
    );
    assert_eq!(handle.state(), State::Inactive);
}

/// guest 报错就是加载失败，且是普通的组件失败。
#[test]
fn the_adapter_reports_errors_as_component_failures() {
    let missing = ProcessPlugin::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("target/guests/根本没有这个文件"),
        capabilities(),
        Vec::new(),
    );
    assert!(
        matches!(missing, Err(Error::Component(_))),
        "文件不存在该以组件失败的形式出现"
    );
}
