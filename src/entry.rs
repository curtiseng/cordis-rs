use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// 配置树里的一行。
///
/// `id` 是这一行的稳定身份：patch 层按 id 定位，对账也按 id 判断「同一行」。
/// `name` 是注册表里的键。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// 交给组件构造器的配置值。缺省是 `null`，由构造器决定如何解释。
    #[serde(default)]
    pub config: Value,
    /// 这一行是否被关掉。关掉的行仍在配置里，只是不被实例化。
    #[serde(default)]
    pub disabled: bool,
}

impl Entry {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Entry {
            id: id.into(),
            name: name.into(),
            config: Value::Null,
            disabled: false,
        }
    }

    pub fn with_config(mut self, config: Value) -> Self {
        self.config = config;
        self
    }

    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }
}

/// patch 层里的一条。
///
/// 形状照 dsh 的 `cordis.patch.yml`：要么按 `id` 定位一行改它，要么用 `insert`
/// 追加新行。两者互斥。
///
/// 一处刻意保留的语义：`config` 存在时**整体替换**目标行的 config，没改的字段
/// 也要重述。这让一层 patch 读起来就是它生效后的完整值，代价是啰嗦。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default)]
    pub disabled: Option<bool>,
    #[serde(default)]
    pub insert: Option<Vec<Entry>>,
    /// 期望目标行的 `name`。
    ///
    /// 这是一条**断言而不是赋值**：不匹配就跳过这条 patch 并留一条警告。照 dsh 的
    /// 设计，理由是一层 patch 可能是为另一套组合写的，而 id 撞车时静默地重配了
    /// 另一个插件，比这条 patch 不生效危险得多。
    ///
    /// 所以换实现不是改 `name`，而是把旧行 `disabled` 掉再 `insert` 新行。
    #[serde(default)]
    pub name: Option<String>,
}

impl Patch {
    /// 按 id 定位一行，替换它的整个 config。
    pub fn config(id: impl Into<String>, config: Value) -> Self {
        Patch {
            id: Some(id.into()),
            config: Some(config),
            disabled: None,
            insert: None,
            name: None,
        }
    }

    /// 按 id 定位一行，开或关它。
    pub fn set_disabled(id: impl Into<String>, disabled: bool) -> Self {
        Patch {
            id: Some(id.into()),
            config: None,
            disabled: Some(disabled),
            insert: None,
            name: None,
        }
    }

    /// 追加若干新行。
    pub fn insert(entries: Vec<Entry>) -> Self {
        Patch {
            id: None,
            config: None,
            disabled: None,
            insert: Some(entries),
            name: None,
        }
    }

    /// 断言目标行的 `name`，不匹配则这条 patch 不生效。
    pub fn expecting(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// 叠加之后的结果。
#[derive(Clone, Debug, Default)]
pub struct Composed {
    pub entries: Vec<Entry>,
    /// 指向不存在的 id 的 patch。这**不是**错误：一层 patch 可能是为另一套
    /// 组合写的，为此让整个应用起不来是过度反应。dsh 在这里写一行 stderr 警告，
    /// 这里把它交回调用方处置。
    pub warnings: Vec<String>,
}

/// 把若干 patch 层依次叠加到基础条目上。
///
/// 层的顺序就是优先级：后面的层看到的是前面各层已经生效后的树。
pub fn compose(base: &[Entry], layers: &[Vec<Patch>]) -> Result<Composed> {
    let mut entries = base.to_vec();
    check_unique(&entries)?;
    let mut warnings = Vec::new();

    for (index, layer) in layers.iter().enumerate() {
        for patch in layer {
            match (&patch.id, &patch.insert) {
                (Some(_), Some(_)) => {
                    return Err(Error::Config(format!(
                        "第 {} 层的 patch 同时给了 id 和 insert，二者互斥",
                        index + 1
                    )));
                }
                (None, None) => {
                    return Err(Error::Config(format!(
                        "第 {} 层有一条 patch 既没有 id 也没有 insert",
                        index + 1
                    )));
                }
                (Some(id), None) => {
                    match entries.iter_mut().find(|entry| &entry.id == id) {
                        Some(entry) => {
                            // `name` 是断言：对不上就整条跳过，而不是改掉它。
                            if let Some(expected) = &patch.name
                                && expected != &entry.name
                            {
                                warnings.push(format!(
                                    "第 {} 层的 patch 断言 {id} 是 {expected}，实际是 {}，已跳过",
                                    index + 1,
                                    entry.name
                                ));
                                continue;
                            }
                            if let Some(config) = &patch.config {
                                entry.config = config.clone();
                            }
                            if let Some(disabled) = patch.disabled {
                                entry.disabled = disabled;
                            }
                        }
                        // 定位不到就是一条警告：见 `Composed::warnings`。
                        None => warnings.push(format!(
                            "第 {} 层的 patch 指向了不存在的 id：{id}",
                            index + 1
                        )),
                    }
                }
                (None, Some(inserted)) => {
                    for entry in inserted {
                        if entries.iter().any(|existing| existing.id == entry.id) {
                            return Err(Error::Config(format!(
                                "第 {} 层 insert 的 id 与已有行重复：{}",
                                index + 1,
                                entry.id
                            )));
                        }
                        entries.push(entry.clone());
                    }
                }
            }
        }
    }

    Ok(Composed { entries, warnings })
}

pub(crate) fn check_unique(entries: &[Entry]) -> Result<()> {
    for (index, entry) in entries.iter().enumerate() {
        if entries[..index].iter().any(|prior| prior.id == entry.id) {
            return Err(Error::Config(format!("id 重复：{}", entry.id)));
        }
    }
    Ok(())
}

/// 解析一份条目列表（`cordis.yml`）。
pub fn parse_entries(yaml: &str) -> Result<Vec<Entry>> {
    parse_list(yaml, "条目列表")
}

/// 解析一层 patch（`cordis.patch.yml`）。
pub fn parse_patches(yaml: &str) -> Result<Vec<Patch>> {
    parse_list(yaml, "patch 层")
}

/// 顶层必须是数组。
///
/// 空文件或只有注释的文件解析出 `null`，这里报错而不是当成空列表——照 dsh 的
/// 判断：那通常是写坏了，而「这一层什么都不做」有明确的写法 `[]`。
fn parse_list<T: serde::de::DeserializeOwned>(yaml: &str, what: &str) -> Result<Vec<T>> {
    let value: Value = serde_yaml_ng::from_str(yaml)
        .map_err(|error| Error::Config(format!("{what} 解析失败：{error}")))?;
    if value.is_null() {
        return Err(Error::Config(format!(
            "{what} 必须是一个顶层数组；要让这一层什么都不做，请写 []"
        )));
    }
    serde_json::from_value(value)
        .map_err(|error| Error::Config(format!("{what} 形状不对：{error}")))
}
