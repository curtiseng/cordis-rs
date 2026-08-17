use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

use spatiotemporal::{Patch, Result};

use crate::host::root_dir;
use crate::runtime::AgentRuntime;

#[derive(Clone, serde::Serialize)]
pub struct PendingScript {
    pub id: String,
    pub file: String,
    pub role: String,
    pub source_lines: usize,
    pub preview: String,
}

struct Pending {
    script: PendingScript,
    layer: Vec<Patch>,
    rel_path: String,
}

#[derive(Clone, Default)]
pub struct ApprovalQueue {
    inner: Rc<RefCell<Option<Pending>>>,
}

impl ApprovalQueue {
    pub fn new() -> Self {
        ApprovalQueue::default()
    }

    pub fn pending(&self) -> Option<PendingScript> {
        self.inner.borrow().as_ref().map(|p| p.script.clone())
    }

    pub fn propose(
        &self,
        id: String,
        rel_path: String,
        role: String,
        source: String,
        layer: Vec<Patch>,
    ) -> Result<PendingScript> {
        if self.inner.borrow().is_some() {
            return Err(spatiotemporal::Error::Component(
                "已有一条待审批的安装请求，请先处理".into(),
            ));
        }

        let preview: String = source.chars().take(800).collect();
        let script = PendingScript {
            id: id.clone(),
            file: rel_path.clone(),
            role,
            source_lines: source.lines().count(),
            preview,
        };
        *self.inner.borrow_mut() = Some(Pending {
            script: script.clone(),
            layer,
            rel_path,
        });
        Ok(script)
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
            pending.script.id, pending.script.file, applied.created, applied.updated
        ))
    }

    pub fn reject(&self) -> Result<String> {
        let pending = self
            .inner
            .borrow_mut()
            .take()
            .ok_or_else(|| spatiotemporal::Error::Component("没有待审批的安装".into()))?;
        let path = root_dir().join(&pending.rel_path);
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                spatiotemporal::Error::Component(format!("删不掉 {}：{error}", path.display()))
            })?;
        }
        Ok(format!("已拒绝 `{}`，临时文件已删除", pending.script.id))
    }
}
