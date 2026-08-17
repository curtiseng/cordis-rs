use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use spatiotemporal::App;
use tiny_http::{Header, Method, Response, Server, StatusCode};

use crate::chat;
use crate::plugins::{Toolbox, lookup_doc, lookup_llm};

const INDEX: &str = include_str!("../assets/index.html");

pub fn serve(port: u16, doc_path: PathBuf, tools: Toolbox, app: App) {
    let llm = lookup_llm(&app.root()).expect("deepseek 没装上");
    let doc = lookup_doc(&app.root()).expect("文档没装上");
    let server = Server::http(("127.0.0.1", port)).unwrap_or_else(|error| {
        panic!("听不了 {port}：{error}");
    });
    let turns = AtomicU64::new(0);

    for mut request in server.incoming_requests() {
        let url = request.url().split('?').next().unwrap_or("/").to_owned();
        let method = request.method().clone();

        let response = match (method, url.as_str()) {
            (Method::Get, "/") => html(INDEX),
            (Method::Get, "/api/status") => json_ok(json!({
                "doc": doc.path(),
                "model": llm.model(),
                "has_key": std::env::var("DEEPSEEK_API_KEY").is_ok(),
                "tools": tools.list().into_iter().map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "substrate": t.substrate,
                })).collect::<Vec<_>>(),
                "plugins": [
                    {"name": "doc", "substrate": "native", "role": "提供 markdown 能力"},
                    {"name": "deepseek", "substrate": "native", "role": "调用 DeepSeek LLM"},
                    {"name": "read_doc", "substrate": "native", "role": "登记读全文工具"},
                    {"name": "outline", "substrate": "wasm", "role": "抽标题大纲"},
                    {"name": "cite", "substrate": "script", "role": "按关键词引用原文"},
                ],
            })),
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
                    turns.fetch_add(1, Ordering::Relaxed);
                    let turn = chat::run(
                        &*llm,
                        &tools,
                        &doc_path.display().to_string(),
                        &user,
                        &history,
                    );
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
