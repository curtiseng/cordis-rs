use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
struct Registry {
    current: Option<String>,
    recent: Vec<String>,
}

#[derive(Serialize)]
pub struct WorkspaceList {
    pub current: String,
    pub recent: Vec<String>,
}

pub struct WorkspaceStore {
    registry_path: PathBuf,
    active: Rc<RefCell<PathBuf>>,
}

impl WorkspaceStore {
    pub fn new(initial: PathBuf) -> Rc<Self> {
        let initial = initial.canonicalize().unwrap_or_else(|_| initial.clone());
        let registry_path = initial.join(".agent/workspaces.json");
        let store = Rc::new(Self {
            registry_path,
            active: Rc::new(RefCell::new(initial.clone())),
        });
        if let Ok(reg) = store.load_registry() {
            if let Some(current) = reg.current {
                let path = PathBuf::from(&current);
                if path.is_dir()
                    && let Ok(canonical) = path.canonicalize()
                {
                    *store.active.borrow_mut() = canonical;
                }
            }
            store.merge_recent(&reg.recent);
        }
        let _ = store.touch_recent(&store.active.borrow());
        store
    }

    pub fn handle(&self) -> Rc<RefCell<PathBuf>> {
        self.active.clone()
    }

    pub fn current(&self) -> PathBuf {
        self.active.borrow().clone()
    }

    pub fn list(&self) -> WorkspaceList {
        let current = self.current().display().to_string();
        let recent = self
            .load_registry()
            .map(|reg| reg.recent)
            .unwrap_or_default()
            .into_iter()
            .filter(|path| Path::new(path).is_dir())
            .collect();
        WorkspaceList { current, recent }
    }

    pub fn switch(&self, path: &str) -> Result<PathBuf, String> {
        let canonical = canonical_dir(path)?;
        *self.active.borrow_mut() = canonical.clone();
        let _ = self.touch_recent(&canonical);
        Ok(canonical)
    }

    fn load_registry(&self) -> Result<Registry, String> {
        if !self.registry_path.is_file() {
            return Ok(Registry::default());
        }
        let text = fs::read_to_string(&self.registry_path).map_err(|error| error.to_string())?;
        serde_json::from_str(&text).map_err(|error| error.to_string())
    }

    fn save_registry(&self, reg: &Registry) -> Result<(), String> {
        if let Some(parent) = self.registry_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let text = serde_json::to_string_pretty(reg).map_err(|error| error.to_string())?;
        fs::write(&self.registry_path, text).map_err(|error| error.to_string())
    }

    fn merge_recent(&self, paths: &[String]) {
        for path in paths {
            if Path::new(path).is_dir() {
                let _ = self.touch_recent(&PathBuf::from(path));
            }
        }
    }

    fn touch_recent(&self, path: &Path) -> Result<(), String> {
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("无效目录：{error}"))?;
        let key = canonical.display().to_string();
        let mut reg = self.load_registry().unwrap_or_default();
        reg.current = Some(key.clone());
        reg.recent.retain(|entry| entry != &key);
        reg.recent.insert(0, key);
        reg.recent.truncate(12);
        self.save_registry(&reg)
    }
}

fn canonical_dir(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("缺少目录路径".into());
    }
    let candidate = PathBuf::from(trimmed);
    let canonical = candidate
        .canonicalize()
        .map_err(|error| format!("无法打开目录：{error}"))?;
    if !canonical.is_dir() {
        return Err("不是目录".into());
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("st-ws-store-{name}-{nanos:x}"))
    }

    #[test]
    fn switch_updates_current_and_recent() {
        let a = temp_root("a");
        let b = temp_root("b");
        fs::create_dir_all(&a).expect("mkdir a");
        fs::create_dir_all(&b).expect("mkdir b");
        let store = WorkspaceStore::new(a.clone());
        store.switch(&b.display().to_string()).expect("switch");
        assert_eq!(store.current(), b.canonicalize().expect("canon b"));
        let list = store.list();
        assert_eq!(
            list.current,
            b.canonicalize().expect("canon b").display().to_string()
        );
        assert!(
            list.recent
                .iter()
                .any(|entry| entry.contains("st-ws-store-b"))
        );
        let _ = fs::remove_dir_all(a);
        let _ = fs::remove_dir_all(b);
    }
}
