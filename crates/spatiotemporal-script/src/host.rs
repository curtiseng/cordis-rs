use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use rquickjs::{Ctx, Exception, Function, Object};
use spatiotemporal::{Context, Error, Key, KeyId, KeyRegistry, Result};

use crate::ToolHost;

/// 「从上下文里取出一项能力，并投影成 guest 收得下的形状」。
type Projection = Box<dyn Fn(&Context) -> Result<String>>;

/// 宿主愿意让脚本看见的能力面。
///
/// 形状与 `spatiotemporal-wasm` 的同名类型一致：每一项都要宿主明确写出投影，
/// 因为跨语言边界的值只能是字符串，而原生 coeffect 是 `Rc<dyn Trait>`。两个
/// crate 各自带一份，是为了互不依赖、各自发布；宿主若同时用两种基质，就要建
/// 两张表——这不是疏忽，是「能力面由宿主钉死」这条约束的两次出现。
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
    ///
    /// 快照不是对动态性的妥协：论文里一个 fiber 的 committed view 在它整段
    /// 活跃期内是固定的，依赖一变它就被重载。所以「装上时取一次」与「每次
    /// 调用都去查」在可观察行为上没有差别——而快照顺带挡掉了重入：JS 运行时
    /// 里不放 `Context`，guest 就没法在自己的转换还没结束时回头调内核。
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

/// 把 `host.log` / `host.capability` / `host.registerTool` / `host.registerLlm` /
/// `host.callTool` 挂到全局。
pub(crate) fn install_host<'js>(
    ctx: Ctx<'js>,
    view: HashMap<String, String>,
    logs: Arc<Mutex<Vec<String>>>,
    pending_tools: Arc<Mutex<Vec<(String, String)>>>,
    pending_llm: Arc<Mutex<Option<String>>>,
    tools: Option<Rc<dyn ToolHost>>,
) -> rquickjs::Result<()> {
    let tools_table = Object::new(ctx.clone())?;
    ctx.globals().set("__tools", tools_table)?;

    let host = Object::new(ctx.clone())?;

    let logs_for_log = logs.clone();
    host.set(
        "log",
        Function::new(ctx.clone(), move |message: String| {
            if let Ok(mut logs) = logs_for_log.lock() {
                logs.push(message);
            }
        })?,
    )?;

    host.set(
        "capability",
        Function::new(ctx.clone(), move |ctx: Ctx<'_>, name: String| {
            match view.get(&name) {
                Some(value) => Ok(value.clone()),
                None => Err(Exception::throw_message(
                    &ctx,
                    &format!("没有授予这项能力：{name}"),
                )),
            }
        })?,
    )?;

    host.set(
        "registerTool",
        Function::new(ctx.clone(), {
            let pending_tools = pending_tools.clone();
            move |ctx, name, description, func| {
                stash_tool(ctx, &pending_tools, name, description, func)
            }
        })?,
    )?;

    host.set(
        "registerLlm",
        Function::new(ctx.clone(), {
            let pending_llm = pending_llm.clone();
            move |ctx, model, func| stash_llm(ctx, &pending_llm, model, func)
        })?,
    )?;

    host.set(
        "callTool",
        Function::new(
            ctx.clone(),
            move |js_ctx: Ctx<'_>, name: String, args: String| {
                let Some(host) = tools.as_ref() else {
                    return Err(Exception::throw_message(&js_ctx, "宿主未接 tool 桥接"));
                };
                match host.call_tool(&name, &args) {
                    Ok(text) => Ok(text),
                    Err(error) => Err(Exception::throw_message(&js_ctx, &error.to_string())),
                }
            },
        )?,
    )?;

    ctx.globals().set("host", host)
}

fn stash_tool<'js>(
    ctx: Ctx<'js>,
    pending: &Arc<Mutex<Vec<(String, String)>>>,
    name: String,
    description: String,
    func: Function<'js>,
) -> rquickjs::Result<()> {
    let table: Object<'js> = ctx.globals().get("__tools")?;
    table.set(name.as_str(), func)?;
    if let Ok(mut pending) = pending.lock() {
        pending.push((name, description));
    }
    Ok(())
}

fn stash_llm<'js>(
    ctx: Ctx<'js>,
    pending: &Arc<Mutex<Option<String>>>,
    model: String,
    func: Function<'js>,
) -> rquickjs::Result<()> {
    ctx.globals().set("__llm", func)?;
    if let Ok(mut pending) = pending.lock() {
        *pending = Some(model);
    }
    Ok(())
}
