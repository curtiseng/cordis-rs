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
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T, E = Error> = std::result::Result<T, E>;
