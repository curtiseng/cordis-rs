use std::rc::Rc;

use spatiotemporal::Key;

use crate::host::{Document, Fs, Llm, Shell, Surface};

pub(crate) enum Doc {}
impl Key for Doc {
    type Api = dyn Document;
    const NAME: &'static str = "markdown";
}

pub(crate) enum LlmKey {}
impl Key for LlmKey {
    type Api = dyn Llm;
    const NAME: &'static str = "llm";
}

pub(crate) enum SurfaceKey {}
impl Key for SurfaceKey {
    type Api = dyn Surface;
    const NAME: &'static str = "surface";
}

pub(crate) enum FsKey {}
impl Key for FsKey {
    type Api = dyn Fs;
    const NAME: &'static str = "fs";
}

pub(crate) enum ShellKey {}
impl Key for ShellKey {
    type Api = dyn Shell;
    const NAME: &'static str = "shell";
}

pub fn lookup_surface(ctx: &spatiotemporal::Context) -> Option<Rc<dyn Surface>> {
    ctx.lookup::<SurfaceKey>()
}
