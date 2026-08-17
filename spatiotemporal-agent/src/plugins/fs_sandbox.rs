use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, Result, Steps, Value};

use crate::host::Fs;
use crate::util::workspace_root;

/// 本地插件：把工作区根目录挂到 `fs` 键上。
pub struct FsSandbox {
    pub root: PathBuf,
}

impl FsSandbox {
    pub fn from_config(config: &Value) -> Self {
        FsSandbox {
            root: workspace_root(config),
        }
    }
}

struct LocalFs {
    root: PathBuf,
}

impl Fs for LocalFs {
    fn root(&self) -> &Path {
        &self.root
    }
}

impl Component for FsSandbox {
    fn name(&self) -> &str {
        "fs-sandbox"
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let root = self.root.clone();
        Box::pin(async move {
            ctx.set::<crate::keys::FsKey>(Rc::new(LocalFs { root }) as Rc<dyn Fs>);
            Ok(())
        })
    }
}
