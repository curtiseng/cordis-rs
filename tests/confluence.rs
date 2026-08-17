//! 合流性：装配顺序不影响静止时的最终状态。
//!
//! 对应论文定理 68。这条定理是「配置文件的行序不携带加载语义」的依据——
//! 也是 deepseek-harness 敢让用户随便 patch 那 78 行插件的原因。

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

/// 一次装配的结果：三个组件的终态，加上它们各自解析到的东西。
#[derive(PartialEq, Eq, Debug)]
struct Outcome {
    states: Vec<State>,
    resolved: Vec<String>,
}

/// 按给定顺序装配同一组三个组件（p 提供 Db，m 消费 Db 并提供 Repo，c 消费 Repo）。
fn assemble(order: [usize; 3]) -> Outcome {
    let log = Log::new();
    let mut app = App::new();
    let root = app.root();

    let make = |index: usize| -> Rc<dyn cordis::Component> {
        let log = log.clone();
        match index {
            0 => Probe::new("p", move |ctx, _steps| {
                Box::pin(async move {
                    ctx.set::<Db>(Rc::new(Tagged("pg")));
                    Ok(())
                })
            }),
            1 => Probe::needs("m", vec![KeyId::of::<Db>()], move |ctx, _steps| {
                let log = log.clone();
                Box::pin(async move {
                    let db = ctx.resolve::<Db>()?;
                    log.push(format!("m 解析到 {}", db.tag()));
                    ctx.set::<Repo>(Rc::new(Tagged("repo-on-pg")));
                    Ok(())
                })
            }),
            _ => Probe::needs("c", vec![KeyId::of::<Repo>()], move |ctx, _steps| {
                let log = log.clone();
                Box::pin(async move {
                    log.push(format!("c 解析到 {}", ctx.resolve::<Repo>()?.tag()));
                    Ok(())
                })
            }),
        }
    };

    // 三个句柄按组件编号归位，这样不同装配顺序下的断言仍可比较。
    let mut handles: Vec<Option<cordis::FiberHandle>> = vec![None, None, None];
    for index in order {
        handles[index] = Some(root.use_component(make(index)));
        // 每插一行就推进到静止，模拟「一次加一个插件」；
        // 顺序不同时中间态各异，最终态应当一致。
        app.settle();
    }

    let mut resolved = log.lines();
    resolved.sort();
    Outcome {
        states: handles
            .iter()
            .map(|handle| handle.as_ref().expect("三个都已装配").state())
            .collect(),
        resolved,
    }
}

/// 六种插入顺序都收敛到同一个静止态。
#[test]
fn insertion_order_does_not_affect_the_end_state() {
    let permutations = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    let expected = Outcome {
        states: vec![State::Active, State::Active, State::Active],
        resolved: vec!["c 解析到 repo-on-pg".to_string(), "m 解析到 pg".to_string()],
    };

    for order in permutations {
        assert_eq!(
            assemble(order),
            expected,
            "装配顺序 {order:?} 应当收敛到同一稳定态"
        );
    }
}

/// 一次性把三个都插进去、只在最后推进一次，结果同样一致。
///
/// 这一条比上面更强：中间完全没有静止点，所有转换交错在一起。
#[test]
fn interleaved_assembly_converges_too() {
    let log = Log::new();
    let mut app = App::new();
    let root = app.root();

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
    let middle = Probe::needs("m", vec![KeyId::of::<Db>()], move |ctx, _steps| {
        Box::pin(async move {
            let db = ctx.resolve::<Db>()?;
            ctx.set::<Repo>(Rc::new(Tagged(db.tag())));
            Ok(())
        })
    });
    let base = Probe::new("p", move |ctx, _steps| {
        Box::pin(async move {
            ctx.set::<Db>(Rc::new(Tagged("pg")));
            Ok(())
        })
    });

    // 倒序插入，且中途一次都不推进。
    let leaf_handle = root.use_component(leaf);
    let middle_handle = root.use_component(middle);
    let base_handle = root.use_component(base);
    app.settle();

    assert_eq!(base_handle.state(), State::Active);
    assert_eq!(middle_handle.state(), State::Active);
    assert_eq!(leaf_handle.state(), State::Active);
    assert_eq!(log.lines(), vec!["c 装在 pg"]);
}

/// 依赖环：涉及的组件永久保持非活动，且不报错、不死锁。
///
/// 论文 6.5 节说这一状况仅从依赖声明就可预测，与并发死锁不同。
#[test]
fn dependency_cycle_leaves_both_inactive() {
    let mut app = App::new();
    let root = app.root();

    let a = Probe::needs("a", vec![KeyId::of::<Repo>()], move |ctx, _steps| {
        Box::pin(async move {
            let _ = ctx.resolve::<Repo>()?;
            ctx.set::<Db>(Rc::new(Tagged("db-from-a")));
            Ok(())
        })
    });
    let b = Probe::needs("b", vec![KeyId::of::<Db>()], move |ctx, _steps| {
        Box::pin(async move {
            let _ = ctx.resolve::<Db>()?;
            ctx.set::<Repo>(Rc::new(Tagged("repo-from-b")));
            Ok(())
        })
    });

    let a_handle = root.use_component(a);
    let b_handle = root.use_component(b);
    app.settle();

    assert_eq!(a_handle.state(), State::Inactive);
    assert_eq!(b_handle.state(), State::Inactive);
    assert_eq!(a_handle.error(), None, "环不是错误，只是永不被满足");
    assert_eq!(b_handle.error(), None);
}
