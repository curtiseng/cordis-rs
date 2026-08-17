use std::rc::Rc;

use spatiotemporal::Key;

use crate::host::{Document, Fs, Llm, Shell, Surface, SystemPrompt, AgentLoop};

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

pub(crate) enum PromptKey {}
impl Key for PromptKey {
    type Api = dyn SystemPrompt;
    const NAME: &'static str = "system-prompt";
}

pub(crate) enum AgentLoopKey {}
impl Key for AgentLoopKey {
    type Api = dyn AgentLoop;
    const NAME: &'static str = "agent-loop";
}

pub fn lookup_surface(ctx: &spatiotemporal::Context) -> Option<Rc<dyn Surface>> {
    ctx.lookup::<SurfaceKey>()
}
