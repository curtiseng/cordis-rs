use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use spatiotemporal::{Patch, Result};

use crate::host::root_dir;
use crate::runtime::AgentRuntime;

#[derive(Clone, serde::Serialize)]
pub struct PendingInstall {
    pub id: String,
    pub kind: String,
    pub summary: String,
    pub preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_lines: Option<usize>,
}

struct Pending {
    install: PendingInstall,
    layer: Vec<Patch>,
    cleanup_path: Option<PathBuf>,
}

#[derive(Clone, Default)]
pub struct ApprovalQueue {
    inner: Rc<RefCell<Option<Pending>>>,
}

impl ApprovalQueue {
    pub fn new() -> Self {
        ApprovalQueue::default()
    }

    pub fn pending(&self) -> Option<PendingInstall> {
        self.inner.borrow().as_ref().map(|p| p.install.clone())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn propose_install(
        &self,
        id: String,
        kind: String,
        summary: String,
        preview: String,
        source_lines: Option<usize>,
        layer: Vec<Patch>,
        cleanup_path: Option<PathBuf>,
    ) -> Result<PendingInstall> {
        if self.inner.borrow().is_some() {
            return Err(spatiotemporal::Error::Component(
                "已有一条待审批的安装请求，请先处理".into(),
            ));
        }

        let install = PendingInstall {
            id: id.clone(),
            kind,
            summary,
            preview,
            source_lines,
        };
        *self.inner.borrow_mut() = Some(Pending {
            install: install.clone(),
            layer,
            cleanup_path,
        });
        Ok(install)
    }

    pub fn approve(&self, runtime: &AgentRuntime) -> Result<String> {
        let pending = self
            .inner
            .borrow_mut()
            .take()
            .ok_or_else(|| spatiotemporal::Error::Component("没有待审批的安装".into()))?;
        let applied = runtime.push_layer(pending.layer)?;
        Ok(format!(
            "已批准并热装 `{}`（{}）\ncreated={:?} updated={:?}",
            pending.install.id, pending.install.summary, applied.created, applied.updated
        ))
    }

    pub fn reject(&self) -> Result<String> {
        let pending = self
            .inner
            .borrow_mut()
            .take()
            .ok_or_else(|| spatiotemporal::Error::Component("没有待审批的安装".into()))?;
        if let Some(path) = pending.cleanup_path {
            let full = if path.is_absolute() {
                path
            } else {
                root_dir().join(path)
            };
            if full.exists() {
                fs::remove_file(&full).map_err(|error| {
                    spatiotemporal::Error::Component(format!("删不掉 {}：{error}", full.display()))
                })?;
            }
        }
        Ok(format!("已拒绝 `{}`", pending.install.id))
    }
}
