use serde_json::{Value, json};

use crate::host::{Llm, Toolbox};

const MAX_TOOL_ROUNDS: usize = 12;

#[derive(Clone, serde::Serialize)]
pub struct Trace {
    pub tool: String,
    pub substrate: String,
    pub input: String,
    pub output: String,
}

#[derive(serde::Serialize)]
pub struct Turn {
    pub reply: String,
    pub traces: Vec<Trace>,
    /// 不含 system 的完整会话（含 tool 消息），供下一轮传入。
    pub history: Vec<Value>,
}

pub struct ChatConfig<'a> {
    pub llm: &'a dyn Llm,
    pub tools: &'a Toolbox,
    pub workspace: &'a str,
    pub doc_path: Option<&'a str>,
    pub creation: bool,
    pub user: &'a str,
    pub history: &'a [Value],
}

pub fn run(config: ChatConfig<'_>) -> Turn {
    let ChatConfig {
        llm,
        tools,
        workspace,
        doc_path,
        creation,
        user,
        history,
    } = config;

    let mut messages = vec![json!({
        "role": "system",
        "content": system_prompt(workspace, doc_path, creation, tools),
    })];
    messages.extend(history.iter().cloned());
    messages.push(json!({"role": "user", "content": user}));

    let schemas = tools.schemas();
    let mut traces = Vec::new();

    for _ in 0..MAX_TOOL_ROUNDS {
        let mut body = json!({
            "model": llm.model(),
            "messages": messages,
        });
        if !schemas.is_empty() {
            body["tools"] = json!(schemas);
            body["tool_choice"] = json!("auto");
        }

        let response = match llm.complete(body) {
            Ok(value) => value,
            Err(error) => {
                return finish(messages, error.to_string(), traces);
            }
        };

        let message = response["choices"][0]["message"].clone();
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array)
            && !calls.is_empty()
        {
            messages.push(message.clone());
            for call in calls {
                let name = call["function"]["name"].as_str().unwrap_or("").to_owned();
                let arguments = call["function"]["arguments"]
                    .as_str()
                    .unwrap_or("{}")
                    .to_owned();
                let id = call["id"].as_str().unwrap_or("").to_owned();
                let substrate = tools
                    .list()
                    .into_iter()
                    .find(|t| t.name == name)
                    .map(|t| t.substrate)
                    .unwrap_or_else(|| "?".into());
                let output = match tools.call(&name, &arguments) {
                    Ok(text) => text,
                    Err(error) => error.to_string(),
                };
                traces.push(Trace {
                    tool: name.clone(),
                    substrate,
                    input: arguments.clone(),
                    output: output.clone(),
                });
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": output,
                }));
            }
            continue;
        }

        let reply = message["content"]
            .as_str()
            .unwrap_or("（模型没有返回文本）")
            .to_owned();
        messages.push(message);
        return finish(messages, reply, traces);
    }

    finish(messages, "工具调用轮次用尽了。".into(), traces)
}

fn finish(messages: Vec<Value>, reply: String, traces: Vec<Trace>) -> Turn {
    let history = messages.into_iter().skip(1).collect();
    Turn {
        reply,
        traces,
        history,
    }
}

fn system_prompt(workspace: &str, doc_path: Option<&str>, creation: bool, tools: &Toolbox) -> String {
    let names: Vec<_> = tools.list().into_iter().map(|t| t.name).collect();
    let mut prompt = format!(
        "你是 spatiotemporal agent，运行在一个可组合插件宿主上。\n\
         工作区根目录：{workspace}\n\
         当前可用工具：{}\n",
        names.join(", ")
    );
    if let Some(doc_path) = doc_path {
        prompt.push_str(&format!(
            "当前文档：{doc_path}。可用 read_doc / outline / cite 阅读。\n"
        ));
    }
    prompt.push_str(
        "文件操作用 read / write / edit（JSON：path；write 还需 content；edit 还需 old、new）；\
         shell 用 bash（JSON：command，可选 cwd）。\
         所有工具参数必须是合法 JSON 对象，字段名与 schema 一致。\n\
         回答用中文，引用时尽量附上原文或命令输出。不要编造没有依据的内容。\n",
    );
    if creation {
        prompt.push_str(
            "【创造模式】可用 inspect_plugins / inspect_tools / inspect_config 查看运行时；\
             define_script 只会提交安装请求，必须等用户在浏览器点击「批准」后才会热装；\
             undefine_plugin 禁用；save_patch 持久化。\n",
        );
    }
    prompt
}
