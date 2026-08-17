use std::fs;
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, KeyId, Result, Steps};

use crate::host::Toolbox;
use crate::keys::FsKey;
use crate::tool_schema;
use crate::util::{arg_str, parse_json_args, resolve_within};

/// 本地插件：read / write / edit 文件工具。
pub struct ToolFs {
    pub tools: Toolbox,
}

impl Component for ToolFs {
    fn name(&self) -> &str {
        "tool-fs"
    }

    fn inject(&self) -> Vec<KeyId> {
        vec![KeyId::of::<FsKey>()]
    }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let tools = self.tools.clone();
        Box::pin(async move {
            let fs = ctx.resolve::<FsKey>()?;
            let root = fs.root().to_path_buf();

            let root_read = root.clone();
            register(
                &tools,
                "read",
                "读取工作区内的文本文件",
                tool_schema::read_schema(),
                move |args| {
                let value = parse_json_args(args)?;
                let path = arg_str(&value, "path")?;
                let bytes = fs::read(resolve_within(&root_read, path)?).map_err(map_io)?;
                Ok(String::from_utf8_lossy(&bytes).into_owned())
                },
            );

            let root_write = root.clone();
            register(
                &tools,
                "write",
                "写入或覆盖工作区内的文本文件",
                tool_schema::write_schema(),
                move |args| {
                    let value = parse_json_args(args)?;
                    let path = arg_str(&value, "path")?;
                    let content = arg_str(&value, "content")?;
                    let target = resolve_within(&root_write, path)?;
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent).map_err(map_io)?;
                    }
                    fs::write(&target, content).map_err(map_io)?;
                    Ok(format!(
                        "已写入 {}（{} 字节）",
                        target.display(),
                        content.len()
                    ))
                },
            );

            let root_edit = root;
            register(
                &tools,
                "edit",
                "在文件里把 old 替换成 new（全文件一次替换）",
                tool_schema::edit_schema(),
                move |args| {
                    let value = parse_json_args(args)?;
                    let path = arg_str(&value, "path")?;
                    let old = arg_str(&value, "old")?;
                    let new = arg_str(&value, "new")?;
                    let target = resolve_within(&root_edit, path)?;
                    let text = fs::read_to_string(&target).map_err(map_io)?;
                    if !text.contains(old) {
                        return Err(spatiotemporal::Error::Component(format!(
                            "文件 {} 里没有找到要替换的片段",
                            target.display()
                        )));
                    }
                    let updated = text.replacen(old, new, 1);
                    fs::write(&target, &updated).map_err(map_io)?;
                    Ok(format!("已编辑 {}", target.display()))
                },
            );

            let tools = tools.clone();
            steps.step_sync(move || {
                for name in ["read", "write", "edit"] {
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
    parameters: spatiotemporal::Value,
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

fn map_io(error: std::io::Error) -> spatiotemporal::Error {
    spatiotemporal::Error::Component(error.to_string())
}
