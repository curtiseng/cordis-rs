//! 配置文件的解析与 patch 层的叠加。
//!
//! 形状照 dsh 的 `cordis.yml` + `cordis.patch.yml`：一份基础条目列表，外加若干层
//! 按 id 定位的 patch。这一层完全是纯函数——把「算出期望状态」与「把期望状态施加
//! 到活着的树上」分开，是能对配置做静态检查的前提。

use cordis::{Entry, Error, Patch, compose, parse_entries, parse_patches};
use serde_json::json;

const BASE: &str = r#"
- id: timer
  name: cordis-plugin-timer

- id: llm
  name: dsh-llm
  config:
    stream: true
    timeoutMs: 60000

- id: tool-web
  name: dsh-tool-web
  config:
    fetch: false
"#;

#[test]
fn parses_a_dsh_shaped_entry_list() {
    let entries = parse_entries(BASE).expect("应当解析");

    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].id, "timer");
    assert_eq!(entries[1].config["timeoutMs"], json!(60000));
    assert!(!entries[2].disabled, "没写 disabled 就是开着");
    assert_eq!(entries[0].config, json!(null), "没写 config 就是 null");
}

/// 按 id 的 patch 替换目标行的**整个** config。
///
/// 没改的字段也要重述。啰嗦是故意的：一层 patch 读起来就是它生效后的完整值，
/// 不需要读者去脑内合并。
#[test]
fn a_patch_replaces_the_whole_config() {
    let base = parse_entries(BASE).unwrap();
    let layer = parse_patches(
        r#"
- id: tool-web
  config:
    fetch: true
    searchTimeoutMs: 60000
"#,
    )
    .unwrap();

    let composed = compose(&base, &[layer]).expect("应当叠加");
    let row = composed
        .entries
        .iter()
        .find(|entry| entry.id == "tool-web")
        .unwrap();

    assert_eq!(
        row.config,
        json!({ "fetch": true, "searchTimeoutMs": 60000 })
    );
    assert!(composed.warnings.is_empty());
}

/// 层的顺序就是优先级：后面的层看到前面各层的结果。
#[test]
fn later_layers_win() {
    let base = vec![Entry::new("llm", "dsh-llm").with_config(json!({ "model": "a" }))];
    let first = vec![Patch::config("llm", json!({ "model": "b" }))];
    let second = vec![Patch::config("llm", json!({ "model": "c" }))];

    let composed = compose(&base, &[first, second]).unwrap();

    assert_eq!(composed.entries[0].config, json!({ "model": "c" }));
}

/// `disabled` 与 `config` 是两件独立的事，可以分层写。
#[test]
fn disabling_and_reconfiguring_compose_independently() {
    let base = parse_entries(BASE).unwrap();
    let layer = parse_patches(
        r#"
- id: timer
  disabled: true

- id: llm
  config:
    stream: false
    timeoutMs: 5000
"#,
    )
    .unwrap();

    let composed = compose(&base, &[layer]).unwrap();

    assert!(composed.entries[0].disabled);
    assert_eq!(composed.entries[1].config["timeoutMs"], json!(5000));
    assert!(!composed.entries[2].disabled, "别的行不该被牵连");
}

/// `insert` 追加新行。
#[test]
fn insert_appends_rows() {
    let base = parse_entries(BASE).unwrap();
    let layer = parse_patches(
        r#"
- insert:
    - id: tool-lsp
      name: dsh-tool-lsp
    - id: mcp
      name: dsh-mcp-client
      config:
        servers: []
"#,
    )
    .unwrap();

    let composed = compose(&base, &[layer]).unwrap();

    let ids: Vec<&str> = composed
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(ids, ["timer", "llm", "tool-web", "tool-lsp", "mcp"]);
}

/// 指向不存在的 id 只是一条警告。
///
/// 一层 patch 可能是为另一套组合写的；为此让整个应用起不来是过度反应。
#[test]
fn a_patch_on_a_missing_id_is_only_a_warning() {
    let base = parse_entries(BASE).unwrap();
    let layer = vec![Patch::set_disabled("并不存在的行", true)];

    let composed = compose(&base, &[layer]).expect("不该是错误");

    assert_eq!(composed.entries.len(), 3);
    assert_eq!(composed.warnings.len(), 1);
    assert!(composed.warnings[0].contains("并不存在的行"));
}

/// patch 里的 `name` 是断言而不是赋值：对不上就整条跳过。
///
/// 照 dsh 的设计。理由是一层 patch 可能是为另一套组合写的，而 id 撞车时静默地
/// 重配了另一个插件，比这条 patch 不生效危险得多。换实现的写法是关旧行加插新行。
#[test]
fn a_name_in_a_patch_is_an_assertion_not_an_assignment() {
    let base = parse_entries(BASE).unwrap();
    let layer = parse_patches(
        r#"
- id: tool-web
  name: 别人家的包
  config:
    fetch: true
"#,
    )
    .unwrap();

    let composed = compose(&base, &[layer]).unwrap();
    let row = composed
        .entries
        .iter()
        .find(|entry| entry.id == "tool-web")
        .unwrap();

    assert_eq!(row.name, "dsh-tool-web", "name 不会被改写");
    assert_eq!(row.config, json!({ "fetch": false }), "整条 patch 未生效");
    assert_eq!(composed.warnings.len(), 1);
    assert!(composed.warnings[0].contains("已跳过"));
}

/// 断言对得上时，patch 照常生效。
#[test]
fn a_matching_name_lets_the_patch_through() {
    let base = parse_entries(BASE).unwrap();
    let layer = vec![Patch::config("tool-web", json!({ "fetch": true })).expecting("dsh-tool-web")];

    let composed = compose(&base, &[layer]).unwrap();

    assert_eq!(composed.entries[2].config, json!({ "fetch": true }));
    assert!(composed.warnings.is_empty());
}

/// 同一层里，后面的 patch 能定位到前面的 patch 插入的行。
#[test]
fn a_later_patch_can_target_an_inserted_row() {
    let base = parse_entries(BASE).unwrap();
    let layer = parse_patches(
        r#"
- insert:
    - id: mcp
      name: dsh-mcp-client

- id: mcp
  config:
    servers: [github]
"#,
    )
    .unwrap();

    let composed = compose(&base, &[layer]).unwrap();
    let row = composed.entries.last().unwrap();

    assert_eq!(row.id, "mcp");
    assert_eq!(row.config, json!({ "servers": ["github"] }));
    assert!(composed.warnings.is_empty());
}

/// insert 的 id 与已有行重复则是错误：重复的 id 无法被定位。
#[test]
fn a_duplicate_insert_is_rejected() {
    let base = parse_entries(BASE).unwrap();
    let layer = vec![Patch::insert(vec![Entry::new("llm", "别的包")])];

    let error = compose(&base, &[layer]).expect_err("应当被拒");

    assert!(
        matches!(&error, Error::Config(message) if message.contains("重复")),
        "得到的是 {error}"
    );
}

/// 空文件与只有注释的文件都被拒绝。
///
/// 它们解析出 `null`，几乎总是写坏了。「这一层什么都不做」有明确写法：`[]`。
#[test]
fn an_empty_layer_must_be_written_as_an_empty_array() {
    for text in ["", "# 只有注释\n"] {
        let error = parse_patches(text).expect_err("应当被拒");
        assert!(
            matches!(&error, Error::Config(message) if message.contains("[]")),
            "得到的是 {error}"
        );
    }

    assert!(parse_patches("[]").unwrap().is_empty());
}

/// 拼错的字段名不被静默忽略。
#[test]
fn a_misspelled_field_is_rejected() {
    let error = parse_entries("- id: a\n  name: b\n  conifg: {}\n").expect_err("应当被拒");

    assert!(
        matches!(&error, Error::Config(message) if message.contains("conifg")),
        "得到的是 {error}"
    );
}

/// 一条 patch 不能既定位又插入。
#[test]
fn a_patch_cannot_both_target_and_insert() {
    let base = parse_entries(BASE).unwrap();
    let layer = parse_patches("- id: llm\n  insert: []\n").unwrap();

    let error = compose(&base, &[layer]).expect_err("应当被拒");

    assert!(
        matches!(&error, Error::Config(message) if message.contains("互斥")),
        "得到的是 {error}"
    );
}
