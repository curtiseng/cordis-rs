use std::rc::Rc;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, Result, Steps, Value};

use crate::approval::{ApprovalPolicy, ApprovalQueue};

/// 本地插件：从配置加载创造模式审批策略。
pub struct ApprovalPolicyPlugin;

impl ApprovalPolicyPlugin {
    pub fn from_config(config: &Value, approvals: ApprovalQueue) -> Self {
        approvals.set_policy(ApprovalPolicy::from_config(config));
        ApprovalPolicyPlugin
    }
}

impl Component for ApprovalPolicyPlugin {
    fn name(&self) -> &str {
        "approval-policy"
    }

    fn apply(&self, _ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// 占位组件，仅用于把策略插件挂到 roster 上。
pub fn component(config: &Value, approvals: ApprovalQueue) -> Rc<dyn Component> {
    Rc::new(ApprovalPolicyPlugin::from_config(config, approvals))
}
