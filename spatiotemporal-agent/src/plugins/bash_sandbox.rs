use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, Result, Steps, Value};

use crate::host::Shell;
use crate::util::workspace_root;

/// 本地插件：把工作区内的 shell 执行挂到 `shell` 键上。
pub struct BashSandbox {
    pub root: PathBuf,
}

impl BashSandbox {
    pub fn from_config(config: &Value) -> Self {
        BashSandbox {
            root: workspace_root(config),
        }
    }
}

struct LocalShell {
    root: PathBuf,
}

impl Shell for LocalShell {
    fn root(&self) -> &Path {
        &self.root
    }
}

impl Component for BashSandbox {
    fn name(&self) -> &str {
        "bash-sandbox"
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let root = self.root.clone();
        Box::pin(async move {
            ctx.set::<crate::keys::ShellKey>(Rc::new(LocalShell { root }) as Rc<dyn Shell>);
            Ok(())
        })
    }
}
