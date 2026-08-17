//! 空间可组合性：激活分类、隔离、以及依赖访问的边界。
//!
//! 对应论文 3.2 节（响应式 coeffect）、定义 26（变化的三种分类）、
//! 定义 29（隔离）与 5.1.4 节（访问在使用点被强制）。

mod common;

use std::rc::Rc;

use common::{Log, Probe, Service, Tagged};
use spatiotemporal::{App, Key, KeyId, State};

enum Db {}
impl Key for Db {
    type Api = dyn Service;
    const NAME: &'static str = "db";
}

enum Cache {}
impl Key for Cache {
    type Api = dyn Service;
    const NAME: &'static str = "cache";
}

fn provider(tag: &'static str, log: Log) -> Rc<impl spatiotemporal::Component> {
    Probe::new("provider", move |ctx, _steps| {
        let log = log.clone();
        Box::pin(async move {
            ctx.set::<Db>(Rc::new(Tagged(tag)));
            log.push(format!("提供 {tag}"));
            Ok(())
        })
    })
}

fn consumer(name: &'static str, log: Log) -> Rc<impl spatiotemporal::Component> {
    Probe::needs(name, vec![KeyId::of::<Db>()], move |ctx, steps| {
        let log = log.clone();
        Box::pin(async move {
            let db = ctx.resolve::<Db>()?;
            log.push(format!("{name} 激活于 {}", db.tag()));
            let log2 = log.clone();
            steps.step(move || async move { log2.push(format!("{name} 去激活")) })?;
            Ok(())
        })
    })
}

/// 定义 26 的三种分类：依赖不在就保持非活动且不报错；提供者一到即激活；一走即去激活。
#[test]
fn activation_follows_the_provider() {
    let log = Log::new();
    let mut app = App::new();
    let root = app.root();

    let consumer_handle = root.use_component(consumer("c", log.clone()));
    app.settle();
    assert_eq!(
        consumer_handle.state(),
        State::Inactive,
        "依赖不可用时保持非活动，且不是失败"
    );
    assert_eq!(consumer_handle.error(), None);

    let provider_handle = root.use_component(provider("pg", log.clone()));
    app.settle();
    assert_eq!(consumer_handle.state(), State::Active);
    assert!(log.contains("c 激活于 pg"));

    app.block_on(provider_handle.dispose());
    assert_eq!(consumer_handle.state(), State::Inactive);
    assert!(log.contains("c 去激活"));
}

/// 提供者被换成另一个：消费者重载一次，并解析到新的提供者。
///
/// 判据是提供者的 uid 而不是值，所以即使两个提供者给出相等的值也会被区分。
#[test]
fn switching_provider_reloads_the_consumer() {
    let log = Log::new();
    let mut app = App::new();
    let root = app.root();

    let first = root.use_component(provider("pg", log.clone()));
    let consumer_handle = root.use_component(consumer("c", log.clone()));
    app.settle();
    assert!(log.contains("c 激活于 pg"));

    // 先装第二个提供者会覆盖同一个 (键, realm) 上的绑定，
    // 所以这里按论文建议先撤回旧的，再装新的。
    app.block_on(first.dispose());
    let _second = root.use_component(provider("sqlite", log.clone()));
    app.settle();

    assert_eq!(consumer_handle.state(), State::Active);
    assert!(log.contains("c 激活于 sqlite"));
    assert!(
        log.before("c 去激活", "c 激活于 sqlite"),
        "换提供者应当先去激活再重新激活"
    );
}

/// 定义 29 的隔离：同一个键在两个 realm 下解析到互相独立的绑定。
#[test]
fn isolate_splits_the_binding() {
    let log = Log::new();
    let mut app = App::new();
    let root = app.root();

    // 左右两棵子树各自隔离 Db。
    let left = root.isolate::<Db>();
    let right = root.isolate::<Db>();

    let left_consumer = left.use_component(consumer("左", log.clone()));
    let right_consumer = right.use_component(consumer("右", log.clone()));
    app.settle();
    assert_eq!(left_consumer.state(), State::Inactive);
    assert_eq!(right_consumer.state(), State::Inactive);

    // 只在左边提供。
    let left_provider = left.use_component(provider("左库", log.clone()));
    app.settle();

    assert_eq!(left_consumer.state(), State::Active);
    assert_eq!(
        right_consumer.state(),
        State::Inactive,
        "另一个 realm 的绑定不应被看见"
    );
    assert!(log.contains("左 激活于 左库"));

    app.block_on(left_provider.dispose());
    assert_eq!(left_consumer.state(), State::Inactive);
}

/// 5.1.4 节：访问在使用点被强制。未声明的键拿不到，声明了但没被提供也拿不到。
#[test]
fn access_is_mediated_by_the_declaration() {
    let mut app = App::new();
    let root = app.root();

    let component = Probe::needs(
        "declared-db-only",
        vec![KeyId::of::<Db>()],
        move |ctx, _steps| {
            Box::pin(async move {
                // 声明过的键：能拿到。
                assert_eq!(ctx.resolve::<Db>()?.tag(), "pg");
                // 没声明的键：即使别处提供了，也是 Undeclared。
                assert!(matches!(
                    ctx.resolve::<Cache>(),
                    Err(spatiotemporal::Error::Undeclared("cache"))
                ));
                Ok(())
            })
        },
    );

    // 在根上下文直接提供两项服务：根 fiber 生来活动，所以立即可用。
    root.set::<Db>(Rc::new(Tagged("pg")));
    root.set::<Cache>(Rc::new(Tagged("mem")));

    let handle = root.use_component(component);
    app.settle();
    assert_eq!(handle.state(), State::Active, "断言都应在组件内部通过");
}

/// 未被任何 fiber 声明的键，在上下文上直接解析同样被拒绝。
#[test]
fn root_context_rejects_undeclared_access() {
    let app = App::new();
    let root = app.root();
    assert!(matches!(
        root.resolve::<Db>(),
        Err(spatiotemporal::Error::Undeclared("db"))
    ));
    // 裸查询永不失败，只是没有值。
    assert!(root.lookup::<Db>().is_none());
}
