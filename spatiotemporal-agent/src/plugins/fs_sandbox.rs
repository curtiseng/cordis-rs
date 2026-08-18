use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, Result, Steps};

use crate::host::Fs;

/// 本地插件：把工作区根目录挂到 `fs` 键上（运行时可切换）。
pub struct FsSandbox {
    pub root: Rc<RefCell<PathBuf>>,
}

struct SharedFs {
    root: Rc<RefCell<PathBuf>>,
}

impl Fs for SharedFs {
    fn root(&self) -> PathBuf {
        self.root.borrow().clone()
    }
}

impl Component for FsSandbox {
    fn name(&self) -> &str {
        "fs-sandbox"
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let root = self.root.clone();
        Box::pin(async move {
            ctx.set::<crate::keys::FsKey>(Rc::new(SharedFs { root }) as Rc<dyn Fs>);
            Ok(())
        })
    }
}
