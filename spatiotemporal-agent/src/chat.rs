use serde_json::Value;

use crate::host::{Llm, Toolbox};
use crate::runtime::AgentProfile;

#[derive(Clone, serde::Serialize)]
pub struct Trace {
    pub tool: String,
    pub substrate: String,
    pub input: String,
    pub output: String,
}

/// 单轮 agent 循环中的一步（思考或工具调用），供 UI 可视化。
#[derive(Clone, serde::Serialize)]
pub struct Step {
    pub kind: String,
    pub round: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub substrate: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub input: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output: String,
}

#[derive(serde::Serialize)]
pub struct Turn {
    pub reply: String,
    pub traces: Vec<Trace>,
    pub steps: Vec<Step>,
    pub session_id: Option<String>,
    /// 不含 system 的完整会话（含 tool 消息），供下一轮传入。
    pub history: Vec<Value>,
}

pub struct TurnRequest<'a> {
    pub llm: &'a dyn Llm,
    pub tools: &'a Toolbox,
    pub workspace: &'a str,
    pub doc_path: Option<&'a str>,
    pub profile: AgentProfile,
    pub user: &'a str,
    pub history: &'a [Value],
}

pub fn finish(messages: Vec<Value>, reply: String, traces: Vec<Trace>, steps: Vec<Step>) -> Turn {
    let history = messages.into_iter().skip(1).collect();
    Turn {
        reply,
        traces,
        steps,
        session_id: None,
        history,
    }
}

pub fn fallback_system_prompt(
    workspace: &str,
    doc_path: Option<&str>,
    profile: AgentProfile,
    tools: &Toolbox,
) -> String {
    let names: Vec<_> = tools.list().into_iter().map(|t| t.name).collect();
    let mut prompt = format!(
        "你是 spatiotemporal agent，运行在一个可组合插件宿主上。\n\
         工作区根目录：{workspace}\n\
         当前可用工具：{}\n",
        names.join(", ")
    );
    if let Some(doc_path) = doc_path
        && profile != AgentProfile::Coding
    {
        prompt.push_str(&format!(
            "当前文档：{doc_path}。可用 read_doc / outline / cite 阅读。\n"
        ));
    }
    prompt.push_str(
        "文件操作用 read / write / edit（JSON：path；write 还需 content；edit 还需 old、new）；\
         shell 用 bash（JSON：command，可选 cwd）；网络用 web_fetch（JSON：url）。\
         所有工具参数必须是合法 JSON 对象，字段名与 schema 一致。\n\
         回答用中文，引用时尽量附上原文或命令输出。不要编造没有依据的内容。\n",
    );
    if profile == AgentProfile::Creation {
        prompt.push_str(
            "【创造模式】可用 inspect_plugins / inspect_tools / inspect_config 查看运行时；\
             define_plugin / run_patch 按审批策略提交安装或试跑请求，必须等用户在浏览器点击「批准」；\
             revert_patch 撤销动态层；undefine_plugin 禁用；save_patch 持久化。\n",
        );
    }
    if profile == AgentProfile::Coding {
        prompt.push_str(
            "【编码模式】专注 read/write/edit/bash；同一文件少重复 read；改完用一条 bash 验证；\
             不要调用 demo 文档工具（outline/cite/stats/read_doc 已禁用）。\n",
        );
    }
    prompt
}
