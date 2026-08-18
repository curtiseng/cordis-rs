use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::Value;
use spatiotemporal::{Patch, Result};

use crate::host::root_dir;
use crate::runtime::AgentRuntime;

#[derive(Clone, Debug, serde::Serialize)]
pub struct PendingInstall {
    pub id: String,
    pub kind: String,
    pub summary: String,
    pub preview: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_lines: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct ApprovalPolicy {
    pub require: Vec<String>,
    pub max_queue: usize,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        ApprovalPolicy {
            require: vec![
                "script".into(),
                "wasm".into(),
                "process".into(),
                "patch".into(),
            ],
            max_queue: 10,
        }
    }
}

impl ApprovalPolicy {
    pub fn from_config(config: &spatiotemporal::Value) -> Self {
        let mut policy = ApprovalPolicy::default();
        if let Some(items) = config.get("require").and_then(Value::as_array) {
            policy.require = items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect();
        }
        if let Some(max) = config.get("max_queue").and_then(Value::as_u64) {
            policy.max_queue = max.max(1) as usize;
        }
        policy
    }

    pub fn requires(&self, kind: &str) -> bool {
        self.require.iter().any(|item| item == kind)
    }
}

#[derive(Clone, serde::Serialize)]
struct AuditRecord {
    ts: u64,
    action: String,
    id: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

struct Pending {
    install: PendingInstall,
    session_id: String,
    layer: Vec<Patch>,
    cleanup_path: Option<PathBuf>,
}

#[derive(Clone)]
pub struct ApprovalQueue {
    policy: Rc<RefCell<ApprovalPolicy>>,
    pending: Rc<RefCell<Vec<Pending>>>,
    workspace: PathBuf,
}

impl ApprovalQueue {
    pub fn new(workspace: PathBuf) -> Self {
        ApprovalQueue {
            policy: Rc::new(RefCell::new(ApprovalPolicy::default())),
            pending: Rc::new(RefCell::new(Vec::new())),
            workspace,
        }
    }

    pub fn set_policy(&self, policy: ApprovalPolicy) {
        *self.policy.borrow_mut() = policy;
    }

    pub fn policy(&self) -> ApprovalPolicy {
        self.policy.borrow().clone()
    }

    pub fn requires(&self, kind: &str) -> bool {
        self.policy.borrow().requires(kind)
    }

    pub fn pending_all(&self) -> Vec<PendingInstall> {
        self.pending
            .borrow()
            .iter()
            .map(|item| item.install.clone())
            .collect()
    }

    #[allow(dead_code)]
    pub fn pending(&self) -> Option<PendingInstall> {
        self.pending_all().into_iter().next()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn propose_install(
        &self,
        session_id: String,
        id: String,
        kind: String,
        summary: String,
        preview: String,
        source_lines: Option<usize>,
        layer: Vec<Patch>,
        cleanup_path: Option<PathBuf>,
    ) -> Result<PendingInstall> {
        let max = self.policy.borrow().max_queue;
        let mut queue = self.pending.borrow_mut();
        if queue.len() >= max {
            return Err(spatiotemporal::Error::Component(format!(
                "审批队列已满（最多 {max} 条），请先处理现有请求"
            )));
        }
        if queue.iter().any(|item| item.install.id == id) {
            return Err(spatiotemporal::Error::Component(format!(
                "已存在 id 为 `{id}` 的待审批请求"
            )));
        }

        let install = PendingInstall {
            id: id.clone(),
            kind: kind.clone(),
            summary,
            preview,
            session_id: session_id.clone(),
            source_lines,
        };
        queue.push(Pending {
            install: install.clone(),
            session_id,
            layer,
            cleanup_path,
        });
        self.audit("propose", &id, &kind, None);
        Ok(install)
    }

    pub fn approve(
        &self,
        runtime: &AgentRuntime,
        workspace: &Path,
        id: Option<&str>,
    ) -> Result<String> {
        let index = self.find_pending(id)?;
        let pending = self.pending.borrow_mut().remove(index);
        runtime.activate_session(workspace, &pending.session_id)?;
        let applied = runtime.push_layer(pending.layer)?;
        self.audit("approve", &pending.install.id, &pending.install.kind, None);
        Ok(format!(
            "已批准并热装 `{}`（{}）\ncreated={:?} updated={:?}",
            pending.install.id, pending.install.summary, applied.created, applied.updated
        ))
    }

    pub fn reject(&self, id: Option<&str>, reason: Option<&str>) -> Result<String> {
        let index = self.find_pending(id)?;
        let pending = self.pending.borrow_mut().remove(index);
        if let Some(path) = pending.cleanup_path {
            let full = if path.is_absolute() {
                path
            } else {
                root_dir().join(path)
            };
            if full.exists() {
                std::fs::remove_file(&full).map_err(|error| {
                    spatiotemporal::Error::Component(format!("删不掉 {}：{error}", full.display()))
                })?;
            }
        }
        self.audit(
            "reject",
            &pending.install.id,
            &pending.install.kind,
            reason.map(str::to_owned),
        );
        let suffix = reason
            .filter(|text| !text.is_empty())
            .map(|text| format!("：{text}"))
            .unwrap_or_default();
        Ok(format!("已拒绝 `{}`{suffix}", pending.install.id))
    }

    fn find_pending(&self, id: Option<&str>) -> Result<usize> {
        let queue = self.pending.borrow();
        if queue.is_empty() {
            return Err(spatiotemporal::Error::Component("没有待审批的安装".into()));
        }
        match id {
            Some(id) => queue
                .iter()
                .position(|item| item.install.id == id)
                .ok_or_else(|| spatiotemporal::Error::Component(format!("找不到待审批项 `{id}`"))),
            None => Ok(0),
        }
    }

    fn audit(&self, action: &str, id: &str, kind: &str, reason: Option<String>) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let record = AuditRecord {
            ts,
            action: action.into(),
            id: id.into(),
            kind: kind.into(),
            reason,
        };
        let _ = append_audit(&self.workspace, &record);
    }
}

fn append_audit(workspace: &Path, record: &AuditRecord) -> Result<()> {
    let dir = workspace.join(".agent");
    std::fs::create_dir_all(&dir).map_err(map_io)?;
    let path = dir.join("approvals.jsonl");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(map_io)?;
    serde_json::to_writer(&mut file, record)
        .map_err(|error| spatiotemporal::Error::Component(format!("审批审计写入失败：{error}")))?;
    file.write_all(b"\n").map_err(map_io)?;
    Ok(())
}

fn map_io(error: std::io::Error) -> spatiotemporal::Error {
    spatiotemporal::Error::Component(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn policy_requires_configured_kinds() {
        let policy = ApprovalPolicy::from_config(&json!({
            "require": ["script", "patch"],
            "max_queue": 3
        }));
        assert!(policy.requires("script"));
        assert!(policy.requires("patch"));
        assert!(!policy.requires("wasm"));
        assert_eq!(policy.max_queue, 3);
    }
}
