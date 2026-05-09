//! `mise` integration trait plus host/test implementations.

use std::collections::BTreeMap;

use crate::{
    error::Error,
    fs::{Path, PathBuf},
    spec::{Backend, Runtime, RuntimePins, RuntimeSpec, ToolId, ToolSpec},
};

/// Integration contract for calling out to `mise`.
pub trait Mise {
    /// Resolve a tool spec (for example `npm:prettier@latest`) to an exact package version.
    fn resolve_latest_version(
        &self,
        runtime_versions: &RuntimePins,
        spec: &ToolSpec,
    ) -> Result<ToolSpec, Error>;

    /// Read the globally configured selector for a runtime (for example `node -> 22`).
    fn resolve_global_runtime_selector(
        &self,
        runtime: &Runtime,
    ) -> Result<Option<RuntimeSpec>, Error>;

    /// Return whether the globally configured runtime is currently installed.
    fn resolve_global_runtime_installed(&self, runtime: &Runtime) -> Result<bool, Error>;

    /// Resolve a runtime selector to its concrete current version.
    fn resolve_current_runtime_version(&self, spec: &RuntimeSpec) -> Result<RuntimeSpec, Error>;

    /// Resolve runtime pins for install/upgrade:
    /// explicit `--use` values if provided, otherwise from backend defaults + global mise config.
    fn resolve_runtime_pins(
        &self,
        backend: &Backend,
        use_inputs: &RuntimePins,
    ) -> Result<RuntimePins, Error> {
        if use_inputs.is_empty() {
            resolve_backend_runtime_pins(self, backend)
        } else {
            resolve_explicit_runtime_pins(self, use_inputs)
        }
    }

    /// Install an exact tool globally under the tool-local activated mise environment.
    fn install_global(&self, tool_spec: &ToolSpec, project_dir: &Path) -> Result<(), Error>;

    /// Install an exact tool into `target_dir` under the provided runtime selections.
    fn install_into(
        &self,
        runtime_versions: &RuntimePins,
        tool_spec: &ToolSpec,
        target_dir: &Path,
    ) -> Result<(), Error>;

    /// Return command names and global executable paths exported by the installed package.
    fn installed_command_targets(
        &self,
        tool_spec: &ToolSpec,
        project_dir: &Path,
    ) -> Result<BTreeMap<String, PathBuf>, Error>;

    /// Return executable bin directories for the installed package at `install_dir`.
    fn bin_paths(&self, tool_id: &ToolId, install_dir: &Path) -> Result<Vec<PathBuf>, Error>;

    /// Uninstall a global tool package under the tool-local activated mise environment.
    fn uninstall_global(&self, tool_id: &ToolId, project_dir: &Path) -> Result<(), Error>;

    /// Trust a generated `mise.toml` so mise will evaluate it non-interactively.
    fn trust_config(&self, config_path: &Path) -> Result<(), Error>;
}

mod cli;

pub use cli::Cli;

#[cfg(test)]
mod test;

#[cfg(test)]
pub use test::Test;

pub fn new(interactive: bool, verbose: u8) -> cli::Cli {
    cli::Cli::new(interactive, verbose)
}

fn resolve_explicit_runtime_pins<M: Mise + ?Sized>(
    mise: &M,
    use_inputs: &RuntimePins,
) -> Result<RuntimePins, Error> {
    let mut pins = RuntimePins::new();

    for selection in use_inputs.values() {
        let runtime = selection.runtime();
        let concrete = mise.resolve_current_runtime_version(selection)?;
        pins.insert(runtime.clone(), concrete);
    }

    Ok(pins)
}

fn resolve_backend_runtime_pins<M: Mise + ?Sized>(
    mise: &M,
    backend: &Backend,
) -> Result<RuntimePins, Error> {
    let Some(runtimes) = backend.default_runtimes() else {
        return Err(Error::UnmappedBackendWithoutUse {
            backend: backend.as_ref().to_string(),
        });
    };

    let mut pins = RuntimePins::new();

    for runtime in runtimes {
        let selector = mise
            .resolve_global_runtime_selector(runtime)?
            .ok_or_else(|| Error::MissingGlobalRuntime {
                runtime: runtime.as_ref().to_string(),
            })?;

        if !mise.resolve_global_runtime_installed(runtime)? {
            return Err(Error::RuntimeNotInstalled {
                runtime: runtime.as_ref().to_string(),
            });
        }

        let concrete = mise.resolve_current_runtime_version(&selector)?;
        pins.insert(runtime.clone(), concrete);
    }

    Ok(pins)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Backend;

    #[test]
    fn resolve_runtime_pins_rejects_unmapped_backend() {
        let mise = Test::default();
        let err = mise
            .resolve_runtime_pins(&Backend::Other("aqua".to_string()), &RuntimePins::new())
            .unwrap_err();
        assert!(matches!(err, Error::UnmappedBackendWithoutUse { .. }));
    }

    #[test]
    fn resolve_runtime_pins_requires_installed_runtime() {
        let mut mise = Test::default();
        mise.global_selector(Runtime::Node, "22")
            .global_installed(Runtime::Node, false);

        let err = mise
            .resolve_runtime_pins(&Backend::Npm, &RuntimePins::new())
            .unwrap_err();
        assert!(matches!(err, Error::RuntimeNotInstalled { .. }));
    }

    #[test]
    fn resolve_runtime_pins_accepts_repeatable_use() {
        let mut mise = Test::default();
        mise.current(Runtime::Node, "22", "22.13.1")
            .current(Runtime::Python, "3.12", "3.12.8");

        let pins = mise
            .resolve_runtime_pins(
                &Backend::Npm,
                &RuntimePins::try_from(vec![
                    RuntimeSpec::new(Runtime::Node, "22"),
                    RuntimeSpec::new(Runtime::Python, "3.12"),
                ])
                .unwrap(),
            )
            .unwrap();

        assert_eq!(
            pins.get(&Runtime::Node).unwrap(),
            &RuntimeSpec::new(Runtime::Node, "22.13.1")
        );
        assert_eq!(
            pins.get(&Runtime::Python).unwrap(),
            &RuntimeSpec::new(Runtime::Python, "3.12.8")
        );
    }
}
