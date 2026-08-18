use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use spatiotemporal::{Error, Result};

/// 默认等 guest 回应一行 JSON 的上限。
pub const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(30);

pub fn load_request(id: u64, capabilities: &serde_json::Map<String, Value>) -> Value {
    json!({
        "id": id,
        "op": "load",
        "capabilities": capabilities,
    })
}

pub fn invoke_request(id: u64, name: &str, args: &str) -> Value {
    json!({
        "id": id,
        "op": "invoke",
        "name": name,
        "args": args,
    })
}

pub fn unload_request(id: u64) -> Value {
    json!({
        "id": id,
        "op": "unload",
    })
}

pub struct Session<R: Read + Send + 'static> {
    stdin: Arc<Mutex<Box<dyn Write + Send>>>,
    stdout: Arc<Mutex<BufReader<R>>>,
    next_id: u64,
    io_timeout: Duration,
}

impl<R: Read + Send + 'static> Session<R> {
    pub fn new(stdin: impl Write + Send + 'static, stdout: R, io_timeout: Duration) -> Self {
        Session {
            stdin: Arc::new(Mutex::new(Box::new(stdin))),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            next_id: 1,
            io_timeout,
        }
    }

    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = id.saturating_add(1);
        id
    }

    pub fn transact(&mut self, request: Value) -> Result<Value> {
        self.transact_with_timeout(request, self.io_timeout)
    }

    pub fn transact_with_timeout(&mut self, request: Value, timeout: Duration) -> Result<Value> {
        let id = request
            .get("id")
            .and_then(Value::as_u64)
            .unwrap_or(self.next_id);
        self.next_id = id.saturating_add(1);

        let line = serde_json::to_string(&request)
            .map_err(|error| Error::Component(format!("编不出请求 JSON：{error}")))?;
        {
            let mut stdin = self.stdin.lock().map_err(lock_error)?;
            stdin
                .write_all(line.as_bytes())
                .map_err(io_error("写 stdin"))?;
            stdin.write_all(b"\n").map_err(io_error("写 stdin"))?;
            stdin.flush().map_err(io_error("刷 stdin"))?;
        }

        read_json_line(self.stdout.clone(), timeout)
    }
}

fn read_json_line<R: Read + Send + 'static>(
    stdout: Arc<Mutex<BufReader<R>>>,
    timeout: Duration,
) -> Result<Value> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = (|| {
            let mut line = String::new();
            let read = {
                let mut reader = stdout.lock().map_err(lock_error)?;
                reader.read_line(&mut line)
            };
            match read {
                Ok(0) => Err(Error::Component("guest 提前 EOF".into())),
                Ok(_) => serde_json::from_str(&line).map_err(|error| {
                    Error::Component(format!("guest 回了非法 JSON：{error}；行={line:?}"))
                }),
                Err(error) => Err(io_error("读 stdout")(error)),
            }
        })();
        tx.send(outcome).ok();
    });

    rx.recv_timeout(timeout)
        .map_err(|_| Error::Component(format!("等 guest 回应超时（>{timeout:?}）")))?
}

pub fn expect_ok(response: &Value) -> Result<()> {
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        let message = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("guest 失败");
        Err(Error::Component(message.to_owned()))
    }
}

pub fn take_logs(response: &Value) -> Vec<String> {
    response
        .get("logs")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub fn take_tools(response: &Value) -> Vec<(String, String)> {
    response
        .get("tools")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item.get("name")?.as_str()?.to_owned();
                    let description = item
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    Some((name, description))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn take_llm(response: &Value) -> Option<String> {
    response.get("llm").and_then(|value| match value {
        Value::Null => None,
        Value::String(text) if text.is_empty() => None,
        Value::String(text) => Some(text.clone()),
        _ => None,
    })
}

pub fn take_result(response: &Value) -> Result<String> {
    expect_ok(response)?;
    response
        .get("result")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Component("guest 没给 result".into()))
}

fn io_error(context: &str) -> impl FnOnce(std::io::Error) -> Error + use<'_> {
    move |error| Error::Component(format!("{context}：{error}"))
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> Error {
    Error::Component("子进程会话锁中毒".into())
}
