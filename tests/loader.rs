//! 声明式配置层的对账语义（论文 5.2.1 节）。
//!
//! 这一整层不引入任何新机制：每条断言最终都落在可撤销 effect 与 fiber 生命
//! 周期上。所以这些测试真正在钉的是**配置热重载完全是可撤销性的一个应用**。

mod common;

use std::rc::Rc;

use common::{Log, Probe, Service, Tagged, yield_once};
use spatiotemporal::{App, Component, Entry, Error, Key, KeyId, Loader, Registry, State, Value};

enum Db {}
impl Key for Db {
    type Api = dyn Service;
    const NAME: &'static str = "db";
}

/// 一个普通的行：加载时记一笔，卸载时记一笔，并把 config 里的 `tag` 带上。
fn plain(
    log: Log,
    name: &'static str,
) -> impl Fn(&Value) -> spatiotemporal::Result<Rc<dyn Component>> {
    move |config: &Value| {
        let tag = config
            .get("tag")
            .and_then(Value::as_str)
            .unwrap_or("默认")
            .to_owned();
        let fails = config
            .get("fails")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let slow = config.get("slow").and_then(Value::as_bool).unwrap_or(false);
        let log = log.clone();
        Ok(Probe::new(name, move |_ctx, steps| {
            let log = log.clone();
            let tag = tag.clone();
            Box::pin(async move {
                if slow {
                    yield_once().await;
                }
                if fails {
                    return Err(Error::msg(format!("{name} 按配置要求失败")));
                }
                log.push(format!("{name} 装上（{tag}）"));
                let undo = log.clone();
                let undo_tag = tag.clone();
                steps
                    .step(move || async move { undo.push(format!("{name} 拆掉（{undo_tag}）")) })?;
                Ok(())
            })
        }) as Rc<dyn Component>)
    }
}

/// 提供 `db` 的行。
fn provider(
    log: Log,
    tag: &'static str,
) -> impl Fn(&Value) -> spatiotemporal::Result<Rc<dyn Component>> {
    move |_config: &Value| {
        let log = log.clone();
        Ok(Probe::new("provider", move |ctx, _steps| {
            let log = log.clone();
            Box::pin(async move {
                ctx.set::<Db>(Rc::new(Tagged(tag)));
                log.push(format!("提供 db={tag}"));
                Ok(())
            })
        }) as Rc<dyn Component>)
    }
}

/// 依赖 `db` 的行。
fn consumer(log: Log) -> impl Fn(&Value) -> spatiotemporal::Result<Rc<dyn Component>> {
    move |_config: &Value| {
        let log = log.clone();
        Ok(
            Probe::needs("consumer", vec![KeyId::of::<Db>()], move |ctx, _steps| {
                let log = log.clone();
                Box::pin(async move {
                    let db = ctx.resolve::<Db>()?;
                    log.push(format!("consumer 看到 db={}", db.tag()));
                    Ok(())
                })
            }) as Rc<dyn Component>,
        )
    }
}

fn registry(log: &Log) -> Registry {
    let mut registry = Registry::new();
    registry
        .add("alpha", plain(log.clone(), "alpha"))
        .add("beta", plain(log.clone(), "beta"))
        .add("gamma", plain(log.clone(), "gamma"))
        .add("provider-en", provider(log.clone(), "en"))
        .add("provider-zh", provider(log.clone(), "zh"))
        .add("consumer", consumer(log.clone()));
    registry
}

fn rows(ids: &[(&str, &str)]) -> Vec<Entry> {
    ids.iter()
        .map(|(id, name)| Entry::new(*id, *name))
        .collect()
}

/// 每一行都变成一个 fiber，顺序与配置一致。
#[test]
fn entries_become_fibers() {
    let log = Log::new();
    let mut app = App::new();
    let loader = Loader::new(app.root(), registry(&log));

    let applied = app
        .block_on(loader.apply(rows(&[("a", "alpha"), ("b", "beta")])))
        .expect("应当装上");

    assert_eq!(applied.created, vec!["a", "b"]);
    assert_eq!(loader.ids(), vec!["a", "b"]);
    assert_eq!(loader.state("a"), Some(State::Active));
    assert_eq!(
        log.lines(),
        vec!["alpha 装上（默认）", "beta 装上（默认）"],
        "顺序应当照配置"
    );
}

/// 没变的行不被碰。
///
/// 这是增量对账的全部意义：改一行不该让整棵树重启。
#[test]
fn unchanged_rows_are_left_alone() {
    let log = Log::new();
    let mut app = App::new();
    let loader = Loader::new(app.root(), registry(&log));
    let config = rows(&[("a", "alpha"), ("b", "beta")]);

    app.block_on(loader.apply(config.clone())).expect("首次");
    log.clear();

    let applied = app.block_on(loader.apply(config)).expect("再来一次");

    assert!(applied.is_noop(), "同一份配置不该产生任何变更");
    assert!(log.lines().is_empty(), "组件不该被重新加载");
}

/// 改一行的 config 只重挂那一行。
#[test]
fn a_config_change_reloads_only_that_row() {
    let log = Log::new();
    let mut app = App::new();
    let loader = Loader::new(app.root(), registry(&log));

    app.block_on(loader.apply(rows(&[("a", "alpha"), ("b", "beta")])))
        .expect("首次");
    log.clear();

    let mut next = rows(&[("a", "alpha"), ("b", "beta")]);
    next[1].config = serde_json::json!({ "tag": "改过" });
    let applied = app.block_on(loader.apply(next)).expect("重配");

    assert_eq!(applied.updated, vec!["b"]);
    assert!(applied.created.is_empty() && applied.removed.is_empty());
    assert_eq!(
        log.lines(),
        vec!["beta 拆掉（默认）", "beta 装上（改过）"],
        "只有 b 动了，而且是先拆后装"
    );
}

/// `disabled: true` 把一行拆掉，但它仍留在配置里。
#[test]
fn disabling_a_row_unloads_just_it() {
    let log = Log::new();
    let mut app = App::new();
    let loader = Loader::new(app.root(), registry(&log));

    app.block_on(loader.apply(rows(&[("a", "alpha"), ("b", "beta")])))
        .expect("首次");
    log.clear();

    let mut next = rows(&[("a", "alpha"), ("b", "beta")]);
    next[0].disabled = true;
    let applied = app.block_on(loader.apply(next)).expect("关掉 a");

    assert_eq!(applied.removed, vec!["a"]);
    assert_eq!(loader.ids(), vec!["b"]);
    assert_eq!(log.lines(), vec!["alpha 拆掉（默认）"]);
}

/// 移除多行时按配置逆序拆除。
#[test]
fn rows_unload_in_reverse_order() {
    let log = Log::new();
    let mut app = App::new();
    let loader = Loader::new(app.root(), registry(&log));

    app.block_on(loader.apply(rows(&[("a", "alpha"), ("b", "beta"), ("c", "gamma")])))
        .expect("首次");
    log.clear();

    app.block_on(loader.apply(rows(&[("a", "alpha")])))
        .expect("只留 a");

    assert_eq!(
        log.lines(),
        vec!["gamma 拆掉（默认）", "beta 拆掉（默认）"],
        "后装的先拆"
    );
}

/// 注册表里没有的名字，在任何 fiber 被动过之前就失败。
///
/// 对应 dsh 的「先导入变化后的模块名，再 dispose 活动 fiber」：一次写坏的编辑
/// 不该先把系统拆一半。
#[test]
fn an_unknown_name_fails_before_touching_anything() {
    let log = Log::new();
    let mut app = App::new();
    let loader = Loader::new(app.root(), registry(&log));

    app.block_on(loader.apply(rows(&[("a", "alpha")])))
        .expect("首次");
    log.clear();

    // 这份配置同时删掉 a 并加入一个不存在的组件。
    let error = app
        .block_on(loader.apply(rows(&[("x", "不存在的包")])))
        .expect_err("应当被拒");

    assert_eq!(error, Error::Unknown("不存在的包".into()));
    assert_eq!(loader.ids(), vec!["a"], "a 应当毫发无伤");
    assert!(log.lines().is_empty(), "不该有任何装卸发生");
}

/// 一行加载失败时，回滚到先前那棵树。
#[test]
fn a_failing_row_rolls_back_to_the_previous_tree() {
    let log = Log::new();
    let mut app = App::new();
    let loader = Loader::new(app.root(), registry(&log));

    app.block_on(loader.apply(rows(&[("a", "alpha")])))
        .expect("首次");
    log.clear();

    let mut next = rows(&[("a", "alpha")]);
    next[0].config = serde_json::json!({ "fails": true });
    let error = app.block_on(loader.apply(next)).expect_err("应当失败");

    assert!(
        matches!(&error, Error::Component(message) if message.contains("已回滚")),
        "得到的是 {error}"
    );
    assert_eq!(loader.ids(), vec!["a"]);
    assert_eq!(loader.state("a"), Some(State::Active), "旧配置被重建");
    assert_eq!(
        log.lines(),
        vec!["alpha 拆掉（默认）", "alpha 装上（默认）"],
        "候选没能装上，于是先前那行被重新装回"
    );
}

/// 等着依赖的行不是失败，是一条有效的 pending 配置项。
///
/// 这条把配置层与空间可组合性接上：论文说依赖不可用只是保持非活动。所以
/// 「消费者写在提供者前面」不需要用户去操心顺序。
#[test]
fn a_row_waiting_for_its_dependency_is_not_a_failure() {
    let log = Log::new();
    let mut app = App::new();
    let loader = Loader::new(app.root(), registry(&log));

    app.block_on(loader.apply(rows(&[("c", "consumer")])))
        .expect("消费者单独存在也应当成功");
    assert_eq!(loader.state("c"), Some(State::Inactive));
    assert_eq!(loader.error("c"), None, "非活动不是错误");

    // 提供者后到，消费者自己就活了。
    app.block_on(loader.apply(rows(&[("c", "consumer"), ("p", "provider-en")])))
        .expect("加上提供者");

    assert_eq!(loader.state("c"), Some(State::Active));
    assert!(log.contains("consumer 看到 db=en"));
}

/// 换掉提供者那一行，消费者自己重载。
///
/// 这就是「改一行配置换掉模型或沙箱」在演算里的样子：消费者没有写任何重连逻辑。
#[test]
fn swapping_a_provider_row_reloads_its_consumers() {
    let log = Log::new();
    let mut app = App::new();
    let loader = Loader::new(app.root(), registry(&log));

    app.block_on(loader.apply(rows(&[("p", "provider-en"), ("c", "consumer")])))
        .expect("首次");
    assert!(log.contains("consumer 看到 db=en"));
    log.clear();

    app.block_on(loader.apply(rows(&[("p", "provider-zh"), ("c", "consumer")])))
        .expect("换提供者");

    assert_eq!(loader.state("c"), Some(State::Active));
    assert!(log.contains("consumer 看到 db=zh"));
    assert!(
        log.before("提供 db=zh", "consumer 看到 db=zh"),
        "得到的是 {:?}",
        log.lines()
    );
}

/// 并发的 apply 被串行化并合并，最终收敛到最后一份期望状态。
///
/// dsh 在这里踩过一个三方死锁：HMR 的初始扫描在首次 apply 还没结束时触发了
/// refresh，两轮对账在同一行上交叉执行 create 与回滚。所以串行化是正确性要求，
/// 不是吞吐取舍。
#[test]
fn concurrent_applies_are_serialized_and_coalesced() {
    let log = Log::new();
    let mut app = App::new();
    let loader = Loader::new(app.root(), registry(&log));

    // 第一份配置里的行会 await 一次，于是第二次 apply 一定落在对账进行中。
    let mut first = rows(&[("a", "alpha")]);
    first[0].config = serde_json::json!({ "slow": true });
    let second = rows(&[("a", "alpha"), ("b", "beta")]);

    let (left, right) =
        app.block_on(async { futures::join!(loader.apply(first), loader.apply(second)) });

    let left = left.expect("第一次");
    let right = right.expect("第二次");
    assert!(!left.coalesced, "先到的那次自己在跑");
    assert!(right.coalesced, "后到的那次应当被合并");
    assert_eq!(left.passes, 2, "同一次调用里跑了两轮对账");
    assert_eq!(loader.ids(), vec!["a", "b"], "收敛到最后一份期望状态");
}

/// 卸载整棵树后不留任何逆。
#[test]
fn unload_all_leaves_nothing_behind() {
    let log = Log::new();
    let mut app = App::new();
    let loader = Loader::new(app.root(), registry(&log));

    app.block_on(loader.apply(rows(&[("a", "alpha"), ("b", "beta")])))
        .expect("首次");
    log.clear();

    app.block_on(loader.unload_all());

    assert!(loader.ids().is_empty());
    assert_eq!(log.lines(), vec!["beta 拆掉（默认）", "alpha 拆掉（默认）"]);
}
