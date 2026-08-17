use std::any::TypeId;

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
