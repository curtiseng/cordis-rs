use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::util::resolve_within;

const SKIP_NAMES: &[&str] = &[".git", "target", "node_modules", ".agent", ".cursor"];
const MAX_PREVIEW_BYTES: usize = 256 * 1024;

#[derive(Serialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}

#[derive(Serialize)]
pub struct WorkspaceListing {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub entries: Vec<DirEntry>,
}

#[derive(Serialize)]
pub struct WorkspaceFile {
    pub path: String,
    pub text: String,
    pub truncated: bool,
    pub binary: bool,
}

pub fn list_dir(root: &Path, user_path: &str) -> spatiotemporal::Result<WorkspaceListing> {
    let rel = user_path.trim().trim_matches('/');
    let dir = if rel.is_empty() {
        root.canonicalize().map_err(map_io)?
    } else {
        resolve_within(root, rel)?
    };
    if !dir.is_dir() {
        return Err(spatiotemporal::Error::Component(format!(
            "不是目录：{user_path}"
        )));
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir).map_err(map_io)? {
        let entry = entry.map_err(map_io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if should_skip(&name) {
            continue;
        }
        let meta = entry.metadata().map_err(map_io)?;
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if meta.is_dir() {
            entries.push(DirEntry {
                name,
                path: child_rel,
                kind: "dir",
                size: None,
                count: Some(count_dir_children(&entry.path())),
            });
        } else if meta.is_file() {
            entries.push(DirEntry {
                name,
                path: child_rel,
                kind: "file",
                size: Some(meta.len()),
                count: None,
            });
        }
    }

    entries.sort_by(|a, b| {
        a.kind
            .cmp(b.kind)
            .reverse()
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let parent = if rel.is_empty() {
        None
    } else {
        Path::new(rel)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| !p.is_empty())
    };

    Ok(WorkspaceListing {
        path: rel.to_owned(),
        parent,
        entries,
    })
}

pub fn read_file(root: &Path, user_path: &str) -> spatiotemporal::Result<WorkspaceFile> {
    let rel = user_path.trim().trim_matches('/');
    if rel.is_empty() {
        return Err(spatiotemporal::Error::Component("缺少文件路径".into()));
    }
    let path = resolve_within(root, rel)?;
    if !path.is_file() {
        return Err(spatiotemporal::Error::Component(format!(
            "不是文件：{user_path}"
        )));
    }
    let bytes = fs::read(&path).map_err(map_io)?;
    let truncated = bytes.len() > MAX_PREVIEW_BYTES;
    let slice = if truncated {
        &bytes[..MAX_PREVIEW_BYTES]
    } else {
        &bytes
    };
    if slice.contains(&0) {
        return Ok(WorkspaceFile {
            path: rel.to_owned(),
            text: String::new(),
            truncated: false,
            binary: true,
        });
    }
    Ok(WorkspaceFile {
        path: rel.to_owned(),
        text: String::from_utf8_lossy(slice).into_owned(),
        truncated,
        binary: false,
    })
}

fn should_skip(name: &str) -> bool {
    SKIP_NAMES.contains(&name)
}

fn count_dir_children(path: &Path) -> usize {
    fs::read_dir(path)
        .ok()
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| !should_skip(&entry.file_name().to_string_lossy()))
                .count()
        })
        .unwrap_or(0)
}

fn map_io(error: std::io::Error) -> spatiotemporal::Error {
    spatiotemporal::Error::Component(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn setup() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "st-workspace-test-{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).expect("mkdir");
        fs::write(root.join("README.md"), "# hi").expect("write");
        fs::write(root.join("src/lib.rs"), "fn main() {}").expect("write");
        fs::create_dir_all(root.join(".git")).expect("git dir");
        root
    }

    #[test]
    fn lists_workspace_skips_git() {
        let root = setup();
        let listing = list_dir(&root, "").expect("list");
        assert!(listing.entries.iter().any(|e| e.name == "README.md"));
        assert!(listing.entries.iter().any(|e| e.name == "src"));
        assert!(!listing.entries.iter().any(|e| e.name == ".git"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_text_file() {
        let root = setup();
        let file = read_file(&root, "README.md").expect("read");
        assert_eq!(file.text, "# hi");
        assert!(!file.binary);
        let _ = fs::remove_dir_all(&root);
    }
}
