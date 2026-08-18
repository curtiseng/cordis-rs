use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use serde_json::json;
use spatiotemporal::{Component, Context, Entry, Patch, Result, Steps, Value, parse_patches};

use crate::approval::ApprovalQueue;
use crate::host::{Roster, Toolbox, root_dir};
use crate::patch_yaml;
use crate::plugins::patch_watcher::reload_patch_file;
use crate::runtime::AgentRuntime;
use crate::tool_schema;
use crate::util::{arg_str, parse_json_args};

/// 创造模式元工具：检视运行时、试跑 patch、提交安装（需人工审批）、持久化 patch。
pub struct CreationTools {
    pub tools: Toolbox,
    pub roster: Roster,
    pub runtime: Rc<AgentRuntime>,
    pub approvals: ApprovalQueue,
    pub patch_path: PathBuf,
    pub creation: bool,
}

impl CreationTools {
    pub fn from_config(
        config: &Value,
        tools: Toolbox,
        roster: Roster,
        runtime: Rc<AgentRuntime>,
        approvals: ApprovalQueue,
        patch_path: PathBuf,
    ) -> Self {
        CreationTools {
            tools,
            roster,
            runtime,
            approvals,
            patch_path,
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
        let patch_path = self.patch_path.clone();

        Box::pin(async move {
            let _ = ctx;
            let names = [
                "inspect_plugins",
                "inspect_tools",
                "inspect_config",
                "define_script",
                "define_plugin",
                "run_patch",
                "revert_patch",
                "reload_patch",
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
                "提交 script 插件安装（同 define_plugin，name=script）",
                tool_schema::define_script_schema(),
                {
                    let approvals = approvals.clone();
                    move |args| define_plugin(&approvals, args)
                },
            );
            register(
                &tools,
                "define_plugin",
                "提交任意 registry 插件安装（script/wasm/process/已有 native），需界面审批",
                tool_schema::define_plugin_schema(),
                {
                    let approvals = approvals.clone();
                    move |args| define_plugin(&approvals, args)
                },
            );
            register(
                &tools,
                "run_patch",
                "试跑一层 patch YAML（按审批策略可能需界面批准）",
                tool_schema::run_patch_schema(),
                {
                    let runtime = runtime.clone();
                    let approvals = approvals.clone();
                    move |args| run_patch(&runtime, &approvals, args)
                },
            );
            register(
                &tools,
                "revert_patch",
                "撤销最近一次 run_patch / 批准安装 追加的动态层",
                tool_schema::empty_object(),
                {
                    let runtime = runtime.clone();
                    move |_| revert_patch(&runtime)
                },
            );
            register(
                &tools,
                "reload_patch",
                "从 cordis.patch.yml 重新加载文件层",
                tool_schema::reload_patch_schema(),
                {
                    let runtime = runtime.clone();
                    let patch_path = patch_path.clone();
                    move |args| reload_patch(&runtime, &patch_path, args)
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
                "把运行时动态 patch 层写入 cordis.patch.yml（合法 YAML）",
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

fn define_plugin(approvals: &ApprovalQueue, args: &str) -> Result<String> {
    let value = parse_json_args(args)?;
    let id = arg_str(&value, "id")?.to_owned();
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("script")
        .to_owned();
    let role = value
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("模型生成的插件")
        .to_owned();

    let mut cleanup_path = None;
    let config = match name.as_str() {
        "script" => {
            let source = arg_str(&value, "source")?.to_owned();
            let grant = value.get("grant").cloned().unwrap_or(json!([]));
            let dir = root_dir().join("plugins/generated");
            fs::create_dir_all(&dir).map_err(map_io)?;
            let rel = format!("plugins/generated/{id}.js");
            fs::write(root_dir().join(&rel), &source).map_err(map_io)?;
            cleanup_path = Some(PathBuf::from(&rel));
            let preview: String = source.chars().take(800).collect();
            let lines = source.lines().count();
            let config = json!({ "file": rel, "grant": grant, "role": role.clone() });
            return finish_propose(
                approvals,
                id,
                "script",
                rel,
                preview,
                Some(lines),
                config,
                name,
                cleanup_path,
            );
        }
        "wasm" => {
            let guest = value
                .get("guest")
                .or_else(|| value.get("config").and_then(|c| c.get("guest")))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    spatiotemporal::Error::Component("wasm 插件需要 guest 字段".into())
                })?;
            let grant = value.get("grant").cloned().unwrap_or(json!([]));
            json!({ "guest": guest, "grant": grant, "role": role.clone() })
        }
        _ => value.get("config").cloned().unwrap_or(json!({})),
    };

    let preview = serde_json::to_string_pretty(&config).unwrap_or_else(|_| config.to_string());
    let id_for_summary = id.clone();
    finish_propose(
        approvals,
        id,
        &name,
        format!("{name} ({id_for_summary})"),
        preview,
        None,
        config,
        name.clone(),
        cleanup_path,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_propose(
    approvals: &ApprovalQueue,
    id: String,
    kind: &str,
    summary: String,
    preview: String,
    source_lines: Option<usize>,
    config: Value,
    name: String,
    cleanup_path: Option<PathBuf>,
) -> Result<String> {
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
            insert: Some(vec![Entry::new(&id, &name).with_config(config)]),
            name: None,
        },
    ];
    let _pending = approvals.propose_install(
        id.clone(),
        kind.into(),
        summary.clone(),
        preview,
        source_lines,
        layer,
        cleanup_path,
    )?;
    Ok(format!(
        "已提交安装请求 `{id}` → {summary}。\
         请用户在浏览器界面点击「批准」后才会热装。{}",
        source_lines
            .map(|lines| format!("（{lines} 行）"))
            .unwrap_or_default()
    ))
}

fn run_patch(runtime: &AgentRuntime, approvals: &ApprovalQueue, args: &str) -> Result<String> {
    let value = parse_json_args(args)?;
    let yaml = arg_str(&value, "patch")?;
    let layer = parse_patches(yaml)?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("patch-run")
        .to_owned();

    if approvals.requires("patch") {
        let preview: String = yaml.chars().take(1200).collect();
        let lines = yaml.lines().count();
        approvals.propose_install(
            id,
            "patch".into(),
            format!("试跑 patch（{} 条）", layer.len()),
            preview,
            Some(lines),
            layer,
            None,
        )?;
        return Ok("已提交 patch 试跑请求。请用户在浏览器界面点击「批准」后才会热装。".into());
    }

    let applied = runtime.push_layer(layer)?;
    Ok(format!(
        "已试跑 patch（{} 条）\ncreated={:?} updated={:?} removed={:?}\n\
         用 revert_patch 可撤销。",
        applied.created.len() + applied.updated.len() + applied.removed.len(),
        applied.created,
        applied.updated,
        applied.removed
    ))
}

fn revert_patch(runtime: &AgentRuntime) -> Result<String> {
    match runtime.pop_layer()? {
        Some(applied) => Ok(format!(
            "已撤销上一动态层\nupdated={:?} removed={:?}",
            applied.updated, applied.removed
        )),
        None => Ok("没有可撤销的动态层。".into()),
    }
}

fn reload_patch(runtime: &AgentRuntime, default_path: &Path, args: &str) -> Result<String> {
    let value = parse_json_args(args)?;
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| default_path.to_path_buf());
    reload_patch_file(runtime, &path)
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

    let flat: Vec<Patch> = runtime.dynamic_layers().into_iter().flatten().collect();
    if flat.is_empty() {
        return Ok("没有运行时动态 patch 可保存（bootstrap / 文件层不含在内）。".into());
    }

    let mut out = String::from("# 由 creation-tools 自动生成\n");
    out.push_str(&patch_yaml::render_patches(&flat)?);
    fs::write(&path, out).map_err(map_io)?;
    Ok(format!(
        "已写入 {}（{} 条 patch）",
        path.display(),
        flat.len()
    ))
}

fn map_io(error: std::io::Error) -> spatiotemporal::Error {
    spatiotemporal::Error::Component(error.to_string())
}
