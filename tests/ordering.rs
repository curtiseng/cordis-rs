//! coeffect 定序：拆解时依赖仍然可读，且依赖方先于提供者排空。
//!
//! 对应论文定理 63（时间可组合性里的 coeffect 定序）与定理 66（终止性）。
//! 这是整套实现里最容易写错的一处：论文用三行代码的**位置**来保证它。

mod common;

use std::rc::Rc;

use common::{Log, Probe, Service, Tagged};
use cordis::{App, Key, KeyId, State};

enum Db {}
impl Key for Db {
    type Api = dyn Service;
    const NAME: &'static str = "db";
}

enum Repo {}
impl Key for Repo {
    type Api = dyn Service;
    const NAME: &'static str = "repo";
}

/// 定理 63：组件在其被加载的整段时间内——**包括它自己的拆解**——读到相同的绑定。
///
/// 这条性质的实际意义是：一个「因为依赖走了才被拆解」的组件，仍然能用那个依赖
/// 完成清理（比如用数据库连接写一条注销记录）。
#[test]
fn dependency_is_readable_during_own_teardown() {
    let log = Log::new();
    let mut app = App::new();
    let root = app.root();

    let provider = {
        let log = log.clone();
        Probe::new("p", move |ctx, steps| {
            let log = log.clone();
            Box::pin(async move {
                ctx.set::<Db>(Rc::new(Tagged("pg")));
                log.push("p 提供 pg");
                let log2 = log.clone();
                steps.step(move || async move { log2.push("p 关闭连接") })?;
                Ok(())
            })
        })
    };

    let consumer = {
        let log = log.clone();
        Probe::needs("c", vec![KeyId::of::<Db>()], move |ctx, steps| {
            let log = log.clone();
            Box::pin(async move {
                log.push("c 激活");
                let log2 = log.clone();
                steps.step(move || async move {
                    match ctx.resolve::<Db>() {
                        Ok(db) => log2.push(format!("c 拆解时读到 {}", db.tag())),
                        Err(error) => log2.push(format!("c 拆解时读不到：{error}")),
                    }
                })?;
                Ok(())
            })
        })
    };

    let provider_handle = root.use_component(provider);
    root.use_component(consumer);
    app.settle();
    assert_eq!(log.lines(), vec!["p 提供 pg", "c 激活"]);

    app.block_on(provider_handle.dispose());

    assert!(
        log.contains("c 拆解时读到 pg"),
        "拆解中的组件必须还能读到那个正在离去的依赖，实际日志：{:?}",
        log.lines()
    );
    assert!(
        log.before("c 拆解时读到 pg", "p 关闭连接"),
        "依赖方必须先排空，提供者的逆才可以运行"
    );
}

/// 定理 66：拆解沿提供者图按需传导，链条从末端往回收，并且会终止。
#[test]
fn teardown_cascades_from_the_far_end() {
    let log = Log::new();
    let mut app = App::new();
    let root = app.root();

    // p 提供 Db；m 消费 Db 并提供 Repo；c 消费 Repo。
    let base = {
        let log = log.clone();
        Probe::new("p", move |ctx, steps| {
            let log = log.clone();
            Box::pin(async move {
                ctx.set::<Db>(Rc::new(Tagged("pg")));
                let log2 = log.clone();
                steps.step(move || async move { log2.push("p 撤") })?;
                Ok(())
            })
        })
    };

    let middle = {
        let log = log.clone();
        Probe::needs("m", vec![KeyId::of::<Db>()], move |ctx, steps| {
            let log = log.clone();
            Box::pin(async move {
                let db = ctx.resolve::<Db>()?;
                ctx.set::<Repo>(Rc::new(Tagged(db.tag())));
                let log2 = log.clone();
                steps.step(move || async move { log2.push("m 撤") })?;
                Ok(())
            })
        })
    };

    let leaf = {
        let log = log.clone();
        Probe::needs("c", vec![KeyId::of::<Repo>()], move |ctx, steps| {
            let log = log.clone();
            Box::pin(async move {
                let _repo = ctx.resolve::<Repo>()?;
                let log2 = log.clone();
                steps.step(move || async move { log2.push("c 撤") })?;
                Ok(())
            })
        })
    };

    let base_handle = root.use_component(base);
    let middle_handle = root.use_component(middle);
    let leaf_handle = root.use_component(leaf);
    app.settle();
    assert_eq!(base_handle.state(), State::Active);
    assert_eq!(middle_handle.state(), State::Active);
    assert_eq!(leaf_handle.state(), State::Active);

    app.block_on(base_handle.dispose());

    assert_eq!(
        log.lines(),
        vec!["c 撤", "m 撤", "p 撤"],
        "应当从链条末端往回拆"
    );
    assert_eq!(middle_handle.state(), State::Inactive);
    assert_eq!(leaf_handle.state(), State::Inactive);
}

/// 依赖回来时，整条链自己重新装起来。
#[test]
fn chain_reactivates_when_the_dependency_returns() {
    let log = Log::new();
    let mut app = App::new();
    let root = app.root();

    let base = |tag: &'static str| {
        Probe::new("p", move |ctx, _steps| {
            Box::pin(async move {
                ctx.set::<Db>(Rc::new(Tagged(tag)));
                Ok(())
            })
        })
    };

    let middle = {
        let log = log.clone();
        Probe::needs("m", vec![KeyId::of::<Db>()], move |ctx, _steps| {
            let log = log.clone();
            Box::pin(async move {
                let db = ctx.resolve::<Db>()?;
                log.push(format!("m 装在 {}", db.tag()));
                ctx.set::<Repo>(Rc::new(Tagged(db.tag())));
                Ok(())
            })
        })
    };

    let leaf = {
        let log = log.clone();
        Probe::needs("c", vec![KeyId::of::<Repo>()], move |ctx, _steps| {
            let log = log.clone();
            Box::pin(async move {
                log.push(format!("c 装在 {}", ctx.resolve::<Repo>()?.tag()));
                Ok(())
            })
        })
    };

    let first = root.use_component(base("pg"));
    let middle_handle = root.use_component(middle);
    let leaf_handle = root.use_component(leaf);
    app.settle();
    assert_eq!(log.lines(), vec!["m 装在 pg", "c 装在 pg"]);

    app.block_on(first.dispose());
    assert_eq!(leaf_handle.state(), State::Inactive);

    let _second = root.use_component(base("sqlite"));
    app.settle();

    assert_eq!(middle_handle.state(), State::Active);
    assert_eq!(leaf_handle.state(), State::Active);
    assert_eq!(
        log.lines(),
        vec!["m 装在 pg", "c 装在 pg", "m 装在 sqlite", "c 装在 sqlite"],
        "换掉链条根部会让整条链重装到新提供者上"
    );
}
