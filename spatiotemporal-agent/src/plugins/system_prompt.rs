use std::fs;
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, KeyId, Result, Steps, Value};

use crate::host::{PromptInput, SystemPrompt};
use crate::keys::{FsKey, PromptKey};
use crate::runtime::AgentProfile;
use serde_json::Value as JsonValue;

/// 本地插件：从 AGENTS.md 与工具 schema 组装 system prompt。
pub struct SystemPromptPlugin {
    pub agents_file: String,
}

impl SystemPromptPlugin {
    pub fn from_config(config: &Value) -> Self {
        SystemPromptPlugin {
            agents_file: config
                .get("agents_file")
                .and_then(Value::as_str)
                .unwrap_or("AGENTS.md")
                .to_owned(),
        }
    }
}

struct PromptService {
    agents_file: String,
}

impl SystemPrompt for PromptService {
    fn build(&self, input: PromptInput<'_>) -> String {
        let mut sections = Vec::new();

        sections.push(format!(
            "你是 spatiotemporal agent，一个可组合插件宿主上的通用助手。\n\
             工作区根目录：{}\n\
             优先用工具收集事实，再回答；不要编造没有依据的内容。\n\
             回答用中文。",
            input.workspace
        ));

        if let Some(doc_path) = input.doc_path
            && input.profile != AgentProfile::Coding
        {
            sections.push(format!(
                "markdown 快照（仅 outline / cite / stats / read_doc 使用）：{doc_path}。\n\
                 读工作区其它文件、改代码、跑命令：直接用 read / write / edit / bash，不要假设只有这一篇文件。"
            ));
        }

        if input.profile == AgentProfile::Coding {
            let coding_path = crate::host::root_dir().join("assets/CODING.prompt.md");
            if coding_path.exists()
                && let Ok(text) = fs::read_to_string(&coding_path)
                && !text.trim().is_empty()
            {
                sections.push(text.trim().to_owned());
            } else {
                sections.push(coding_mode_fallback());
            }
        }

        let agents_path = std::path::Path::new(input.workspace).join(&self.agents_file);
        if agents_path.exists()
            && let Ok(text) = fs::read_to_string(&agents_path)
        {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                sections.push(format!("## 项目指令（{}）\n{trimmed}", self.agents_file));
            }
        }

        sections.push(tool_guide(input.tools));
        sections.push(substrate_guide());
        sections.push(
            "安全：不要执行明显破坏性的 bash 命令，除非用户明确要求。\n\
             文件操作用 read / write / edit；shell 用 bash；网络用 web_fetch。"
                .into(),
        );

        if input.profile == AgentProfile::Creation {
            sections.push(
                "【创造模式】inspect_* 查看运行时；define_script 提交安装需用户界面批准；\
                 undefine_plugin 禁用；save_patch 持久化。"
                    .into(),
            );
        }

        sections.join("\n\n")
    }
}

fn coding_mode_fallback() -> String {
    "【编码模式】专注 read/write/edit/bash 完成代码任务；同一文件少重复 read；\
     改完用一条 bash 验证；标准/demo 文档类 tool 已禁用；\
     不要 grant fs 给 script/wasm，IO 直接用 native tool。"
        .into()
}

fn substrate_guide() -> String {
    "## 基质与 IO\n\
     - **native**（read/write/edit/bash/web_fetch）：文件与命令走这里；编码任务优先这些 tool。\n\
     - **script/wasm 叶子**：通常只 grant 宿主授予的能力快照（如 markdown）；**不要** grant fs/shell。\n\
     - 叶子内编排其它 tool：script 用 `host.callTool(name, argsJson)`，wasm 用 WIT `call-tool`；\
       与 LLM 调 tool 同路径，**不要**为此写 native 插件。\n\
     - history 过长或 LLM 报 tool 消息格式错误：建议**新开会话**。"
        .into()
}

fn tool_guide(tools: &crate::host::Toolbox) -> String {
    let mut out = String::from("## 工具与 JSON 参数\n");
    for tool in tools.list() {
        let schema = tools
            .schemas()
            .into_iter()
            .find(|entry| entry["function"]["name"].as_str() == Some(&tool.name));
        let params: JsonValue = schema
            .as_ref()
            .and_then(|entry| entry["function"].get("parameters"))
            .cloned()
            .unwrap_or_else(crate::tool_schema::empty_object);
        out.push_str(&format!(
            "- **{}** [{}]：{}\n  parameters: {}\n",
            tool.name,
            tool.substrate,
            tool.description,
            serde_json::to_string(&params).unwrap_or_else(|_| "{}".into())
        ));
    }
    out
}

impl Component for SystemPromptPlugin {
    fn name(&self) -> &str {
        "system-prompt"
    }

    fn inject(&self) -> Vec<KeyId> {
        vec![KeyId::of::<FsKey>()]
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let agents_file = self.agents_file.clone();
        Box::pin(async move {
            let _fs = ctx.resolve::<FsKey>()?;
            let service: Rc<dyn SystemPrompt> = Rc::new(PromptService { agents_file });
            ctx.set::<PromptKey>(service);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    #[test]
    fn fallback_prompt_lists_tools() {
        let tools = crate::host::Toolbox::new();
        tools.insert(
            "read".into(),
            "读文件".into(),
            "native",
            Rc::new(|_| Ok(String::new())),
        );
        let service = PromptService {
            agents_file: "AGENTS.md".into(),
        };
        let text = service.build(PromptInput {
            workspace: "/tmp",
            doc_path: None,
            profile: AgentProfile::Standard,
            tools: &tools,
        });
        assert!(text.contains("read"));
        assert!(text.contains("parameters"));
    }
}
