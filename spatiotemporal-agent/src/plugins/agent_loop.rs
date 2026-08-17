use std::rc::Rc;

use futures::future::LocalBoxFuture;
use serde_json::{Value, json};
use spatiotemporal::{Component, Context, KeyId, Result, Steps, Value as StValue};

use crate::chat::{Turn, TurnRequest, finish};
use crate::host::{AgentLoop, SystemPrompt};
use crate::keys::{AgentLoopKey, PromptKey};

/// 本地插件：默认 agent 循环（多轮 tool call）。
pub struct AgentLoopPlugin {
    pub max_rounds: usize,
}

impl AgentLoopPlugin {
    pub fn from_config(config: &StValue) -> Self {
        AgentLoopPlugin {
            max_rounds: config
                .get("max_rounds")
                .and_then(StValue::as_u64)
                .unwrap_or(12) as usize,
        }
    }
}

struct DefaultAgentLoop {
    max_rounds: usize,
    prompt: Option<Rc<dyn SystemPrompt>>,
}

impl AgentLoop for DefaultAgentLoop {
    fn run_turn(&self, request: TurnRequest<'_>) -> Turn {
        run_loop(request, self.max_rounds, self.prompt.as_deref())
    }
}

impl Component for AgentLoopPlugin {
    fn name(&self) -> &str {
        "agent-loop"
    }

    fn inject(&self) -> Vec<KeyId> {
        vec![KeyId::of::<PromptKey>()]
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let max_rounds = self.max_rounds;
        Box::pin(async move {
            let prompt = ctx.lookup::<PromptKey>();
            let service: Rc<dyn AgentLoop> = Rc::new(DefaultAgentLoop {
                max_rounds,
                prompt,
            });
            ctx.set::<AgentLoopKey>(service);
            Ok(())
        })
    }
}

fn run_loop(
    request: TurnRequest<'_>,
    max_rounds: usize,
    prompt: Option<&dyn SystemPrompt>,
) -> Turn {
    let TurnRequest {
        llm,
        tools,
        workspace,
        doc_path,
        creation,
        user,
        history,
    } = request;

    let system = if let Some(builder) = prompt {
        builder.build(crate::host::PromptInput {
            workspace,
            doc_path,
            creation,
            tools,
        })
    } else {
        crate::chat::fallback_system_prompt(workspace, doc_path, creation, tools)
    };

    let mut messages = vec![json!({ "role": "system", "content": system })];
    messages.extend(history.iter().cloned());
    messages.push(json!({ "role": "user", "content": user }));

    let schemas = tools.schemas();
    let mut traces = Vec::new();

    for _ in 0..max_rounds {
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
                traces.push(crate::chat::Trace {
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
