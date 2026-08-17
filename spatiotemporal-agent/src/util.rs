use std::path::{Path, PathBuf};

/// 默认工作区：环境变量 `WORKSPACE`，否则当前工作目录。
pub fn default_workspace_root() -> PathBuf {
    std::env::var("WORKSPACE")
        .map(PathBuf::from)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 在 workspace 根目录内解析路径，拒绝逃逸。
pub fn resolve_within(root: &Path, user_path: &str) -> spatiotemporal::Result<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|error| spatiotemporal::Error::Component(format!("工作区无效：{error}")))?;
    let joined = if Path::new(user_path).is_absolute() {
        PathBuf::from(user_path)
    } else {
        root.join(user_path)
    };
    let canonical = joined.canonicalize().map_err(|error| {
        spatiotemporal::Error::Component(format!("路径不存在或不可访问：{user_path} ({error})"))
    })?;
    if !canonical.starts_with(&root) {
        return Err(spatiotemporal::Error::Component(format!(
            "路径在工作区外：{user_path}"
        )));
    }
    Ok(canonical)
}

pub fn workspace_root(config: &spatiotemporal::Value) -> PathBuf {
    match config.get("root").and_then(spatiotemporal::Value::as_str) {
        None | Some(".") | Some("") => default_workspace_root(),
        Some(path) => {
            let candidate = Path::new(path);
            if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                default_workspace_root().join(candidate)
            }
        }
    }
}

pub fn parse_json_args(args: &str) -> spatiotemporal::Result<serde_json::Value> {
    if args.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(args).map_err(|error| {
        spatiotemporal::Error::Component(format!("参数不是合法 JSON：{error}"))
    })
}

pub fn arg_str<'a>(value: &'a serde_json::Value, key: &str) -> spatiotemporal::Result<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| spatiotemporal::Error::Component(format!("缺少字段 `{key}`")))
}
