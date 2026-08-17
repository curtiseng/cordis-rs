use serde_json::{Value, json};

pub fn function_schema(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters,
        }
    })
}

pub fn text_query(description: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": description }
        }
    })
}

pub fn empty_object() -> Value {
    json!({
        "type": "object",
        "properties": {}
    })
}

pub fn read_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "工作区内的相对或绝对路径" }
        },
        "required": ["path"]
    })
}

pub fn write_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "工作区内的相对或绝对路径" },
            "content": { "type": "string", "description": "要写入的全文" }
        },
        "required": ["path", "content"]
    })
}

pub fn edit_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "工作区内的相对或绝对路径" },
            "old": { "type": "string", "description": "要被替换的原文片段" },
            "new": { "type": "string", "description": "替换后的文本" }
        },
        "required": ["path", "old", "new"]
    })
}

pub fn bash_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": { "type": "string", "description": "要执行的 shell 命令" },
            "cwd": { "type": "string", "description": "可选，工作区内的起始目录" }
        },
        "required": ["command"]
    })
}

pub fn define_script_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": "插件 id，也是文件名 stem" },
            "source": { "type": "string", "description": "完整 JavaScript 源码（含 load/unload）" },
            "grant": {
                "type": "array",
                "items": { "type": "string" },
                "description": "可选，授予的 capability 名，如 markdown"
            },
            "role": { "type": "string", "description": "可选，插件说明" }
        },
        "required": ["id", "source"]
    })
}

pub fn id_only_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": "配置行 id" }
        },
        "required": ["id"]
    })
}

pub fn save_patch_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "可选，输出路径，默认 cordis.patch.yml" }
        }
    })
}

pub fn inspect_config_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "include_config": { "type": "boolean", "description": "是否包含每行的 config 字段" }
        }
    })
}
