use std::fmt;

/// 组件与运行时之间的失败。
///
/// 对应论文 4.3.4 节：失败使 fiber 的 target 置 ⊥ 并触发已累积逆的回卷，
/// 而不是让运行时进入未定义状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// 守卫在某个 step 边界失效：这次转换已经过期。
    ///
    /// 对应论文 4.3.2 节的步骤边界中断。组件应当把它向上传播（`?`），
    /// 运行时会把已登记的逆回卷掉，再链接进下一次转换。
    Aborted,
    /// 访问了未在 `inject` 中声明的 coeffect。
    ///
    /// 对应算法 6 第 6 行的 `UNDECLARED_ACCESS`。
    Undeclared(&'static str),
    /// 声明了该 coeffect，但发起访问的 fiber 尚未提交它。
    ///
    /// 对应算法 6 第 5 行的 `INACTIVE_ACCESS`。
    Inactive(&'static str),
    /// 组件自身报告的失败。
    Component(String),
    /// 配置里指名了注册表中没有的组件。
    ///
    /// Rust 没有运行时模块注册表（论文 6.4 节），所以「装不上」在这里是一个
    /// 同步的、发生在任何 fiber 被改动**之前**的错误。
    Unknown(String),
    /// 配置本身不合法：解析失败，或不符合组件的期望形状。
    Config(String),
    /// 对账失败后，回滚也失败了。
    ///
    /// 系统此时处于部分应用状态。这条错误的存在本身就是承诺的边界：可撤销
    /// effect 保证逆会被调用，不保证逆自己不会出错。
    Rollback(Vec<String>),
}

impl Error {
    pub fn msg(message: impl Into<String>) -> Self {
        Error::Component(message.into())
    }

    /// 是否为「转换过期」而非真正的错误。
    pub fn is_aborted(&self) -> bool {
        matches!(self, Error::Aborted)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Aborted => write!(f, "转换已过期，effect 在 step 边界被中断"),
            Error::Undeclared(name) => write!(f, "访问了未声明的依赖：{name}"),
            Error::Inactive(name) => write!(f, "依赖当前未被提供：{name}"),
            Error::Component(message) => write!(f, "{message}"),
            Error::Unknown(name) => write!(f, "注册表里没有这个组件：{name}"),
            Error::Config(message) => write!(f, "配置不合法：{message}"),
            Error::Rollback(messages) => {
                write!(f, "对账失败后回滚也失败了：{}", messages.join("；"))
            }
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T, E = Error> = std::result::Result<T, E>;
