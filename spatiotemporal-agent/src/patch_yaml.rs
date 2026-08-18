use spatiotemporal::{Entry, Patch, Result, parse_patches};

/// 会话 patch 栈（每层对应一次 `push_layer`）的 JSON 持久化。
pub fn render_layer_stack(layers: &[Vec<Patch>]) -> Result<String> {
    let value: Vec<Vec<serde_json::Value>> = layers
        .iter()
        .map(|layer| layer.iter().map(patch_to_value).collect())
        .collect();
    serde_json::to_string_pretty(&value).map_err(|error| {
        spatiotemporal::Error::Component(format!("session patch JSON 序列化失败：{error}"))
    })
}

pub fn parse_layer_stack(text: &str) -> Result<Vec<Vec<Patch>>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: Vec<Vec<serde_json::Value>> = serde_json::from_str(text).map_err(|error| {
        spatiotemporal::Error::Component(format!("session patch JSON 解析失败：{error}"))
    })?;
    value
        .into_iter()
        .map(|layer| {
            let yaml = serde_yaml_ng::to_string(&layer).map_err(|error| {
                spatiotemporal::Error::Component(format!("session patch YAML 转换失败：{error}"))
            })?;
            parse_patches(&yaml)
        })
        .collect()
}

/// 把 patch 列表写成可被 `parse_patches` 读回的 YAML。
pub fn render_patches(patches: &[Patch]) -> Result<String> {
    let value: Vec<serde_json::Value> = patches.iter().map(patch_to_value).collect();
    serde_yaml_ng::to_string(&value).map_err(|error| {
        spatiotemporal::Error::Component(format!("patch YAML 序列化失败：{error}"))
    })
}

fn patch_to_value(patch: &Patch) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    if let Some(id) = &patch.id {
        map.insert("id".into(), serde_json::Value::String(id.clone()));
    }
    if let Some(config) = &patch.config {
        map.insert("config".into(), config.clone());
    }
    if let Some(disabled) = patch.disabled {
        map.insert("disabled".into(), serde_json::Value::Bool(disabled));
    }
    if let Some(name) = &patch.name {
        map.insert("name".into(), serde_json::Value::String(name.clone()));
    }
    if let Some(insert) = &patch.insert {
        map.insert(
            "insert".into(),
            serde_json::Value::Array(insert.iter().map(entry_to_value).collect()),
        );
    }
    serde_json::Value::Object(map)
}

fn entry_to_value(entry: &Entry) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("id".into(), serde_json::Value::String(entry.id.clone()));
    map.insert("name".into(), serde_json::Value::String(entry.name.clone()));
    if entry.disabled {
        map.insert("disabled".into(), serde_json::Value::Bool(true));
    }
    if !entry.config.is_null() {
        map.insert("config".into(), entry.config.clone());
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use spatiotemporal::{Entry, Patch, parse_patches};

    use super::{parse_layer_stack, render_layer_stack, render_patches};

    #[test]
    fn layer_stack_round_trip() {
        let patches = vec![
            Patch {
                id: Some("llm".into()),
                config: None,
                disabled: Some(true),
                insert: None,
                name: None,
            },
            Patch {
                id: None,
                config: None,
                disabled: None,
                insert: Some(vec![Entry::new("echo", "script").with_config(
                    serde_json::json!({"file": "plugins/echo.js", "grant": []}),
                )]),
                name: None,
            },
        ];
        let layers = vec![patches];
        let json = render_layer_stack(&layers).expect("render");
        let parsed = parse_layer_stack(&json).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].len(), 2);
    }

    #[test]
    fn round_trip_patch_yaml() {
        let patches = vec![
            Patch {
                id: Some("llm".into()),
                config: None,
                disabled: Some(true),
                insert: None,
                name: None,
            },
            Patch {
                id: None,
                config: None,
                disabled: None,
                insert: Some(vec![Entry::new("echo", "script").with_config(
                    serde_json::json!({"file": "plugins/echo.js", "grant": []}),
                )]),
                name: None,
            },
        ];
        let yaml = render_patches(&patches).expect("render");
        let parsed = parse_patches(&yaml).expect("parse");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id.as_deref(), Some("llm"));
        assert!(parsed[0].disabled.unwrap());
        assert_eq!(parsed[1].insert.as_ref().unwrap()[0].name, "script");
    }
}
