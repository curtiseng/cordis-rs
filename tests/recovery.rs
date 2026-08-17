//! 时间可组合性：逆的定序、至多一次、失败时的部分回滚。
//!
//! 对应论文 3.1 节与定理 61（恢复精确性）、4.3.4 节（失败）。

mod common;

use std::rc::Rc;

use common::{Log, Probe};
use cordis::{App, State};

/// 定理 61：逆按 LIFO 回放，因此后装的先撤。
#[test]
fn inverses_run_in_lifo_order() {
    let log = Log::new();
    let mut app = App::new();

    let component = {
        let log = log.clone();
        Probe::new("three-steps", move |_ctx, steps| {
            let log = log.clone();
            Box::pin(async move {
                for tag in ["a", "b", "c"] {
                    log.push(format!("装 {tag}"));
                    let log = log.clone();
                    steps.step(move || async move {
                        log.push(format!("撤 {tag}"));
                    })?;
                }
                Ok(())
            })
        })
    };

    let handle = app.root().use_component(component);
    app.settle();
    assert_eq!(handle.state(), State::Active);
    assert_eq!(log.lines(), vec!["装 a", "装 b", "装 c"]);

    log.clear();
    app.block_on(handle.dispose());
    assert_eq!(log.lines(), vec!["撤 c", "撤 b", "撤 a"]);
}

/// 恢复至多触发一次：重复撤销是无操作。
///
/// 论文用一个 `armed` 标志实现这条；这里由「取走那一组逆」保证，
/// 而单个逆的类型是 `FnOnce`，连误用两次的可能都没有。
#[test]
fn recovery_is_at_most_once() {
    let log = Log::new();
    let mut app = App::new();

    let component = {
        let log = log.clone();
        Probe::new("once", move |_ctx, steps| {
            let log = log.clone();
            Box::pin(async move {
                let log2 = log.clone();
                steps.step(move || async move { log2.push("撤销") })?;
                Ok(())
            })
        })
    };

    let handle = app.root().use_component(component);
    app.settle();

    app.block_on(handle.dispose());
    app.block_on(handle.dispose());
    app.block_on(handle.dispose());

    assert_eq!(log.lines(), vec!["撤销"], "逆只应运行一次");
}

/// 组件中途失败时，已登记的逆仍被回卷（L-Raise 之后的拆解）。
#[test]
fn failure_rolls_back_completed_steps() {
    let log = Log::new();
    let mut app = App::new();

    let component = {
        let log = log.clone();
        Probe::new("half-way", move |_ctx, steps| {
            let log = log.clone();
            Box::pin(async move {
                log.push("装 a");
                let a = log.clone();
                steps.step(move || async move { a.push("撤 a") })?;

                log.push("装 b");
                let b = log.clone();
                steps.step(move || async move { b.push("撤 b") })?;

                Err(cordis::Error::msg("第三步失败了"))
            })
        })
    };

    let handle = app.root().use_component(component);
    app.settle();

    assert_eq!(handle.state(), State::Failed);
    assert_eq!(handle.error().as_deref(), Some("第三步失败了"));
    assert_eq!(
        log.lines(),
        vec!["装 a", "装 b", "撤 b", "撤 a"],
        "失败不该留下半装状态"
    );
}

/// 实例化是父级的一个普通 effect，因此卸载父级会级联到子代，
/// 且子代先于父级自己的逆被撤销。
#[test]
fn disposing_parent_cascades_to_children() {
    let log = Log::new();
    let mut app = App::new();

    let child = {
        let log = log.clone();
        Probe::new("child", move |_ctx, steps| {
            let log = log.clone();
            Box::pin(async move {
                let log2 = log.clone();
                steps.step(move || async move { log2.push("撤 子") })?;
                Ok(())
            })
        })
    };

    let parent = {
        let log = log.clone();
        Probe::new("parent", move |ctx, steps| {
            let log = log.clone();
            let child = child.clone();
            Box::pin(async move {
                let log2 = log.clone();
                steps.step(move || async move { log2.push("撤 父自己的 effect") })?;
                // 在自己的上下文里实例化子代：这也是一个被追踪的 effect。
                ctx.use_component(child);
                Ok(())
            })
        })
    };

    let handle = app.root().use_component(parent);
    app.settle();

    app.block_on(handle.dispose());
    assert_eq!(
        log.lines(),
        vec!["撤 子", "撤 父自己的 effect"],
        "子代的撤回后装先撤，父级自己的 effect 随后"
    );
}

/// 惯性（4.3.3 节）：转换飞行中的目标变化被记下，但不打断它。
///
/// 这个测试卡住一次加载，然后在它中途撤回该组件，观察三件事：
/// 加载没有被打断（状态仍是 `Loading`）、恢复后立刻链接进卸载、
/// 以及在过期的 step 边界上登记的那个逆同样会被回卷。
#[test]
fn transition_is_inert_while_in_flight() {
    let log = Log::new();
    let mut app = App::new();

    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    let rx = Rc::new(std::cell::RefCell::new(Some(rx)));

    let component = {
        let log = log.clone();
        Probe::new("slow", move |_ctx, steps| {
            let log = log.clone();
            let rx = rx.borrow_mut().take().expect("只会被实例化一次");
            Box::pin(async move {
                log.push("加载开始");
                let _ = rx.await;
                let log2 = log.clone();
                // 此时 target 已被撤回改成 ⊥，守卫在这里失效。
                steps.step(move || async move { log2.push("撤 慢") })?;
                log.push("加载完成");
                Ok(())
            })
        })
    };

    let handle = app.root().use_component(component);
    app.settle();
    assert_eq!(handle.state(), State::Loading);
    assert_eq!(log.lines(), vec!["加载开始"]);

    app.block_on(futures::future::join(handle.dispose(), async {
        // 撤回已经发生，但加载仍在飞：惯性不允许打断它。
        assert_eq!(handle.state(), State::Loading, "转换不应被中途打断");
        tx.send(()).expect("接收端仍存活");
    }));

    assert_eq!(handle.state(), State::Inactive);
    assert_eq!(handle.error(), None, "过期不是失败");
    assert_eq!(
        log.lines(),
        vec!["加载开始", "撤 慢"],
        "过期那一步的逆同样被回卷，且不再往后执行"
    );
}
