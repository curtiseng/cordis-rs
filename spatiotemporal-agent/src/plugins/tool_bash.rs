use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, KeyId, Result, Steps};

use crate::host::Toolbox;
use crate::keys::ShellKey;
use crate::tool_schema;
use crate::util::{parse_json_args, resolve_within};

const TIMEOUT: Duration = Duration::from_secs(30);

/// 本地插件：bash 工具。
pub struct ToolBash {
    pub tools: Toolbox,
}

impl Component for ToolBash {
    fn name(&self) -> &str {
        "tool-bash"
    }

    fn inject(&self) -> Vec<KeyId> {
        vec![KeyId::of::<ShellKey>()]
    }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let tools = self.tools.clone();
        Box::pin(async move {
            let shell = ctx.resolve::<ShellKey>()?;
            let root = shell.root().to_path_buf();

            tools.insert_with_schema(
                "bash".into(),
                "在工作区内执行 shell 命令（30 秒超时）".into(),
                "native",
                tool_schema::bash_schema(),
                Rc::new(move |args: &str| run_bash(&root, args)),
            );

            let tools = tools.clone();
            steps.step_sync(move || tools.remove("bash"))?;
            Ok(())
        })
    }
}

fn run_bash(root: &Path, args: &str) -> Result<String> {
    let value = parse_json_args(args)?;
    let command = value
        .get("command")
        .or_else(|| value.get("input"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(args)
        .trim();
    if command.is_empty() {
        return Err(spatiotemporal::Error::Component("空命令".into()));
    }

    let cwd = value
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(|path| resolve_within(root, path))
        .transpose()?
        .unwrap_or_else(|| root.to_path_buf());

    let mut child = Command::new("bash")
        .arg("-lc")
        .arg(command)
        .current_dir(&cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| spatiotemporal::Error::Component(format!("启动 bash 失败：{error}")))?;

    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| spatiotemporal::Error::Component(error.to_string()))?
        {
            let output = child
                .wait_with_output()
                .map_err(|error| spatiotemporal::Error::Component(error.to_string()))?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut text = format!(
                "exit={}\ncwd={}\n",
                status.code().unwrap_or(-1),
                cwd.display()
            );
            if !stdout.is_empty() {
                text.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !stdout.is_empty() {
                    text.push('\n');
                }
                text.push_str("--- stderr ---\n");
                text.push_str(&stderr);
            }
            const LIMIT: usize = 32_000;
            if text.len() > LIMIT {
                text.truncate(LIMIT);
                text.push_str("\n…（输出已截断）");
            }
            return Ok(text);
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            return Err(spatiotemporal::Error::Component(format!(
                "命令超时（>{TIMEOUT:?}）：{command}"
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
