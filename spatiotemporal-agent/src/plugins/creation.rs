use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use serde_json::json;
use spatiotemporal::{Component, Context, Entry, Patch, Result, Steps, Value};

use crate::approval::ApprovalQueue;
use crate::host::{Roster, Toolbox, root_dir};
use crate::runtime::AgentRuntime;
use crate::tool_schema;
use crate::util::{arg_str, parse_json_args};

/// 创造模式元工具：检视运行时、提交脚本安装（需人工审批）、持久化 patch。
pub struct CreationTools {
    pub tools: Toolbox,
    pub roster: Roster,
    pub runtime: Rc<AgentRuntime>,
    pub approvals: ApprovalQueue,
    pub creation: bool,
}

impl CreationTools {
    pub fn from_config(
        config: &Value,
        tools: Toolbox,
        roster: Roster,
        runtime: Rc<AgentRuntime>,
        approvals: ApprovalQueue,
    ) -> Self {
        CreationTools {
            tools,
            roster,
            runtime,
            approvals,
            creation: config
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        }
    }
}

impl Component for CreationTools {
    fn name(&self) -> &str {
        "creation-tools"
    }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        if !self.creation {
            return Box::pin(async { Ok(()) });
        }

        let tools = self.tools.clone();
        let roster = self.roster.clone();
        let runtime = self.runtime.clone();
        let approvals = self.approvals.clone();

        Box::pin(async move {
            let _ = ctx;
            let names = [
                "inspect_plugins",
                "inspect_tools",
                "inspect_config",
                "define_script",
                "undefine_plugin",
                "save_patch",
            ];

            register(
                &tools,
                "inspect_plugins",
                "列出当前已装载的插件",
                tool_schema::empty_object(),
                move |_| json_list_plugins(&roster),
            );
            register(
                &tools,
                "inspect_tools",
                "列出当前已登记的工具",
                tool_schema::empty_object(),
                {
                    let tools = tools.clone();
                    move |_| json_list_tools(&tools)
                },
            );
            register(
                &tools,
                "inspect_config",
                "列出当前生效的配置行",
                tool_schema::inspect_config_schema(),
                {
                    let runtime = runtime.clone();
                    move |args| json_inspect_config(&runtime, args)
                },
            );
            register(
                &tools,
                "define_script",
                "提交脚本插件安装请求（写入文件，等待用户在界面审批后才热装）",
                tool_schema::define_script_schema(),
                {
                    let approvals = approvals.clone();
                    move |args| define_script(&approvals, args)
                },
            );
            register(
                &tools,
                "undefine_plugin",
                "按 id 禁用一行配置（立即生效）",
                tool_schema::id_only_schema(),
                {
                    let runtime = runtime.clone();
                    move |args| undefine_plugin(&runtime, args)
                },
            );
            register(
                &tools,
                "save_patch",
                "把动态 patch 层写入 cordis.patch.yml",
                tool_schema::save_patch_schema(),
                {
                    let runtime = runtime.clone();
                    move |args| save_patch(&runtime, args)
                },
            );

            let tools = tools.clone();
            steps.step_sync(move || {
                for name in names {
                    tools.remove(name);
                }
            })?;
            Ok(())
        })
    }
}

fn register(
    tools: &Toolbox,
    name: &str,
    description: &str,
    parameters: Value,
    handler: impl Fn(&str) -> Result<String> + 'static,
) {
    tools.insert_with_schema(
        name.into(),
        description.into(),
        "native",
        parameters,
        Rc::new(handler),
    );
}

fn json_list_plugins(roster: &Roster) -> Result<String> {
    let items: Vec<_> = roster
        .list()
        .into_iter()
        .map(|p| {
            json!({
                "name": p.name,
                "substrate": p.substrate,
                "role": p.role,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".into()))
}

fn json_list_tools(tools: &Toolbox) -> Result<String> {
    let items: Vec<_> = tools
        .list()
        .into_iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "substrate": t.substrate,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".into()))
}

fn json_inspect_config(runtime: &AgentRuntime, args: &str) -> Result<String> {
    let value = parse_json_args(args)?;
    let include_config = value
        .get("include_config")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entries = runtime.current_entries()?;
    let items: Vec<_> = entries
        .iter()
        .map(|entry| {
            let mut row = json!({
                "id": entry.id,
                "name": entry.name,
                "disabled": entry.disabled,
            });
            if include_config {
                row["config"] = entry.config.clone();
            }
            row
        })
        .collect();
    Ok(serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".into()))
}

fn define_script(approvals: &ApprovalQueue, args: &str) -> Result<String> {
    let value = parse_json_args(args)?;
    let id = arg_str(&value, "id")?.to_owned();
    let source = arg_str(&value, "source")?.to_owned();
    let role = value
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("模型生成的脚本插件")
        .to_owned();
    let grant = value.get("grant").cloned().unwrap_or(json!([]));

    let dir = root_dir().join("plugins/generated");
    fs::create_dir_all(&dir).map_err(map_io)?;
    let rel = format!("plugins/generated/{id}.js");
    let path = root_dir().join(&rel);
    fs::write(&path, &source).map_err(map_io)?;

    let layer = vec![
        Patch {
            id: Some(id.clone()),
            config: None,
            disabled: Some(true),
            insert: None,
            name: None,
        },
        Patch {
            id: None,
            config: None,
            disabled: None,
            insert: Some(vec![Entry::new(&id, "script").with_config(json!({
                "file": rel.clone(),
                "grant": grant,
                "role": role.clone(),
            }))]),
            name: None,
        },
    ];

    let pending = approvals.propose(id.clone(), rel.clone(), role, source, layer)?;
    Ok(format!(
        "已提交安装请求 `{id}` → {rel}（{} 行）。\
         请用户在浏览器界面点击「批准」后才会热装；拒绝会删除临时文件。",
        pending.source_lines
    ))
}

fn undefine_plugin(runtime: &AgentRuntime, args: &str) -> Result<String> {
    let value = parse_json_args(args)?;
    let id = arg_str(&value, "id")?;
    let applied = runtime.push_layer(vec![Patch {
        id: Some(id.to_owned()),
        config: None,
        disabled: Some(true),
        insert: None,
        name: None,
    }])?;
    Ok(format!(
        "已禁用 `{id}`\nremoved={:?} updated={:?}",
        applied.removed, applied.updated
    ))
}

fn save_patch(runtime: &AgentRuntime, args: &str) -> Result<String> {
    let value = parse_json_args(args)?;
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| root_dir().join("cordis.patch.yml"));

    let layers = runtime.layers();
    if layers.is_empty() {
        return Ok("没有动态 patch 层可保存。".into());
    }

    let mut out = String::from("# 由 creation-tools 自动生成\n");
    for layer in &layers {
        out.push_str(&format_patch_layer(layer)?);
    }
    fs::write(&path, out).map_err(map_io)?;
    Ok(format!("已写入 {}", path.display()))
}

fn format_patch_layer(layer: &[Patch]) -> Result<String> {
    let mut out = String::new();
    for patch in layer {
        if let Some(id) = &patch.id {
            out.push_str(&format!("- id: {id}\n"));
            if let Some(disabled) = patch.disabled {
                out.push_str(&format!("  disabled: {disabled}\n"));
            }
        }
        if let Some(insert) = &patch.insert {
            out.push_str("- insert:\n");
            for entry in insert {
                out.push_str(&format!("    - id: {}\n      name: {}\n", entry.id, entry.name));
                if entry.disabled {
                    out.push_str("      disabled: true\n");
                }
                if !entry.config.is_null() {
                    let cfg = serde_json::to_string_pretty(&entry.config).unwrap_or_default();
                    out.push_str("      config:\n");
                    for line in cfg.lines() {
                        out.push_str(&format!("        {line}\n"));
                    }
                }
            }
        }
    }
    Ok(out)
}

fn map_io(error: std::io::Error) -> spatiotemporal::Error {
    spatiotemporal::Error::Component(error.to_string())
}
