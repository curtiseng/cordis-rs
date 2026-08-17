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

        // 授予过的能力：读得到。
        match capability("db") {
            Ok(value) => log(&format!("db = {value}")),
            Err(error) => log(&format!("db 读不到：{error}")),
        }

        // 没授予的能力：读不到。guest 主动去要也一样，因为授权是宿主给的，
        // 不是 guest 报的。
        match capability("secrets") {
            Ok(value) => log(&format!("secrets = {value}（本该拿不到！）")),
            Err(error) => log(&format!("secrets 读不到：{error}")),
        }

        Ok(())
    }

    fn unload() {
        log("probe 拆掉了");
    }
}

export!(Probe);
