use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use spatiotemporal::{App, Applied, Context, Entry, Loader, Patch, Result, compose, parse_patches};

use crate::host::root_dir;

/// 长驻进程状态：保留 App 与 Loader，支持配置热对账（创造模式）。
pub struct AgentRuntime {
    app: RefCell<App>,
    loader: Loader,
    base: Vec<Entry>,
    /// 启动时叠的层（smoke / doc 路径等），不参与 save_patch。
    bootstrap_layers: Vec<Vec<Patch>>,
    /// 创造模式层（`cordis.creation.yml`），可运行时开关。
    mode_layer: RefCell<Option<Vec<Patch>>>,
    /// 来自 `cordis.patch.yml` 的文件层（可热重载）。
    file_layer: RefCell<Option<Vec<Patch>>>,
    /// 运行时 `push_layer` / `run_patch` 追加的层。
    dynamic_layers: RefCell<Vec<Vec<Patch>>>,
    creation_patch_path: PathBuf,
}

impl AgentRuntime {
    pub fn new(app: App, loader: Loader, base: Vec<Entry>, bootstrap_layers: Vec<Vec<Patch>>) -> Self {
        AgentRuntime {
            app: RefCell::new(app),
            loader,
            base,
            bootstrap_layers,
            mode_layer: RefCell::new(None),
            file_layer: RefCell::new(None),
            dynamic_layers: RefCell::new(Vec::new()),
            creation_patch_path: root_dir().join("cordis.creation.yml"),
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

    #[allow(dead_code)]
    pub fn bootstrap_layers(&self) -> &[Vec<Patch>] {
        &self.bootstrap_layers
    }

    pub fn dynamic_layers(&self) -> Vec<Vec<Patch>> {
        self.dynamic_layers.borrow().clone()
    }

    #[allow(dead_code)]
    pub fn file_layer(&self) -> Option<Vec<Patch>> {
        self.file_layer.borrow().clone()
    }

    pub fn creation_enabled(&self) -> bool {
        self.mode_layer.borrow().is_some()
    }

    pub fn root(&self) -> Context {
        self.app.borrow().root()
    }

    fn composed_layers(&self) -> Vec<Vec<Patch>> {
        let mut layers = self.bootstrap_layers.clone();
        if let Some(mode) = self.mode_layer.borrow().clone() {
            layers.push(mode);
        }
        if let Some(file) = self.file_layer.borrow().clone() {
            layers.push(file);
        }
        layers.extend(self.dynamic_layers.borrow().clone());
        layers
    }

    pub fn current_entries(&self) -> Result<Vec<Entry>> {
        Ok(compose(&self.base, &self.composed_layers())?.entries)
    }

    pub fn apply(&self) -> Result<Applied> {
        let entries = self.current_entries()?;
        self.app
            .borrow_mut()
            .block_on(self.loader.apply(entries))
    }

    /// 运行时开关创造模式：叠/卸 `cordis.creation.yml` 并对账插件树。
    pub fn set_creation_mode(&self, enabled: bool) -> Result<Applied> {
        if enabled {
            if self.mode_layer.borrow().is_some() {
                return self.apply();
            }
            let text = fs::read_to_string(&self.creation_patch_path).map_err(map_io)?;
            let layer = parse_patches(&text)?;
            *self.mode_layer.borrow_mut() = Some(layer);
        } else if self.mode_layer.borrow().is_none() {
            return self.apply();
        } else {
            *self.mode_layer.borrow_mut() = None;
        }
        self.apply()
    }

    pub fn push_layer(&self, layer: Vec<Patch>) -> Result<Applied> {
        self.dynamic_layers.borrow_mut().push(layer);
        self.apply()
    }

    pub fn pop_layer(&self) -> Result<Option<Applied>> {
        if self.dynamic_layers.borrow().is_empty() {
            return Ok(None);
        }
        self.dynamic_layers.borrow_mut().pop();
        Ok(Some(self.apply()?))
    }

    pub fn set_file_layer(&self, layer: Option<Vec<Patch>>) -> Result<Applied> {
        *self.file_layer.borrow_mut() = layer;
        self.apply()
    }

    pub fn load_patch_file(&self, path: &Path) -> Result<Applied> {
        if !path.exists() {
            return self.set_file_layer(None);
        }
        let text = fs::read_to_string(path).map_err(map_io)?;
        if text.trim().is_empty() {
            return self.set_file_layer(None);
        }
        let layer = parse_patches(&text)?;
        self.set_file_layer(Some(layer))
    }
}

fn map_io(error: std::io::Error) -> spatiotemporal::Error {
    spatiotemporal::Error::Component(error.to_string())
}
