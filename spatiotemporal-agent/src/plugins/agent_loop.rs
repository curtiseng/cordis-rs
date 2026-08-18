use std::rc::Rc;

use futures::future::LocalBoxFuture;
use serde_json::{Value, json};
use spatiotemporal::{Component, Context, KeyId, Result, Steps, Value as StValue};

use crate::chat::{Turn, TurnRequest, finish};
use crate::compaction::{CompactionConfig, compact, repair_tool_messages};
use crate::host::{AgentLoop, SystemPrompt};
use crate::keys::{AgentLoopKey, PromptKey};

/// 本地插件：默认 agent 循环（多轮 tool call）。
pub struct AgentLoopPlugin {
    pub max_rounds: usize,
    pub compaction: CompactionConfig,
}

impl AgentLoopPlugin {
    pub fn from_config(config: &StValue) -> Self {
        AgentLoopPlugin {
            max_rounds: config
                .get("max_rounds")
                .and_then(StValue::as_u64)
                .unwrap_or(50) as usize,
            compaction: CompactionConfig::from_config(config),
        }
    }
}

struct DefaultAgentLoop {
    max_rounds: usize,
    compaction: CompactionConfig,
    prompt: Option<Rc<dyn SystemPrompt>>,
}

impl AgentLoop for DefaultAgentLoop {
    fn run_turn(&self, request: TurnRequest<'_>) -> Turn {
        run_loop(
            request,
            self.max_rounds,
            &self.compaction,
            self.prompt.as_deref(),
        )
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
        let compaction = self.compaction.clone();
        Box::pin(async move {
            let prompt = ctx.lookup::<PromptKey>();
            let service: Rc<dyn AgentLoop> = Rc::new(DefaultAgentLoop {
                max_rounds,
                compaction,
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
    compaction: &CompactionConfig,
    prompt: Option<&dyn SystemPrompt>,
) -> Turn {
    let TurnRequest {
        llm,
        tools,
        workspace,
        doc_path,
        profile,
        user,
        history,
    } = request;

    let system = if let Some(builder) = prompt {
        builder.build(crate::host::PromptInput {
            workspace,
            doc_path,
            profile,
            tools,
        })
    } else {
        crate::chat::fallback_system_prompt(workspace, doc_path, profile, tools)
    };

    let mut messages = vec![json!({ "role": "system", "content": system })];
    let (history, _report) = compact(history, compaction);
    messages.extend(history.iter().cloned());
    messages.push(json!({ "role": "user", "content": user }));
    let new_from = messages.len();

    let schemas = tools.schemas();
    let mut traces = Vec::new();
    let mut steps = Vec::new();

    for (round_idx, _) in (0..max_rounds).enumerate() {
        let round = round_idx + 1;
        let mut body = json!({
            "model": llm.model(),
            "messages": sanitize_messages(&messages),
        });
        if !schemas.is_empty() {
            body["tools"] = json!(schemas);
            body["tool_choice"] = json!("auto");
        }

        let response = match llm.complete(body) {
            Ok(value) => value,
            Err(error) => {
                return finish(messages, error.to_string(), traces, steps, new_from);
            }
        };

        let message = response["choices"][0]["message"].clone();

        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array)
            && !calls.is_empty()
        {
            let think = message
                .get("reasoning_content")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    message
                        .get("content")
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                        .map(str::to_owned)
                });
            if let Some(text) = think {
                steps.push(crate::chat::Step {
                    kind: "think".into(),
                    round,
                    text,
                    tool: String::new(),
                    substrate: String::new(),
                    input: String::new(),
                    output: String::new(),
                });
            }
            messages.push(sanitize_message(&message));
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
                    substrate: substrate.clone(),
                    input: arguments.clone(),
                    output: output.clone(),
                });
                steps.push(crate::chat::Step {
                    kind: "tool".into(),
                    round,
                    text: String::new(),
                    tool: name.clone(),
                    substrate: substrate.clone(),
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
        messages.push(sanitize_message(&message));
        return finish(messages, reply, traces, steps, new_from);
    }

    finish(
        messages,
        "工具调用轮次用尽了。".into(),
        traces,
        steps,
        new_from,
    )
}

/// DeepSeek 不接受回传 `reasoning_content` 与 tool_calls 里的 `index`。
fn sanitize_message(message: &Value) -> Value {
    let mut out = message.clone();
    let Some(obj) = out.as_object_mut() else {
        return out;
    };
    obj.remove("reasoning_content");
    if let Some(calls) = obj.get_mut("tool_calls").and_then(Value::as_array_mut) {
        for call in calls {
            if let Some(call_obj) = call.as_object_mut() {
                call_obj.remove("index");
            }
        }
    }
    out
}

fn sanitize_messages(messages: &[Value]) -> Vec<Value> {
    repair_tool_messages(messages)
        .iter()
        .map(sanitize_message)
        .collect()
}
