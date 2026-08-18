use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use spatiotemporal::{App, Applied, Context, Entry, Loader, Patch, Result, compose, parse_patches};

use crate::host::root_dir;
use crate::session;

/// 运行时互斥配置档：标准 / 编码 / 创造。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentProfile {
    Standard,
    Coding,
    Creation,
}

impl AgentProfile {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "standard" => Some(AgentProfile::Standard),
            "coding" => Some(AgentProfile::Coding),
            "creation" => Some(AgentProfile::Creation),
            _ => None,
        }
    }
}

/// 长驻进程状态：保留 App 与 Loader，支持配置热对账（创造模式）。
pub struct AgentRuntime {
    app: RefCell<App>,
    loader: Loader,
    base: Vec<Entry>,
    /// 启动时叠的层（smoke / doc 路径等），不参与 save_patch。
    bootstrap_layers: Vec<Vec<Patch>>,
    /// 互斥配置档层（`cordis.coding.yml` / `cordis.creation.yml`）。
    mode_layer: RefCell<Option<Vec<Patch>>>,
    profile: RefCell<AgentProfile>,
    /// 来自 `cordis.patch.yml` 的文件层（可热重载）。
    file_layer: RefCell<Option<Vec<Patch>>>,
    /// 当前激活会话的 patch 层栈（持久化到 `.agent/sessions/{id}.patch.json`）。
    session_layers: RefCell<Vec<Vec<Patch>>>,
    active_session: RefCell<Option<String>>,
    session_workspace: RefCell<Option<PathBuf>>,
    coding_patch_path: PathBuf,
    creation_patch_path: PathBuf,
}

impl AgentRuntime {
    pub fn new(
        app: App,
        loader: Loader,
        base: Vec<Entry>,
        bootstrap_layers: Vec<Vec<Patch>>,
    ) -> Self {
        AgentRuntime {
            app: RefCell::new(app),
            loader,
            base,
            bootstrap_layers,
            mode_layer: RefCell::new(None),
            profile: RefCell::new(AgentProfile::Standard),
            file_layer: RefCell::new(None),
            session_layers: RefCell::new(Vec::new()),
            active_session: RefCell::new(None),
            session_workspace: RefCell::new(None),
            coding_patch_path: root_dir().join("cordis.coding.yml"),
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

    pub fn session_layers(&self) -> Vec<Vec<Patch>> {
        self.session_layers.borrow().clone()
    }

    pub fn active_session_id(&self) -> Option<String> {
        self.active_session.borrow().clone()
    }

    pub fn require_active_session(&self) -> Result<String> {
        self.active_session_id().ok_or_else(|| {
            spatiotemporal::Error::Component("没有激活的会话，无法变更会话 patch".into())
        })
    }

    #[allow(dead_code)]
    pub fn file_layer(&self) -> Option<Vec<Patch>> {
        self.file_layer.borrow().clone()
    }

    pub fn profile(&self) -> AgentProfile {
        *self.profile.borrow()
    }

    pub fn creation_enabled(&self) -> bool {
        self.profile() == AgentProfile::Creation
    }

    pub fn coding_enabled(&self) -> bool {
        self.profile() == AgentProfile::Coding
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
        layers.extend(self.session_layers.borrow().clone());
        layers
    }

    pub fn current_entries(&self) -> Result<Vec<Entry>> {
        Ok(compose(&self.base, &self.composed_layers())?.entries)
    }

    pub fn apply(&self) -> Result<Applied> {
        let entries = self.current_entries()?;
        self.app.borrow_mut().block_on(self.loader.apply(entries))
    }

    fn profile_patch_path(&self, profile: AgentProfile) -> Option<&Path> {
        match profile {
            AgentProfile::Standard => None,
            AgentProfile::Coding => Some(self.coding_patch_path.as_path()),
            AgentProfile::Creation => Some(self.creation_patch_path.as_path()),
        }
    }

    /// 切换当前会话：从磁盘加载该会话的 patch 栈并对账。
    pub fn activate_session(&self, workspace: &Path, session_id: &str) -> Result<Applied> {
        if !session::valid_session_id(session_id) {
            return Err(spatiotemporal::Error::Component(format!(
                "非法 session_id：{session_id}"
            )));
        }
        let layers = session::load_session_layers(workspace, session_id)?;
        *self.active_session.borrow_mut() = Some(session_id.to_owned());
        *self.session_workspace.borrow_mut() = Some(workspace.to_path_buf());
        *self.session_layers.borrow_mut() = layers;
        self.apply()
    }

    fn persist_session_layers(&self) -> Result<()> {
        let workspace = self.session_workspace.borrow().clone().ok_or_else(|| {
            spatiotemporal::Error::Component("会话 patch 持久化缺少工作区".into())
        })?;
        let session_id = self.require_active_session()?;
        session::save_session_layers(&workspace, &session_id, &self.session_layers.borrow())
    }

    /// 运行时切换互斥配置档并对账插件树。
    pub fn set_profile(&self, profile: AgentProfile) -> Result<Applied> {
        if self.profile() == profile {
            return self.apply();
        }
        let layer = match profile {
            AgentProfile::Standard => None,
            AgentProfile::Coding | AgentProfile::Creation => {
                let path = self.profile_patch_path(profile).expect("profile path");
                let text = fs::read_to_string(path).map_err(map_io)?;
                Some(parse_patches(&text)?)
            }
        };
        *self.profile.borrow_mut() = profile;
        *self.mode_layer.borrow_mut() = layer;
        self.apply()
    }

    /// 运行时开关创造模式（兼容 API）。
    #[allow(dead_code)]
    pub fn set_creation_mode(&self, enabled: bool) -> Result<Applied> {
        self.set_profile(if enabled {
            AgentProfile::Creation
        } else if self.profile() == AgentProfile::Creation {
            AgentProfile::Standard
        } else {
            return self.apply();
        })
    }

    /// 运行时开关编码模式（兼容 API）。
    #[allow(dead_code)]
    pub fn set_coding_mode(&self, enabled: bool) -> Result<Applied> {
        self.set_profile(if enabled {
            AgentProfile::Coding
        } else if self.profile() == AgentProfile::Coding {
            AgentProfile::Standard
        } else {
            return self.apply();
        })
    }

    pub fn push_layer(&self, layer: Vec<Patch>) -> Result<Applied> {
        self.require_active_session()?;
        self.session_layers.borrow_mut().push(layer);
        self.persist_session_layers()?;
        self.apply()
    }

    pub fn pop_layer(&self) -> Result<Option<Applied>> {
        if self.session_layers.borrow().is_empty() {
            return Ok(None);
        }
        self.require_active_session()?;
        self.session_layers.borrow_mut().pop();
        self.persist_session_layers()?;
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
