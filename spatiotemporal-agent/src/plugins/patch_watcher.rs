use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use futures::future::LocalBoxFuture;
use notify::Watcher;
use spatiotemporal::{Component, Context, Result, Steps, Value};

use crate::host::root_dir;
use crate::runtime::AgentRuntime;

/// 监听 `cordis.patch.yml`，在文件变更时发出重载信号（由 Web 在请求边界消费）。
pub struct PatchWatcher {
    pub path: PathBuf,
    pub reload_tx: Rc<std::cell::RefCell<Option<Sender<()>>>>,
}

impl PatchWatcher {
    pub fn from_config(
        config: &Value,
        _runtime: Rc<AgentRuntime>,
        reload_tx: Rc<std::cell::RefCell<Option<Sender<()>>>>,
    ) -> Self {
        let path = config
            .get("path")
            .and_then(Value::as_str)
            .map(|path| {
                let candidate = Path::new(path);
                if candidate.is_absolute() {
                    candidate.to_path_buf()
                } else {
                    root_dir().join(candidate)
                }
            })
            .unwrap_or_else(|| root_dir().join("cordis.patch.yml"));
        PatchWatcher { path, reload_tx }
    }
}

impl Component for PatchWatcher {
    fn name(&self) -> &str {
        "patch-watcher"
    }

    fn apply(&self, _ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let path = self.path.clone();
        let reload_tx_slot = self.reload_tx.clone();
        Box::pin(async move {
            let tx = reload_tx_slot.borrow().clone().ok_or_else(|| {
                spatiotemporal::Error::Component("patch-watcher 缺少 reload 通道".into())
            })?;

            let watch_dir = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(root_dir);
            let file_name = path
                .file_name()
                .map(|name| name.to_owned())
                .unwrap_or_else(|| std::ffi::OsString::from("cordis.patch.yml"));

            thread::spawn(move || {
                let (event_tx, event_rx) = mpsc::channel();
                let mut watcher = match notify::recommended_watcher(
                    move |result: notify::Result<notify::Event>| {
                        if result.is_ok() {
                            let _ = event_tx.send(());
                        }
                    },
                ) {
                    Ok(watcher) => watcher,
                    Err(error) => {
                        eprintln!("patch-watcher 启动失败：{error}");
                        return;
                    }
                };
                if let Err(error) = watcher.watch(&watch_dir, notify::RecursiveMode::NonRecursive) {
                    eprintln!("patch-watcher 监听 {} 失败：{error}", watch_dir.display());
                    return;
                }
                eprintln!(
                    "patch-watcher 监听 {}（文件 {}）",
                    watch_dir.display(),
                    file_name.to_string_lossy()
                );
                while event_rx.recv().is_ok() {
                    while event_rx.recv_timeout(Duration::from_millis(120)).is_ok() {}
                    let _ = tx.send(());
                }
            });
            Ok(())
        })
    }
}

/// 若收到 watcher 信号则重载 patch 文件。
pub fn drain_reload(runtime: &AgentRuntime, path: &Path, reload_rx: &mpsc::Receiver<()>) {
    while reload_rx.try_recv().is_ok() {
        match runtime.load_patch_file(path) {
            Ok(applied) if !applied.is_noop() => {
                eprintln!(
                    "patch-watcher 已重载 {}：created={:?} updated={:?} removed={:?}",
                    path.display(),
                    applied.created,
                    applied.updated,
                    applied.removed
                );
            }
            Ok(_) => eprintln!("patch-watcher 已重载 {}（无变化）", path.display()),
            Err(error) => eprintln!("patch-watcher 重载失败：{error}"),
        }
    }
}

pub fn reload_patch_file(runtime: &AgentRuntime, path: &Path) -> Result<String> {
    let applied = runtime.load_patch_file(path)?;
    Ok(format!(
        "已重载 {}\ncreated={:?} updated={:?} removed={:?}",
        path.display(),
        applied.created,
        applied.updated,
        applied.removed
    ))
}
