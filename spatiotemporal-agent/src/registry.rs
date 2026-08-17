use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};

use spatiotemporal::{Component, Error, Registry, Value};
use spatiotemporal_script::ScriptPlugin;
use spatiotemporal_process::ProcessPlugin;
use spatiotemporal_wasm::WasmPlugin;

use crate::approval::ApprovalQueue;
use crate::host::{
    Announcing, LlmInstaller, Roster, Toolbox, config_str, grant_list, process_caps,
    process_guest, resolve_path, script_caps, wasm_caps, wasm_guest,
};
use crate::host::root_dir;
use crate::plugins::{
    AgentLoopPlugin, BashSandbox, CreationTools, DeepSeek, DocFile, FsSandbox, PatchWatcher, Probe,
    ReadDoc, SystemPromptPlugin, ToolBash, ToolFs, ToolWebFetch, Web,
};
use crate::runtime::AgentRuntime;

#[derive(Clone)]
pub struct Host {
    pub tools: Toolbox,
    pub roster: Roster,
    pub approvals: ApprovalQueue,
    pub wasm_caps: Rc<spatiotemporal_wasm::Capabilities>,
    pub script_caps: Rc<spatiotemporal_script::Capabilities>,
    pub process_caps: Rc<spatiotemporal_process::Capabilities>,
    pub llm_wasm: Rc<dyn spatiotemporal_wasm::LlmHost>,
    pub llm_script: Rc<dyn spatiotemporal_script::LlmHost>,
    pub llm_process: Rc<dyn spatiotemporal_process::LlmHost>,
    pub runtime: Rc<RefCell<Option<Rc<AgentRuntime>>>>,
    pub reload_tx: Rc<RefCell<Option<Sender<()>>>>,
    pub reload_rx: Rc<RefCell<Option<Receiver<()>>>>,
    pub patch_path: Rc<RefCell<PathBuf>>,
}

impl Host {
    pub fn new() -> Self {
        let installer = Rc::new(LlmInstaller);
        let (reload_tx, reload_rx) = std::sync::mpsc::channel();
        Host {
            tools: Toolbox::new(),
            roster: Roster::new(),
            approvals: ApprovalQueue::new(),
            wasm_caps: wasm_caps(),
            script_caps: script_caps(),
            process_caps: process_caps(),
            llm_wasm: installer.clone() as Rc<dyn spatiotemporal_wasm::LlmHost>,
            llm_script: installer.clone() as Rc<dyn spatiotemporal_script::LlmHost>,
            llm_process: installer as Rc<dyn spatiotemporal_process::LlmHost>,
            runtime: Rc::new(RefCell::new(None)),
            reload_tx: Rc::new(RefCell::new(Some(reload_tx))),
            reload_rx: Rc::new(RefCell::new(Some(reload_rx))),
            patch_path: Rc::new(RefCell::new(root_dir().join("cordis.patch.yml"))),
        }
    }
}

pub fn registry(host: Host) -> Registry {
    let mut registry = Registry::new();

    {
        let host = host.clone();
        registry.add("fs-sandbox", move |config: &Value| {
            Ok(announce(
                Rc::new(FsSandbox::from_config(config)),
                host.roster.clone(),
                "native",
                "提供 fs（工作区沙箱）",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("tool-fs", move |_config: &Value| {
            Ok(announce(
                Rc::new(ToolFs {
                    tools: host.tools.clone(),
                }),
                host.roster.clone(),
                "native",
                "登记 read / write / edit",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("bash-sandbox", move |config: &Value| {
            Ok(announce(
                Rc::new(BashSandbox::from_config(config)),
                host.roster.clone(),
                "native",
                "提供 shell（工作区沙箱）",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("tool-bash", move |_config: &Value| {
            Ok(announce(
                Rc::new(ToolBash {
                    tools: host.tools.clone(),
                }),
                host.roster.clone(),
                "native",
                "登记 bash 工具",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("tool-web-fetch", move |_config: &Value| {
            Ok(announce(
                Rc::new(ToolWebFetch {
                    tools: host.tools.clone(),
                }),
                host.roster.clone(),
                "native",
                "登记 web_fetch 工具",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("system-prompt", move |config: &Value| {
            Ok(announce(
                Rc::new(SystemPromptPlugin::from_config(config)),
                host.roster.clone(),
                "native",
                "组装 system prompt（AGENTS.md + 工具 schema）",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("agent-loop", move |config: &Value| {
            Ok(announce(
                Rc::new(AgentLoopPlugin::from_config(config)),
                host.roster.clone(),
                "native",
                "可插拔 agent 循环",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("doc", move |config: &Value| {
            let path = config_str(config, "path")
                .ok_or_else(|| Error::Config("doc 需要 config.path".into()))?;
            let path = resolve_path(path);
            let plugin = DocFile::open(&path)
                .map_err(|error| Error::Config(format!("读不了 {}：{error}", path.display())))?;
            Ok(announce(
                Rc::new(plugin),
                host.roster.clone(),
                "native",
                "提供 markdown 能力",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("read-doc", move |_config: &Value| {
            Ok(announce(
                Rc::new(ReadDoc {
                    tools: host.tools.clone(),
                }),
                host.roster.clone(),
                "native",
                "登记读全文工具",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("deepseek", move |config: &Value| {
            Ok(announce(
                Rc::new(DeepSeek::from_config(config)),
                host.roster.clone(),
                "native",
                "提供 llm（DeepSeek）",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("web", move |config: &Value| {
            let runtime = host
                .runtime
                .borrow()
                .clone()
                .ok_or_else(|| Error::Config("web 需要 AgentRuntime".into()))?;
            let port = config
                .get("port")
                .and_then(Value::as_u64)
                .map(|port| port as u16)
                .or_else(|| {
                    std::env::var("PORT")
                        .ok()
                        .and_then(|port| port.parse().ok())
                })
                .unwrap_or(8787);
            let creation = config
                .get("creation")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(announce(
                Rc::new(Web {
                    port,
                    tools: host.tools.clone(),
                    roster: host.roster.clone(),
                    creation,
                    approvals: host.approvals.clone(),
                    runtime,
                    reload_rx: host.reload_rx.clone(),
                    patch_path: host.patch_path.borrow().clone(),
                }),
                host.roster.clone(),
                "native",
                if creation {
                    "浏览器界面（创造模式）"
                } else {
                    "浏览器界面"
                },
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("probe", move |_config: &Value| {
            Ok(announce(
                Rc::new(Probe {
                    tools: host.tools.clone(),
                    roster: host.roster.clone(),
                }),
                host.roster.clone(),
                "native",
                "命令行探测界面",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("patch-watcher", move |config: &Value| {
            let runtime = host
                .runtime
                .borrow()
                .clone()
                .ok_or_else(|| Error::Config("patch-watcher 需要 AgentRuntime".into()))?;
            if let Some(path) = config.get("path").and_then(Value::as_str) {
                *host.patch_path.borrow_mut() = if PathBuf::from(path).is_absolute() {
                    PathBuf::from(path)
                } else {
                    root_dir().join(path)
                };
            }
            Ok(announce(
                Rc::new(PatchWatcher::from_config(
                    config,
                    runtime,
                    host.reload_tx.clone(),
                )),
                host.roster.clone(),
                "native",
                "监听 cordis.patch.yml 热重载",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("creation-tools", move |config: &Value| {
            let runtime = host
                .runtime
                .borrow()
                .clone()
                .ok_or_else(|| Error::Config("creation-tools 需要 AgentRuntime".into()))?;
            Ok(announce(
                Rc::new(CreationTools::from_config(
                    config,
                    host.tools.clone(),
                    host.roster.clone(),
                    runtime,
                    host.approvals.clone(),
                    host.patch_path.borrow().clone(),
                )),
                host.roster.clone(),
                "native",
                "创造模式元工具",
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("wasm", move |config: &Value| {
            let guest = config_str(config, "guest")
                .ok_or_else(|| Error::Config("wasm 需要 config.guest".into()))?;
            let path = wasm_guest(guest);
            let plugin = WasmPlugin::open(&path, host.wasm_caps.clone(), grant_list(config))
                .map_err(|error| {
                    Error::Config(format!(
                        "编不了 {}：{error}\n先跑 spatiotemporal-agent/scripts/build-guests.sh",
                        path.display()
                    ))
                })?
                .with_tools(Rc::new(host.tools.clone()) as Rc<dyn spatiotemporal_wasm::ToolHost>)
                .with_llm(host.llm_wasm.clone());
            Ok(announce(
                Rc::new(plugin),
                host.roster.clone(),
                "wasm",
                config_str(config, "role").unwrap_or("wasm 插件"),
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("script", move |config: &Value| {
            let file = config_str(config, "file")
                .ok_or_else(|| Error::Config("script 需要 config.file".into()))?;
            let path = resolve_path(file);
            let source = fs::read_to_string(&path)
                .map_err(|error| Error::Config(format!("读不了 {}：{error}", path.display())))?;
            let name = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "script".into());
            let plugin = ScriptPlugin::from_source(
                name,
                source,
                host.script_caps.clone(),
                grant_list(config),
            )?
            .with_tools(Rc::new(host.tools.clone()) as Rc<dyn spatiotemporal_script::ToolHost>)
            .with_llm(host.llm_script.clone());
            Ok(announce(
                Rc::new(plugin),
                host.roster.clone(),
                "script",
                config_str(config, "role").unwrap_or("脚本插件"),
            ))
        });
    }

    {
        let host = host.clone();
        registry.add("process", move |config: &Value| {
            let command = config_str(config, "command")
                .or_else(|| config_str(config, "guest"))
                .ok_or_else(|| Error::Config("process 需要 config.command 或 guest".into()))?;
            let path = if PathBuf::from(command).is_absolute() {
                PathBuf::from(command)
            } else if command.contains('/') {
                resolve_path(command)
            } else {
                process_guest(command)
            };
            let args = config
                .get("args")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let plugin = ProcessPlugin::open(&path, host.process_caps.clone(), grant_list(config))
                .map_err(|error| {
                    Error::Config(format!(
                        "打不开 {}：{error}",
                        path.display()
                    ))
                })?
                .with_args(args)
                .with_tools(Rc::new(host.tools.clone()) as Rc<dyn spatiotemporal_process::ToolHost>)
                .with_llm(host.llm_process.clone());
            Ok(announce(
                Rc::new(plugin),
                host.roster.clone(),
                "process",
                config_str(config, "role").unwrap_or("子进程插件"),
            ))
        });
    }

    registry
}

fn announce(
    inner: Rc<dyn Component>,
    roster: Roster,
    substrate: &str,
    role: &str,
) -> Rc<dyn Component> {
    Announcing::wrap(inner, roster, substrate, role)
}
