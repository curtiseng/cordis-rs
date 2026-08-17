use std::collections::HashMap;

use spatiotemporal::{Context, Error, Key, KeyId, KeyRegistry, Result};

/// 「从上下文里取出一项能力，并投影成 guest 收得下的形状」。
type Projection = Box<dyn Fn(&Context) -> Result<String>>;

/// 宿主愿意让子进程 guest 看见的能力面。
///
/// 形状与 `spatiotemporal-wasm` / `spatiotemporal-script` 的同名类型一致。
pub struct Capabilities {
    keys: KeyRegistry,
    projections: HashMap<String, Projection>,
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities::new()
    }
}

impl Capabilities {
    pub fn new() -> Self {
        Capabilities {
            keys: KeyRegistry::new(),
            projections: HashMap::new(),
        }
    }

    /// 暴露一项能力：登记它的键，并给出怎么把它投影成 guest 能收到的字符串。
    pub fn expose<K, F>(&mut self, project: F) -> &mut Self
    where
        K: Key,
        F: Fn(&K::Api) -> String + 'static,
    {
        self.keys.add::<K>();
        self.projections.insert(
            K::NAME.to_owned(),
            Box::new(move |ctx: &Context| {
                let api = ctx.resolve::<K>()?;
                Ok(project(&api))
            }),
        );
        self
    }

    /// 把 guest 侧的名字翻成可填进 `Component::inject` 的键。
    pub fn inject(&self, granted: &[String]) -> Result<Vec<KeyId>> {
        self.keys.resolve_all(granted)
    }

    /// 已暴露的能力名，排序后返回。
    pub fn names(&self) -> Vec<&str> {
        self.keys.names()
    }

    /// 在加载时刻取一次能力快照。
    pub(crate) fn snapshot(
        &self,
        ctx: &Context,
        granted: &[String],
    ) -> Result<HashMap<String, String>> {
        granted
            .iter()
            .map(|name| {
                let project = self
                    .projections
                    .get(name)
                    .ok_or_else(|| Error::UnknownKey(name.clone()))?;
                Ok((name.clone(), project(ctx)?))
            })
            .collect()
    }
}
