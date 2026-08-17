use std::cell::RefCell;

use spatiotemporal::{App, Applied, Entry, Loader, Patch, Result, compose};

/// 长驻进程状态：保留 App 与 Loader，支持配置热对账（创造模式）。
pub struct AgentRuntime {
    app: RefCell<App>,
    loader: Loader,
    base: Vec<Entry>,
    layers: RefCell<Vec<Vec<Patch>>>,
}

impl AgentRuntime {
    pub fn new(app: App, loader: Loader, base: Vec<Entry>, layers: Vec<Vec<Patch>>) -> Self {
        AgentRuntime {
            app: RefCell::new(app),
            loader,
            base,
            layers: RefCell::new(layers),
        }
    }

    #[allow(dead_code)]
    pub fn loader(&self) -> &Loader {
        &self.loader
    }

    #[allow(dead_code)]
    pub fn base(&self) -> &[Entry] {
        &self.base
    }

    pub fn layers(&self) -> Vec<Vec<Patch>> {
        self.layers.borrow().clone()
    }

    pub fn current_entries(&self) -> Result<Vec<Entry>> {
        Ok(compose(&self.base, &self.layers.borrow())?.entries)
    }

    pub fn apply(&self) -> Result<Applied> {
        let entries = self.current_entries()?;
        self.app
            .borrow_mut()
            .block_on(self.loader.apply(entries))
    }

    pub fn root(&self) -> spatiotemporal::Context {
        self.app.borrow().root()
    }

    pub fn push_layer(&self, layer: Vec<Patch>) -> Result<Applied> {
        self.layers.borrow_mut().push(layer);
        self.apply()
    }
}
