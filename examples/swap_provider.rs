//! 运行中换掉一项能力的提供者，消费者自己跟着搬。
//!
//! ```text
//! cargo run --example swap_provider
//! ```
//!
//! 这个例子对应论文 6.2 节的排他绑定：同一个键先由本地实现满足，
//! 再换成远端实现。消费者代码一个字没改，也不知道对方换了——它只声明了
//! 「我需要 storage」，其余由响应式 coeffect 完成。

use std::rc::Rc;

use cordis::{App, Component, Context, Key, KeyId, Result, State, Steps};
use futures::future::LocalBoxFuture;

/// 一项能力：存储。
trait Storage {
    fn place(&self) -> &'static str;
}

enum StorageKey {}
impl Key for StorageKey {
    type Api = dyn Storage;
    const NAME: &'static str = "storage";
}

struct At(&'static str);
impl Storage for At {
    fn place(&self) -> &'static str {
        self.0
    }
}

/// 提供者：安装一个实现，卸载时自动摘掉。
struct Provider(&'static str);

impl Component for Provider {
    fn name(&self) -> &'static str {
        "provider"
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let place = self.0;
        Box::pin(async move {
            println!("  [provider] 装上 {place}");
            ctx.set::<StorageKey>(Rc::new(At(place)));
            Ok(())
        })
    }
}

/// 消费者：只声明依赖，不关心是谁在提供。
struct Worker;

impl Component for Worker {
    fn name(&self) -> &'static str {
        "worker"
    }

    fn inject(&self) -> Vec<KeyId> {
        vec![KeyId::of::<StorageKey>()]
    }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let place = ctx.resolve::<StorageKey>()?.place();
            println!("  [worker]   我现在跑在 {place} 上");
            steps.step(move || async move {
                println!("  [worker]   离开 {place}");
            })?;
            Ok(())
        })
    }
}

fn main() {
    let mut app = App::new();
    let root = app.root();

    println!("1. 只挂 worker：依赖还不在，它保持非活动且不报错");
    let worker = root.use_component(Rc::new(Worker));
    app.settle();
    println!("   worker = {:?}\n", worker.state());
    assert_eq!(worker.state(), State::Inactive);

    println!("2. 装上本地存储");
    let local = root.use_component(Rc::new(Provider("本机磁盘")));
    app.settle();
    println!("   worker = {:?}\n", worker.state());

    println!("3. 换成远端存储：先撤回旧的，再装新的");
    app.block_on(local.dispose());
    let _remote = root.use_component(Rc::new(Provider("远端沙箱")));
    app.settle();
    println!("   worker = {:?}", worker.state());
    println!(
        "   worker 身上挂着 {} 个待回卷的逆\n",
        worker.tracked_effects()
    );

    println!("4. 撤掉 worker 自己");
    app.block_on(worker.dispose());
    println!("   worker = {:?}", worker.state());
    println!("   待回卷的逆 = {}", worker.tracked_effects());
}
