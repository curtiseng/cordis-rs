//! 一个规矩的 guest：装上时报告它看见了什么，拆掉时留下痕迹。

wit_bindgen::generate!({
    path: "../../wit",
    world: "plugin",
});

use composability::plugin::host::{capability, log};
use exports::composability::plugin::lifecycle::Guest;

struct Probe;

impl Guest for Probe {
    fn load() -> Result<(), String> {
        log("probe 装上了");

        match capability("db") {
            Ok(value) => log(&format!("db = {value}")),
            Err(error) => log(&format!("db 读不到：{error}")),
        }

        match capability("secrets") {
            Ok(value) => log(&format!("secrets = {value}（本该拿不到！）")),
            Err(error) => log(&format!("secrets 读不到：{error}")),
        }

        Ok(())
    }

    fn unload() {
        log("probe 拆掉了");
    }

    fn invoke(_name: String, _args: String) -> Result<String, String> {
        Err("probe 没有工具".into())
    }
}

export!(Probe);
