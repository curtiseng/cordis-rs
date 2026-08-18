use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, Result, Steps};

use crate::host::Shell;

/// 本地插件：把工作区内的 shell 执行挂到 `shell` 键上（运行时可切换）。
pub struct BashSandbox {
    pub root: Rc<RefCell<PathBuf>>,
}

struct SharedShell {
    root: Rc<RefCell<PathBuf>>,
}

impl Shell for SharedShell {
    fn root(&self) -> PathBuf {
        self.root.borrow().clone()
    }
}

impl Component for BashSandbox {
    fn name(&self) -> &str {
        "bash-sandbox"
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let root = self.root.clone();
        Box::pin(async move {
            ctx.set::<crate::keys::ShellKey>(Rc::new(SharedShell { root }) as Rc<dyn Shell>);
            Ok(())
        })
    }
}
