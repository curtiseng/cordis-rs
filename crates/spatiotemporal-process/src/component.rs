use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::LocalBoxFuture;
use serde_json::{Map, Value};
use spatiotemporal::{Component, Context, Error, KeyId, Result, Steps};

use crate::host::Capabilities;
use crate::protocol::{
    DEFAULT_IO_TIMEOUT, Session, expect_ok, invoke_request, load_request, take_llm, take_logs,
    take_result, take_tools, unload_request,
};
use crate::{LlmHost, ToolHost, ToolInvoke};

/// 默认等 guest 跑完 `unload` 的上限；超时则 kill。
const DEFAULT_UNLOAD_TIMEOUT: Duration = Duration::from_secs(5);

/// 一个被接成 fiber 的子进程 guest。
///
/// 这个类型是整件事的全部：它就是 [`Component`] 的又一种实现。内核不知道
/// stdio 存在。
pub struct ProcessPlugin {
    command: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
    name: String,
    granted: Vec<String>,
    inject: Vec<KeyId>,
    capabilities: Rc<Capabilities>,
    io_timeout: Duration,
    unload_timeout: Duration,
    logs: Arc<Mutex<Vec<String>>>,
    tools: Option<Rc<dyn ToolHost>>,
    llm: Option<Rc<dyn LlmHost>>,
}

impl ProcessPlugin {
    /// 配置一个可执行 guest，并把授予它的能力名翻成 coeffect 键。
    ///
    /// 名字默认取可执行文件的 stem——这正是内核把 [`Component::name`] 从
    /// `&'static str` 放宽成 `&str` 的原因。
    pub fn open(
        command: impl AsRef<Path>,
        capabilities: Rc<Capabilities>,
        granted: Vec<String>,
    ) -> Result<Self> {
        let command = command.as_ref().to_path_buf();
        if !command.is_file() {
            return Err(Error::Component(format!(
                "找不到 guest 可执行文件：{}",
                command.display()
            )));
        }
        let name = command
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "process".to_owned());
        let inject = capabilities.inject(&granted)?;

        Ok(ProcessPlugin {
            command,
            args: Vec::new(),
            env: Vec::new(),
            name,
            granted,
            inject,
            capabilities,
            io_timeout: DEFAULT_IO_TIMEOUT,
            unload_timeout: DEFAULT_UNLOAD_TIMEOUT,
            logs: Arc::new(Mutex::new(Vec::new())),
            tools: None,
            llm: None,
        })
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_tools(mut self, tools: Rc<dyn ToolHost>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn with_llm(mut self, llm: Rc<dyn LlmHost>) -> Self {
        self.llm = Some(llm);
        self
    }

    pub fn with_io_timeout(mut self, timeout: Duration) -> Self {
        self.io_timeout = timeout;
        self
    }

    pub fn with_unload_timeout(mut self, timeout: Duration) -> Self {
        self.unload_timeout = timeout;
        self
    }

    /// guest 至今记下的日志。
    pub fn logs(&self) -> Vec<String> {
        self.logs
            .lock()
            .map(|logs| logs.clone())
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for ProcessPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessPlugin")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("granted", &self.granted)
            .field("io_timeout", &self.io_timeout)
            .field("unload_timeout", &self.unload_timeout)
            .finish_non_exhaustive()
    }
}

impl Component for ProcessPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn inject(&self) -> Vec<KeyId> {
        self.inject.clone()
    }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let command = self.command.clone();
        let args = self.args.clone();
        let env = self.env.clone();
        let io_timeout = self.io_timeout;
        let unload_timeout = self.unload_timeout;
        let logs = self.logs.clone();
        let tools = self.tools.clone();
        let llm = self.llm.clone();
        let name = self.name.clone();
        let capabilities = self.capabilities.clone();
        let granted = self.granted.clone();

        Box::pin(async move {
            let view = capabilities.snapshot(&ctx, &granted)?;
            let mut cmd = Command::new(&command);
            cmd.args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());
            for (key, value) in env {
                cmd.env(key, value);
            }

            let mut child = cmd
                .spawn()
                .map_err(|error| Error::Component(format!("启动 guest 失败：{error}")))?;

            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| Error::Component("guest 没 stdin".into()))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| Error::Component("guest 没 stdout".into()))?;

            let mut session = Session::new(stdin, stdout, io_timeout);
            let capabilities: Map<String, Value> = view
                .into_iter()
                .map(|(key, value)| (key, Value::String(value)))
                .collect();

            let load_id = session.next_id();
            let load = session.transact(load_request(load_id, &capabilities));
            let load = match load {
                Ok(response) => response,
                Err(error) => {
                    let _ = child.kill();
                    return Err(error);
                }
            };
            if let Err(error) = expect_ok(&load) {
                let _ = child.kill();
                return Err(error);
            }

            append_logs(&logs, take_logs(&load));

            let registered = take_tools(&load);
            let model = take_llm(&load);

            let live = Rc::new(RefCell::new(Live {
                child,
                session,
            }));

            if let Some(host) = &tools {
                for (tool_name, description) in registered {
                    let live = live.clone();
                    let invoke_name = tool_name.clone();
                    let invoke: ToolInvoke = Rc::new(move |args: &str| {
                        live.borrow_mut().invoke(&invoke_name, args)
                    });
                    host.register(tool_name.clone(), description, invoke);
                    let host = host.clone();
                    steps.step_sync(move || host.unregister(&tool_name))?;
                }
            }

            if let Some(model) = model {
                let Some(host) = &llm else {
                    kill_live(&mut live.borrow_mut());
                    return Err(Error::Component("guest 登记了 LLM，但宿主没接".into()));
                };
                let live = live.clone();
                let invoke: ToolInvoke =
                    Rc::new(move |args: &str| live.borrow_mut().invoke("__llm", args));
                host.install(&ctx, model, invoke)?;
            }

            let logs_for_unload = logs.clone();
            let name_for_unload = name.clone();
            steps.step_sync(move || {
                let mut live = live.borrow_mut();
                let unload_id = live.session.next_id();
                let outcome = live
                    .session
                    .transact_with_timeout(unload_request(unload_id), unload_timeout);
                match outcome {
                    Ok(response) => {
                        append_logs(&logs_for_unload, take_logs(&response));
                        let _ = expect_ok(&response);
                    }
                    Err(error) => {
                        if let Ok(mut logs) = logs_for_unload.lock() {
                            logs.push(format!("{name_for_unload} 的 unload 陷入了：{error}"));
                        }
                    }
                }
                kill_live(&mut live);
            })
        })
    }
}

struct Live {
    child: Child,
    session: Session<std::process::ChildStdout>,
}

impl Live {
    fn invoke(&mut self, name: &str, args: &str) -> Result<String> {
        let id = self.session.next_id();
        let response = self
            .session
            .transact(invoke_request(id, name, args))?;
        take_result(&response)
    }
}

fn append_logs(logs: &Arc<Mutex<Vec<String>>>, lines: Vec<String>) {
    if let Ok(mut bucket) = logs.lock() {
        bucket.extend(lines);
    }
}

fn kill_live(live: &mut Live) {
    let _ = live.child.kill();
    let _ = live.child.wait();
}
