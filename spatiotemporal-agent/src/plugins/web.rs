use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use futures::future::LocalBoxFuture;
use serde_json::{Value, json};
use spatiotemporal::{Component, Context, KeyId, Result, Steps};
use tiny_http::{Header, Method, Response, Server, StatusCode};

use crate::approval::ApprovalQueue;
use crate::chat::TurnRequest;
use crate::host::{AgentLoop, Document, Llm, Roster, Surface, Toolbox};
use crate::keys::{AgentLoopKey, Doc, FsKey, LlmKey, SurfaceKey};
use crate::plugins::patch_watcher::{drain_reload, reload_patch_file};
use crate::runtime::AgentRuntime;
use crate::session;

const INDEX: &str = include_str!("../../assets/index.html");

/// 本地插件：浏览器界面。`apply` 只提供 `surface`，听端口发生在 `run`。
pub struct Web {
    pub port: u16,
    pub tools: Toolbox,
    pub roster: Roster,
    pub approvals: ApprovalQueue,
    pub runtime: Rc<AgentRuntime>,
    pub reload_rx: Rc<RefCell<Option<Receiver<()>>>>,
    pub patch_path: PathBuf,
}

struct WebSurface {
    port: u16,
    tools: Toolbox,
    roster: Roster,
    llm: Rc<dyn Llm>,
    agent_loop: Rc<dyn AgentLoop>,
    doc: Rc<dyn Document>,
    workspace: String,
    approvals: ApprovalQueue,
    runtime: Rc<AgentRuntime>,
    reload_rx: Rc<RefCell<Option<Receiver<()>>>>,
    patch_path: PathBuf,
}

impl Surface for WebSurface {
    fn kind(&self) -> &'static str {
        "web"
    }

    fn run(&self) {
        let port = self.port;
        println!("文档：{}", self.doc.path());
        println!("工作区：{}", self.workspace);
        println!("打开 http://127.0.0.1:{port}");
        serve(ServeContext {
            port,
            tools: &self.tools,
            roster: &self.roster,
            llm: &*self.llm,
            agent_loop: &*self.agent_loop,
            doc: &*self.doc,
            workspace: &self.workspace,
            approvals: &self.approvals,
            runtime: &self.runtime,
            reload_rx: self.reload_rx.clone(),
            patch_path: self.patch_path.clone(),
            kind: self.kind(),
        });
    }
}

impl Component for Web {
    fn name(&self) -> &str {
        "web"
    }

    fn inject(&self) -> Vec<KeyId> {
        vec![
            KeyId::of::<Doc>(),
            KeyId::of::<LlmKey>(),
            KeyId::of::<FsKey>(),
            KeyId::of::<AgentLoopKey>(),
        ]
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let port = self.port;
        let tools = self.tools.clone();
        let roster = self.roster.clone();
        let approvals = self.approvals.clone();
        let runtime = self.runtime.clone();
        let reload_rx = self.reload_rx.clone();
        let patch_path = self.patch_path.clone();
        Box::pin(async move {
            let doc = ctx.resolve::<Doc>()?;
            let llm = ctx.resolve::<LlmKey>()?;
            let agent_loop = ctx.resolve::<AgentLoopKey>()?;
            let fs = ctx.resolve::<FsKey>()?;
            let workspace = fs.root().display().to_string();
            let surface: Rc<dyn Surface> = Rc::new(WebSurface {
                port,
                tools,
                roster,
                llm,
                agent_loop,
                doc,
                workspace,
                approvals,
                runtime,
                reload_rx,
                patch_path,
            });
            ctx.set::<SurfaceKey>(surface);
            Ok(())
        })
    }
}

struct ServeContext<'a> {
    port: u16,
    tools: &'a Toolbox,
    roster: &'a Roster,
    llm: &'a dyn Llm,
    agent_loop: &'a dyn AgentLoop,
    doc: &'a dyn Document,
    workspace: &'a str,
    approvals: &'a ApprovalQueue,
    runtime: &'a AgentRuntime,
    reload_rx: Rc<RefCell<Option<Receiver<()>>>>,
    patch_path: PathBuf,
    kind: &'a str,
}

fn serve(ctx: ServeContext<'_>) {
    let ServeContext {
        port,
        tools,
        roster,
        llm,
        agent_loop,
        doc,
        workspace,
        approvals,
        runtime,
        reload_rx,
        patch_path,
        kind,
    } = ctx;
    let workspace_path = Path::new(workspace);
    let server = Server::http(("127.0.0.1", port)).unwrap_or_else(|error| {
        panic!("听不了 {port}：{error}");
    });

    for mut request in server.incoming_requests() {
        if let Some(rx) = reload_rx.borrow().as_ref() {
            drain_reload(runtime, patch_path.as_path(), rx);
        }
        let raw_url = request.url().to_owned();
        let url = raw_url.split('?').next().unwrap_or("/").to_owned();
        let query = parse_query(&raw_url);
        let method = request.method().clone();

        let response = match (method, url.as_str()) {
            (Method::Get, "/") => html(INDEX),
            (Method::Get, "/api/status") => {
                let creation = runtime.creation_enabled();
                let pending = approvals.pending_all();
                let policy = approvals.policy();
                json_ok(json!({
                    "doc": doc.path(),
                    "model": llm.model(),
                    "has_key": std::env::var("DEEPSEEK_API_KEY").is_ok(),
                    "surface": kind,
                    "creation": creation,
                    "workspace": workspace,
                    "pending": pending,
                    "pending_count": pending.len(),
                    "approval_policy": {
                        "require": policy.require,
                        "max_queue": policy.max_queue,
                    },
                    "tools": tools.list().into_iter().map(|t| json!({
                        "name": t.name,
                        "description": t.description,
                        "substrate": t.substrate,
                    })).collect::<Vec<_>>(),
                    "plugins": roster.list().into_iter().map(|p| json!({
                        "name": p.name,
                        "substrate": p.substrate,
                        "role": p.role,
                    })).collect::<Vec<_>>(),
                }))
            }
            (Method::Get, "/api/session") => {
                let session_id = query
                    .get("session_id")
                    .cloned()
                    .unwrap_or_default();
                if session_id.is_empty() {
                    json_err(400, "缺少 session_id")
                } else {
                    match session::derive_messages(workspace_path, &session_id) {
                        Ok(history) => json_ok(json!({ "session_id": session_id, "history": history })),
                        Err(error) => json_err(500, &error.to_string()),
                    }
                }
            }
            (Method::Get, "/api/creation/pending") => {
                let pending = approvals.pending_all();
                json_ok(json!({ "pending": pending, "count": pending.len() }))
            }
            (Method::Post, "/api/creation/approve") => {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let payload: Value = serde_json::from_str(&body).unwrap_or(json!({}));
                let approve = payload["approve"].as_bool().unwrap_or(false);
                let id = payload["id"].as_str();
                let reason = payload["reason"].as_str();
                let result = if approve {
                    approvals.approve(runtime, id)
                } else {
                    approvals.reject(id, reason)
                };
                match result {
                    Ok(message) => json_ok(json!({ "ok": true, "message": message })),
                    Err(error) => json_err(400, &error.to_string()),
                }
            }
            (Method::Post, "/api/mode") => {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let payload: Value = serde_json::from_str(&body).unwrap_or(json!({}));
                match payload.get("creation").and_then(Value::as_bool) {
                    None => json_err(400, "需要 JSON 字段 creation: true/false"),
                    Some(enabled) => match runtime.set_creation_mode(enabled) {
                        Ok(applied) => json_ok(json!({
                            "ok": true,
                            "creation": runtime.creation_enabled(),
                            "message": if enabled {
                                "已开启创造模式"
                            } else {
                                "已关闭创造模式"
                            },
                            "created": applied.created,
                            "updated": applied.updated,
                            "removed": applied.removed,
                        })),
                        Err(error) => json_err(400, &error.to_string()),
                    },
                }
            }
            (Method::Post, "/api/creation/reload-patch") => {
                match reload_patch_file(runtime, patch_path.as_path()) {
                    Ok(message) => json_ok(json!({ "ok": true, "message": message })),
                    Err(error) => json_err(400, &error.to_string()),
                }
            }
            (Method::Get, "/api/doc") => json_ok(json!({
                "path": doc.path(),
                "text": doc.text(),
            })),
            (Method::Post, "/api/chat") => {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let payload: Value = serde_json::from_str(&body).unwrap_or(json!({}));
                let user = payload["message"].as_str().unwrap_or("").trim().to_owned();
                let client_history = payload["history"].as_array().cloned().unwrap_or_default();
                let session_id = payload["session_id"]
                    .as_str()
                    .map(str::to_owned)
                    .filter(|id| !id.is_empty())
                    .unwrap_or_else(session::new_session_id);
                if user.is_empty() {
                    json_err(400, "空消息")
                } else {
                    let mut prior = session::derive_messages(workspace_path, &session_id)
                        .unwrap_or_default();
                    if prior.is_empty() && !client_history.is_empty() {
                        prior = client_history;
                    }
                    let prev_len = prior.len();
                    let creation = runtime.creation_enabled();
                    let mut turn = agent_loop.run_turn(TurnRequest {
                        llm,
                        tools,
                        workspace,
                        doc_path: Some(&doc.path()),
                        creation,
                        user: &user,
                        history: &prior,
                    });
                    turn.session_id = Some(session_id.clone());
                    if turn.history.len() > prev_len {
                        let _ = session::append_messages(
                            workspace_path,
                            &session_id,
                            &turn.history[prev_len..],
                        );
                    }
                    json_ok(serde_json::to_value(turn).unwrap_or(json!({})))
                }
            }
            _ => json_err(404, "没有这条路"),
        };

        let _ = request.respond(response);
    }
}

fn parse_query(url: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Some(query) = url.split('?').nth(1) else {
        return map;
    };
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            map.insert(key.to_owned(), value.to_owned());
        }
    }
    map
}

fn html(body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_string(body);
    if let Ok(header) = Header::from_bytes(b"Content-Type", b"text/html; charset=utf-8") {
        response = response.with_header(header);
    }
    response
}

fn json_ok(value: Value) -> Response<std::io::Cursor<Vec<u8>>> {
    json_status(200, value)
}

fn json_err(code: u16, message: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    json_status(code, json!({"error": message}))
}

fn json_status(code: u16, value: Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_string(value.to_string()).with_status_code(StatusCode(code));
    if let Ok(header) = Header::from_bytes(b"Content-Type", b"application/json; charset=utf-8") {
        response = response.with_header(header);
    }
    response
}
