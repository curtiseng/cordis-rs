use std::path::Path;
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use spatiotemporal::{Component, Context, Result, Steps};

use crate::host::Document;
use crate::keys::Doc;

struct FileDoc {
    path: String,
    text: String,
}

impl Document for FileDoc {
    fn path(&self) -> String {
        self.path.clone()
    }
    fn text(&self) -> String {
        self.text.clone()
    }
}

/// 本地插件：读一个文件，提供 `markdown` 能力。
pub struct DocFile {
    doc: Rc<FileDoc>,
}

impl DocFile {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(DocFile {
            doc: Rc::new(FileDoc {
                path: path.display().to_string(),
                text,
            }),
        })
    }
}

impl Component for DocFile {
    fn name(&self) -> &str {
        "doc"
    }

    fn apply(&self, ctx: Context, _steps: Steps) -> LocalBoxFuture<'_, Result<()>> {
        let doc = self.doc.clone();
        Box::pin(async move {
            ctx.set::<Doc>(doc as Rc<dyn Document>);
            Ok(())
        })
    }
}
