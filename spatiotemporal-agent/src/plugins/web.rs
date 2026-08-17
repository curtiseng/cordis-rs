use std::rc::Rc;

use futures::future::LocalBoxFuture;
use serde_json::{Value, json};
use spatiotemporal::{Component, Context, KeyId, Result, Steps};
use tiny_http::{Header, Method, Response, Server, StatusCode};

use crate::approval::ApprovalQueue;
use crate::chat;
use crate::host::{Document, Llm, Roster, Surface, Toolbox};
use crate::keys::{Doc, FsKey, LlmKey, SurfaceKey};
use crate::runtime::AgentRuntime;

const INDEX: &str = include_str!("../../assets/index.html");

/// 本地插件：浏览器界面。`apply` 只提供 `surface`，听端口发生在 `run`。
pub struct Web {
    pub port: u16,
    pub tools: Toolbox,
    pub roster: Roster,
    pub creation: bool,
    pub approvals: ApprovalQueue,
    pub runtime: Rc<AgentRuntime>,
}

struct WebSurface {
    port: u16,
    tools: Toolbox,
    roster: Roster,
    llm: Rc<dyn Llm>,
    doc: Rc<dyn Document>,
    workspace: String,
    creation: bool,
    approvals: ApprovalQueue,
    runtime: Rc<AgentRuntime>,
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
            doc: &*self.doc,
            workspace: &self.workspace,
            creation: self.creation,
            approvals: &self.approvals,
            runtime: &self.runtime,
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
        ]
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let port = self.port;
        let tools = self.tools.clone();
        let roster = self.roster.clone();
        let creation = self.creation;
        let approvals = self.approvals.clone();
        let runtime = self.runtime.clone();
        Box::pin(async move {
            let doc = ctx.resolve::<Doc>()?;
            let llm = ctx.resolve::<LlmKey>()?;
            let fs = ctx.resolve::<FsKey>()?;
            let workspace = fs.root().display().to_string();
            let surface: Rc<dyn Surface> = Rc::new(WebSurface {
                port,
                tools,
                roster,
                llm,
                doc,
                workspace,
                creation,
                approvals,
                runtime,
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
    doc: &'a dyn Document,
    workspace: &'a str,
    creation: bool,
    approvals: &'a ApprovalQueue,
    runtime: &'a AgentRuntime,
    kind: &'a str,
}

fn serve(ctx: ServeContext<'_>) {
    let ServeContext {
        port,
        tools,
        roster,
        llm,
        doc,
        workspace,
        creation,
        approvals,
        runtime,
        kind,
    } = ctx;
    let server = Server::http(("127.0.0.1", port)).unwrap_or_else(|error| {
        panic!("听不了 {port}：{error}");
    });

    for mut request in server.incoming_requests() {
        let url = request.url().split('?').next().unwrap_or("/").to_owned();
        let method = request.method().clone();

        let response = match (method, url.as_str()) {
            (Method::Get, "/") => html(INDEX),
            (Method::Get, "/api/status") => {
                let pending = approvals.pending().map(|p| {
                    json!({
                        "id": p.id,
                        "file": p.file,
                        "role": p.role,
                        "source_lines": p.source_lines,
                        "preview": p.preview,
                    })
                });
                json_ok(json!({
                    "doc": doc.path(),
                    "model": llm.model(),
                    "has_key": std::env::var("DEEPSEEK_API_KEY").is_ok(),
                    "surface": kind,
                    "creation": creation,
                    "workspace": workspace,
                    "pending": pending,
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
            (Method::Get, "/api/creation/pending") => {
                let pending = approvals.pending().map(|p| {
                    json!({
                        "id": p.id,
                        "file": p.file,
                        "role": p.role,
                        "source_lines": p.source_lines,
                        "preview": p.preview,
                    })
                });
                json_ok(json!({ "pending": pending }))
            }
            (Method::Post, "/api/creation/approve") => {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let payload: Value = serde_json::from_str(&body).unwrap_or(json!({}));
                let approve = payload["approve"].as_bool().unwrap_or(false);
                let result = if approve {
                    approvals.approve(runtime)
                } else {
                    approvals.reject()
                };
                match result {
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
                let history = payload["history"].as_array().cloned().unwrap_or_default();
                if user.is_empty() {
                    json_err(400, "空消息")
                } else {
                    let turn = chat::run(chat::ChatConfig {
                        llm,
                        tools,
                        workspace,
                        doc_path: Some(&doc.path()),
                        creation,
                        user: &user,
                        history: &history,
                    });
                    json_ok(serde_json::to_value(turn).unwrap_or(json!({})))
                }
            }
            _ => json_err(404, "没有这条路"),
        };

        let _ = request.respond(response);
    }
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
