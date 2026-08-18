//! 测试 guest：叶子 tool 通过 call-tool 桥接宿主工具表。

wit_bindgen::generate!({
    path: "../../wit",
    world: "plugin",
});

use composability::plugin::host::{call_tool, log, register_tool};
use exports::composability::plugin::lifecycle::Guest;

struct Bridge;

impl Guest for Bridge {
    fn load() -> Result<(), String> {
        register_tool("leaf", "桥接上游工具");
        log("bridge 装上了");
        Ok(())
    }

    fn unload() {
        log("bridge 拆掉了");
    }

    fn invoke(name: String, args: String) -> Result<String, String> {
        if name != "leaf" {
            return Err(format!("没有这个工具：{name}"));
        }
        let upstream = call_tool("upstream", &args)?;
        Ok(format!(r#"{{"bridged":"{}"}}"#, upstream.replace('"', "\\\"")))
    }
}

export!(Bridge);
