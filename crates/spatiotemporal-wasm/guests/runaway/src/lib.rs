//! 一个不肯拆掉的 guest：`unload` 里死循环。
//!
//! 存在的意义是把「guest 的逆必须可抢占」这条限制变成一个会失败的测试。没有
//! 燃料上限的话，卸载它会把整个宿主挂死——而卸载是没有别的出路的：论文承诺逆
//! **会被调用**，可没承诺逆自己规矩。

wit_bindgen::generate!({
    path: "../../wit",
    world: "plugin",
});

use composability::plugin::host::log;
use exports::composability::plugin::lifecycle::Guest;

struct Runaway;

impl Guest for Runaway {
    fn load() -> Result<(), String> {
        log("runaway 装上了");
        Ok(())
    }

    fn unload() {
        log("runaway 开始赖着不走");
        let mut n: u64 = 0;
        loop {
            n = n.wrapping_add(1);
            if n == u64::MAX {
                log("到不了这里");
            }
        }
    }

    fn invoke(_name: String, _args: String) -> Result<String, String> {
        Err("runaway 没有工具".into())
    }
}

export!(Runaway);
