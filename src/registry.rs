use std::collections::HashMap;
use std::rc::Rc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::component::Component;
use crate::error::{Error, Result};

/// 从一份配置值构造一个组件。
pub type Factory = Box<dyn Fn(&Value) -> Result<Rc<dyn Component>>>;

/// 名字到构造器的表。
///
/// 论文 5.2.1 节的 Loader 用宿主语言的动态 `import()` 把配置里的包名变成组件。
/// Rust 没有运行时模块注册表——6.4 节把这一条列为原生语言的固有差异——所以这张
/// 表必须显式建立。
///
/// 交换条件值得写清楚：**加一个新组件要动宿主一行代码并重新编译**，但配置层
/// 完全不需要重新编译。已注册组件的开关、重配、插入、移除都由
/// [`Loader`](crate::Loader) 在运行中完成，这正是配置热重载所要的全部。
///
/// ```
/// use spatiotemporal::{Component, Context, Registry, Result, Steps};
/// use futures::future::LocalBoxFuture;
///
/// #[derive(serde::Deserialize)]
/// struct Echo {
///     #[serde(default)]
///     times: usize,
/// }
///
/// impl Component for Echo {
///     fn name(&self) -> &'static str { "echo" }
///     fn apply(&self, _ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
///         Box::pin(async { Ok(()) })
///     }
/// }
///
/// let mut registry = Registry::new();
/// registry.add_de::<Echo>("echo");
/// assert!(registry.contains("echo"));
/// ```
#[derive(Default)]
pub struct Registry {
    factories: HashMap<String, Factory>,
}

impl Registry {
    pub fn new() -> Self {
        Registry::default()
    }

    /// 登记一个构造器。
    pub fn add<F>(&mut self, name: impl Into<String>, factory: F) -> &mut Self
    where
        F: Fn(&Value) -> Result<Rc<dyn Component>> + 'static,
    {
        self.factories.insert(name.into(), Box::new(factory));
        self
    }

    /// 登记一个直接从配置反序列化出来的组件。
    ///
    /// 缺省的 `config`（YAML 里那一行没写 `config:`）被当作空对象，因此组件的
    /// `#[serde(default)]` 字段照常生效——对应 dsh「省略即保留 schema 默认值」
    /// 的约定。
    ///
    /// 这里也是 Rust 相对 TypeScript 的一处便宜：配置校验发生在解析期，且是
    /// **同步**的，所以不合法的配置在任何 fiber 被改动之前就被拒绝。
    pub fn add_de<T>(&mut self, name: impl Into<String>) -> &mut Self
    where
        T: Component + DeserializeOwned,
    {
        let label: String = name.into();
        let diagnostic = label.clone();
        self.add(label, move |config: &Value| {
            let config = if config.is_null() {
                Value::Object(Default::default())
            } else {
                config.clone()
            };
            let component: T = serde_json::from_value(config)
                .map_err(|error| Error::Config(format!("{diagnostic}：{error}")))?;
            Ok(Rc::new(component) as Rc<dyn Component>)
        })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.factories.contains_key(name)
    }

    /// 已登记的名字，排序后返回（便于诊断输出稳定）。
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.factories.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    pub(crate) fn build(&self, name: &str, config: &Value) -> Result<Rc<dyn Component>> {
        let factory = self
            .factories
            .get(name)
            .ok_or_else(|| Error::Unknown(name.to_owned()))?;
        factory(config)
    }
}
