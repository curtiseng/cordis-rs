use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::LocalBoxFuture;
use rquickjs::{Context as JsContext, Function, Module, Runtime};
use spatiotemporal::{Component, Context, Error, KeyId, Result, Steps};

use crate::host::{Capabilities, install_host};

/// 默认给 guest 的中断次数上限。
///
/// QuickJS 没有 wasmtime 那种指令燃料。它定期调用一次 interrupt handler，
/// 返回 true 就抛出不可捕获的异常、把控制交还宿主。次数不是指令数，所以
/// 同一个额度在 wasm 和脚本之间**没有**换算关系——各自的上限各自定。
const DEFAULT_FUEL: u64 = 10_000;

/// 一个被接成 fiber 的 QuickJS 脚本。
///
/// 这个类型是整件事的全部：它就是 [`Component`] 的又一种实现。内核不知道
/// JavaScript 存在。
pub struct ScriptPlugin {
    name: String,
    source: String,
    granted: Vec<String>,
    inject: Vec<KeyId>,
    capabilities: Rc<Capabilities>,
    fuel: u64,
    logs: Arc<Mutex<Vec<String>>>,
}

impl ScriptPlugin {
    /// 编译一段脚本，并把授予它的能力名翻成 coeffect 键。
    ///
    /// 名字由调用方给出——模型这一轮现写的代码没有文件名。这正是内核把
    /// [`Component::name`] 从 `&'static str` 放宽成 `&str` 的原因。
    ///
    /// 语法错误在这里就失败，任何 fiber 都还没被动过。这是 loader「先构造
    /// 再拆除」在脚本基质上的落点：一段写坏的生成代码不该先把系统拆一半。
    pub fn from_source(
        name: impl Into<String>,
        source: impl Into<String>,
        capabilities: Rc<Capabilities>,
        granted: Vec<String>,
    ) -> Result<Self> {
        let name = name.into();
        let source = source.into();
        let inject = capabilities.inject(&granted)?;
        syntax_check(&name, &source)?;

        Ok(ScriptPlugin {
            name,
            source,
            granted,
            inject,
            capabilities,
            fuel: DEFAULT_FUEL,
            logs: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// 改中断次数上限。
    pub fn with_fuel(mut self, fuel: u64) -> Self {
        self.fuel = fuel;
        self
    }

    /// guest 至今记下的日志。
    pub fn logs(&self) -> Vec<String> {
        self.logs
            .lock()
            .map(|logs| logs.clone())
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for ScriptPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptPlugin")
            .field("name", &self.name)
            .field("granted", &self.granted)
            .field("fuel", &self.fuel)
            .field("source_len", &self.source.len())
            .finish_non_exhaustive()
    }
}

impl Component for ScriptPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn inject(&self) -> Vec<KeyId> {
        self.inject.clone()
    }

    fn apply(&self, ctx: Context, steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let view = self.capabilities.snapshot(&ctx, &self.granted)?;

            let rt = Runtime::new().map_err(js_error("建不出 JS 运行时"))?;
            let ticks = Arc::new(AtomicU64::new(0));
            let fuel = self.fuel;
            {
                let ticks = ticks.clone();
                rt.set_interrupt_handler(Some(Box::new(move || {
                    ticks.fetch_add(1, Ordering::Relaxed) >= fuel
                })));
            }

            let js = JsContext::full(&rt).map_err(js_error("建不出 JS 上下文"))?;
            let logs = self.logs.clone();
            let source = self.source.clone();
            let name = self.name.clone();

            js.with(|js_ctx| -> Result<()> {
                install_host(js_ctx.clone(), view, logs.clone())
                    .map_err(js_error("接不上宿主接口"))?;

                let module = Module::declare(js_ctx.clone(), format!("{name}.js"), source)
                    .map_err(js_error("脚本声明失败"))?;
                let (module, _promise) = module.eval().map_err(js_error("脚本求值失败"))?;

                // Function 的生命周期绑在这次 `with` 上，带不出闭包。所以把
                // unload 拷到全局一个我们控制的槽上，逆再从那里取。缺 unload
                // 就登记一个空操作——模型现写的代码经常忘了清理，fiber 仍须
                // 能拆掉。
                match module.get::<_, Function>("unload") {
                    Ok(unload) => js_ctx
                        .globals()
                        .set("__spatiotemporal_unload", unload)
                        .map_err(js_error("挂不上 unload"))?,
                    Err(_) => {
                        let noop = Function::new(js_ctx.clone(), || {})
                            .map_err(js_error("建不出空 unload"))?;
                        js_ctx
                            .globals()
                            .set("__spatiotemporal_unload", noop)
                            .map_err(js_error("挂不上 unload"))?;
                    }
                }

                let load: Function = module
                    .get("load")
                    .map_err(|_| Error::Component("脚本没有导出 load".into()))?;
                load.call::<(), ()>(())
                    .map_err(js_error("load 失败或被中断"))?;
                Ok(())
            })?;

            let name = self.name.clone();
            let logs = self.logs.clone();
            steps.step_sync(move || {
                // 重新计数：load 把额度用掉之后，unload 会当场被打断——
                // 那样逆就没跑成，而逆是不允许「没跑成」的。
                ticks.store(0, Ordering::Relaxed);
                let outcome = js.with(|js_ctx| -> rquickjs::Result<()> {
                    let unload: Function = js_ctx.globals().get("__spatiotemporal_unload")?;
                    unload.call::<(), ()>(())
                });
                if let Err(error) = outcome
                    && let Ok(mut logs) = logs.lock()
                {
                    logs.push(format!("{name} 的 unload 陷入了：{error}"));
                }
            })
        })
    }
}

/// 只解析、不执行：语法错误在任何 fiber 被动过之前就失败。
fn syntax_check(name: &str, source: &str) -> Result<()> {
    let rt = Runtime::new().map_err(js_error("建不出 JS 运行时"))?;
    let js = JsContext::full(&rt).map_err(js_error("建不出 JS 上下文"))?;
    js.with(|ctx| Module::declare(ctx.clone(), format!("{name}.js"), source).map(|_| ()))
        .map_err(js_error("脚本语法不合法"))
}

fn js_error(context: &str) -> impl FnOnce(rquickjs::Error) -> Error + use<'_> {
    move |error| Error::Component(format!("{context}：{error}"))
}
