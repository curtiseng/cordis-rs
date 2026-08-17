use serde_json::{Value, json};

use crate::plugins::{Llm, Toolbox};

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
}

pub fn run(llm: &dyn Llm, tools: &Toolbox, doc_path: &str, user: &str, history: &[Value]) -> Turn {
    let mut messages = vec![json!({
        "role": "system",
        "content": format!(
            "你是这篇 Markdown 文档的讲解员。文档路径：{doc_path}。\n\
             先用工具看文档再回答：read_doc 读正文，outline 看结构，cite 按关键词引用原文。\n\
             回答用中文，引用时尽量附上原文片段。不要编造文档里没有的内容。"
        )
    })];
    messages.extend(history.iter().cloned());
    messages.push(json!({"role": "user", "content": user}));

    let schemas = tools.schemas();
    let mut traces = Vec::new();

    for _ in 0..6 {
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
                return Turn {
                    reply: error.to_string(),
                    traces,
                };
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
        return Turn { reply, traces };
    }

    Turn {
        reply: "工具调用轮次用尽了。".into(),
        traces,
    }
}
