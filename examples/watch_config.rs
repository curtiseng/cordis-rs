//! 配置热重载：编辑文件，运行中的树自己跟上。
//!
//! 这个例子把论文 5.2 节那一层跑起来：一份基础配置 `cordis.yml`，加一层用户
//! patch `cordis.patch.yml`，由 [`notify`] 监听、防抖、重新叠加、对账。
//!
//! 它自己扮演那个编辑文件的人，依次演示四件事：
//!
//! 1. 改一行的 config —— 只有那一行重挂
//! 2. 把提供者换成另一个实现 —— 它的消费者自己重载，没写任何重连逻辑
//! 3. 写一个不存在的包名 —— 被拒绝，而**运行中的树毫发无伤**
//! 4. 改回来 —— 恢复
//!
//! `cargo run --example watch_config`
//!
//! 文件监听本身刻意留在库外面：它属于宿主的职责，而库要提供的是那个可对账、
//! 可回滚的核心。

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use futures::future::LocalBoxFuture;
use notify::{RecursiveMode, Watcher};
use spatiotemporal::{
    App, Component, Context, Key, KeyId, Loader, Registry, Result, State, Steps, Value, compose,
    parse_entries, parse_patches,
};

/* ------------------------------------------------------------------ */
/* 几个假装是插件的组件                                                */
/* ------------------------------------------------------------------ */

/// 一项能力：执行命令的沙箱。
enum Sandbox {}
impl Key for Sandbox {
    type Api = dyn SandboxApi;
    const NAME: &'static str = "sandbox";
}

trait SandboxApi {
    fn where_it_runs(&self) -> &'static str;
}

struct Local;
impl SandboxApi for Local {
    fn where_it_runs(&self) -> &'static str {
        "本机"
    }
}

struct Remote;
impl SandboxApi for Remote {
    fn where_it_runs(&self) -> &'static str {
        "远端"
    }
}

/// 提供沙箱的那一行。
struct SandboxProvider {
    api: fn() -> Rc<dyn SandboxApi>,
}

impl Component for SandboxProvider {
    fn name(&self) -> &'static str {
        "sandbox"
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let api = (self.api)();
        Box::pin(async move {
            println!("    · sandbox 就位（{}）", api.where_it_runs());
            ctx.set::<Sandbox>(api);
            Ok(())
        })
    }
}

/// 依赖沙箱的那一行。它对「沙箱被换掉」这件事一无所知。
struct ToolBash;

impl Component for ToolBash {
    fn name(&self) -> &'static str {
        "tool-bash"
    }

    fn inject(&self) -> Vec<KeyId> {
        vec![KeyId::of::<Sandbox>()]
    }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let sandbox = ctx.resolve::<Sandbox>()?;
            let place = sandbox.where_it_runs();
            println!("    · tool-bash 注册，命令将在{place}执行");
            steps.step(move || async move {
                println!("    · tool-bash 注销（原本在{place}）")
            })?;
            Ok(())
        })
    }
}

/// 一行普通的工具，用 config 控制自己的行为。
struct ToolWeb {
    fetch: bool,
}

impl Component for ToolWeb {
    fn name(&self) -> &'static str {
        "tool-web"
    }

    fn apply(&self, _ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let verbs = if self.fetch {
            "web_search + web_fetch"
        } else {
            "web_search"
        };
        Box::pin(async move {
            println!("    · tool-web 注册 {verbs}");
            steps.step(move || async move { println!("    · tool-web 注销 {verbs}") })?;
            Ok(())
        })
    }
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .add("dsh-sandbox-local", |_| {
            Ok(Rc::new(SandboxProvider {
                api: || Rc::new(Local),
            }) as Rc<dyn Component>)
        })
        .add("dsh-sandbox-remote", |_| {
            Ok(Rc::new(SandboxProvider {
                api: || Rc::new(Remote),
            }) as Rc<dyn Component>)
        })
        .add("dsh-tool-bash", |_| {
            Ok(Rc::new(ToolBash) as Rc<dyn Component>)
        })
        .add("dsh-tool-web", |config: &Value| {
            let fetch = config
                .get("fetch")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(Rc::new(ToolWeb { fetch }) as Rc<dyn Component>)
        });
    registry
}

/* ------------------------------------------------------------------ */
/* 配置文件                                                            */
/* ------------------------------------------------------------------ */

const BASE_YAML: &str = r#"# 基础组合：像 dsh 的 packages/bundle/base/cordis.patch.yml 那样，
# 每一行是一项能力。
- id: sandbox
  name: dsh-sandbox-local

- id: tool-bash
  name: dsh-tool-bash

- id: tool-web
  name: dsh-tool-web
  config:
    fetch: false
"#;

/// 读文件、叠加、对账。这就是热重载的全部。
fn reload(app: &mut App, loader: &Loader, dir: &Path) {
    let text = match fs::read_to_string(dir.join("cordis.yml")) {
        Ok(text) => text,
        Err(error) => return println!("  ✗ 读不到基础配置：{error}"),
    };
    let patch_path = dir.join("cordis.patch.yml");

    let composed = (|| {
        let base = parse_entries(&text)?;
        let layers = if patch_path.exists() {
            vec![parse_patches(
                &fs::read_to_string(&patch_path).unwrap_or_default(),
            )?]
        } else {
            Vec::new()
        };
        compose(&base, &layers)
    })();

    let composed = match composed {
        Ok(composed) => composed,
        // 解析或叠加失败：上一棵可用的树继续跑。
        Err(error) => return println!("  ✗ 配置被拒，仍在跑上一份：{error}"),
    };
    for warning in &composed.warnings {
        println!("  ! {warning}");
    }

    match app.block_on(loader.apply(composed.entries)) {
        Ok(applied) if applied.is_noop() => println!("  = 没有变化"),
        Ok(applied) => println!(
            "  ✓ 新增 {:?}　更新 {:?}　移除 {:?}",
            applied.created, applied.updated, applied.removed
        ),
        // 候选被拒。此时树一定仍是可用的：要么根本没被动过（名字不认识、配置
        // 不合法都在拆除之前同步失败），要么先前那些行已被重建回去。
        Err(error) => println!("  ✗ 候选被拒，树仍可用：{error}"),
    }
}

fn show(loader: &Loader) {
    let rows: Vec<String> = loader
        .ids()
        .iter()
        .map(|id| {
            let state = match loader.state(id) {
                Some(State::Active) => "活动",
                Some(State::Inactive) => "非活动",
                Some(State::Failed) => "失败",
                Some(_) => "转换中",
                None => "未装",
            };
            format!("{id}({state})")
        })
        .collect();
    println!("  树：{}", rows.join(" "));
}

/// 等一次文件变更事件。
///
/// 这里监听目录而不是确切路径，因为那是最省事又可靠的做法。dsh 监听确切路径，
/// 于是必须处理「文件或其父目录尚不存在」——它从最近的现有祖先开始监听再补回
/// 缺失的后缀。
fn wait_for_change(rx: &Receiver<()>) -> bool {
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(()) => {
            // 防抖：一次保存常常产生一串事件，把它们吸收掉。
            while rx.recv_timeout(Duration::from_millis(80)).is_ok() {}
            true
        }
        Err(RecvTimeoutError::Timeout) => {
            println!("  ! watcher 5 秒内没报事件，直接对账");
            false
        }
        Err(RecvTimeoutError::Disconnected) => false,
    }
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir: PathBuf = std::env::temp_dir().join(format!("spatiotemporal-watch-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("cordis.yml"), BASE_YAML)?;

    let mut app = App::new();
    let loader = Loader::new(app.root(), registry());

    println!("配置目录：{}", dir.display());
    println!("\n[1] 首次装配");
    reload(&mut app, &loader, &dir);
    show(&loader);

    let (tx, rx) = channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            // 发送失败只意味着主线程已经不看了。
            let _ = tx.send(());
        }
    })?;
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    let edits: [(&str, &str); 4] = [
        (
            "[2] 编辑 patch：给 tool-web 开 fetch",
            "- id: tool-web\n  config:\n    fetch: true\n",
        ),
        (
            "[3] 编辑 patch：把沙箱换成远端（注意 tool-bash 自己跟上）",
            // 换实现的写法是关掉旧行再插入新行。patch 里的 `name` 是断言而不是
            // 赋值，所以一行不能被改成另一个包。
            "- id: tool-web\n  config:\n    fetch: true\n\n\
             - id: sandbox\n  disabled: true\n\n\
             - insert:\n    - id: sandbox-remote\n      name: dsh-sandbox-remote\n",
        ),
        (
            "[4] 编辑 patch：插入一个不存在的包名",
            "- insert:\n    - id: 打错了\n      name: 并不存在的包\n",
        ),
        (
            "[5] 编辑 patch：改回来",
            "- id: tool-web\n  config:\n    fetch: true\n",
        ),
    ];

    for (title, patch) in edits {
        println!("\n{title}");
        fs::write(dir.join("cordis.patch.yml"), patch)?;
        wait_for_change(&rx);
        reload(&mut app, &loader, &dir);
        show(&loader);
    }

    println!("\n[6] 收工，按 LIFO 拆掉整棵树");
    app.block_on(loader.unload_all());
    show(&loader);

    drop(watcher);
    fs::remove_dir_all(&dir)?;
    Ok(())
}
