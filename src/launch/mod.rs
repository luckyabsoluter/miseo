//! Command launch plans and runtime-specific launch extensions.

pub mod node;

use crate::{fs::PathBuf, spec::Runtime};

/// How a managed command should be launched from a generated shim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandTarget {
    /// Activate the tool-local mise environment, then execute the target path.
    EnvWrapped(PathBuf),
    /// Resolve a pinned runtime in the tool-local project, then execute an
    /// entrypoint directly with that runtime binary.
    RuntimeEntrypoint {
        runtime: Runtime,
        entrypoint: PathBuf,
    },
}

impl CommandTarget {
    pub fn env_wrapped(target: PathBuf) -> Self {
        Self::EnvWrapped(target)
    }

    pub fn runtime_entrypoint(runtime: Runtime, entrypoint: PathBuf) -> Self {
        Self::RuntimeEntrypoint {
            runtime,
            entrypoint,
        }
    }
}
