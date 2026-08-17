use std::any::TypeId;
use std::collections::HashMap;

use crate::error::{Error, Result};

/// 一项 coeffect 的类型化键。
///
/// 对应论文 3.2.1 节的 coeffect 键 $k : K$。宿主类型只作标记，真正被
/// 提供与消费的是 [`Key::Api`]——通常是一个 trait 对象，这样同一个键才能
/// 被不同 provider 以不同实现满足（6.2 节的排他绑定）。
///
/// ```
/// use cordis::Key;
///
/// trait Database {
///     fn query(&self) -> String;
/// }
///
/// enum Db {}
/// impl Key for Db {
///     type Api = dyn Database;
///     const NAME: &'static str = "db";
/// }
/// ```
pub trait Key: 'static {
    /// 该键背后的接口。`?Sized` 是为了允许 `dyn Trait`。
    type Api: ?Sized + 'static;
    /// 诊断用的名字，出现在错误信息里。
    const NAME: &'static str;
}

/// [`Key`] 的运行时标识。
///
/// 用 [`TypeId`] 而不是字符串作为同一性依据，因此两个恰好同名的键不会互相冒充。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyId {
    type_id: TypeId,
    name: &'static str,
}

impl KeyId {
    pub fn of<K: Key>() -> Self {
        KeyId {
            type_id: TypeId::of::<K>(),
            name: K::NAME,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// 名字到 coeffect 键的表。
///
/// [`KeyId`] 的同一性依据是 [`TypeId`]，所以它**无法在运行时凭一个字符串构造
/// 出来**。这对原生组件不是问题（它们直接写 `KeyId::of::<Db>()`），但任何在
/// 编译期之外的东西——一个 wasm 组件的 WIT 导入、一段模型现写的代码——只能用
/// 名字说「我需要 tools」。这张表就是那道翻译。
///
/// 它跟 [`Registry`](crate::Registry) 是一对：那张表说「哪些组件可以被装上」，
/// 这张表说「哪些能力可以被按名字声明」。两张都由宿主显式建立，因此**宿主始终
/// 掌握着可被动态声明的能力面**——一个 guest 说不出宿主没登记的键。
///
/// ```
/// use cordis::{Key, KeyRegistry};
///
/// trait Tools {}
/// enum ToolsKey {}
/// impl Key for ToolsKey {
///     type Api = dyn Tools;
///     const NAME: &'static str = "tools";
/// }
///
/// let mut keys = KeyRegistry::new();
/// keys.add::<ToolsKey>();
///
/// // guest 报上来的字符串在这里变成可填进 `Component::inject` 的键。
/// assert_eq!(keys.resolve_all(&["tools"]).unwrap().len(), 1);
/// assert!(keys.resolve("没登记的能力").is_err());
/// ```
#[derive(Default)]
pub struct KeyRegistry {
    by_name: HashMap<String, KeyId>,
}

impl KeyRegistry {
    pub fn new() -> Self {
        KeyRegistry::default()
    }

    /// 用 [`Key::NAME`] 登记一个键。
    pub fn add<K: Key>(&mut self) -> &mut Self {
        self.add_as::<K>(K::NAME)
    }

    /// 用另一个名字登记一个键，供 guest 侧的命名与 Rust 侧不一致时用。
    ///
    /// # Panics
    ///
    /// 同一个名字被两个不同的键占用时 panic。这是启动期的接线错误，而静默地
    /// 让后来者顶掉前一个，等于放弃 [`KeyId`] 那条「两个恰好同名的键不会互相
    /// 冒充」的保证——只不过这次冒充的后果是一个 guest 拿到了别人的能力。
    pub fn add_as<K: Key>(&mut self, name: impl Into<String>) -> &mut Self {
        let name = name.into();
        let kid = KeyId::of::<K>();
        match self.by_name.get(&name) {
            Some(existing) if existing.type_id != kid.type_id => {
                panic!(
                    "能力键名冲突：{name} 已经登记给 {}，不能再登记给 {}",
                    existing.name, kid.name
                );
            }
            _ => {
                self.by_name.insert(name, kid);
            }
        }
        self
    }

    pub fn get(&self, name: &str) -> Option<KeyId> {
        self.by_name.get(name).copied()
    }

    /// 查一个名字，未登记即 [`Error::UnknownKey`]。
    pub fn resolve(&self, name: &str) -> Result<KeyId> {
        self.get(name)
            .ok_or_else(|| Error::UnknownKey(name.to_owned()))
    }

    /// 一次翻译一串名字，用于把 guest 声明的导入变成 `inject`。
    ///
    /// 全有或全无：任何一个名字没登记就整体失败，因为一个少了某项依赖的组件会
    /// 被激活得太早。
    pub fn resolve_all<S: AsRef<str>>(&self, names: &[S]) -> Result<Vec<KeyId>> {
        names
            .iter()
            .map(|name| self.resolve(name.as_ref()))
            .collect()
    }

    /// 已登记的名字，排序后返回（便于诊断输出稳定）。
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.by_name.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }
}

/// 隔离域符号。
///
/// 对应论文 3.2.1 节的 realm $r : R$。realm 表 $\rho$ 把键映射到 realm，
/// 存储 $\sigma$ 再把 realm 映射到值，于是解析是两层的：$k \to \rho(k) \to \sigma(\rho(k))$。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RealmId(pub(crate) u64);

impl RealmId {
    /// 根 realm：未被隔离的键都解析到它。
    pub const ROOT: RealmId = RealmId(0);
}
