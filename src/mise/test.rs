//! Test double for `mise` integration with programmable fake responses.

use std::{cell::RefCell, collections::HashMap};
use std::{collections::BTreeMap, fmt};

use crate::{
    error::{Error, invariant},
    fs::{self, Fs, Path, PathBuf},
    launch::CommandTarget,
    spec::{Runtime, RuntimePins, RuntimeSpec, ToolId, ToolSpec},
};

use super::Mise;

/// Test double for `Mise` with explicit per-call lookup maps.
pub struct Test {
    _fs: &'static dyn Fs,
    latest_versions: HashMap<String, String>,
    global_selector: HashMap<Runtime, String>,
    global_installed: HashMap<Runtime, bool>,
    selector_current: HashMap<(Runtime, String), String>,
    packages: HashMap<String, Package>,
    global_packages: RefCell<HashMap<String, String>>,
    global_uninstalls: RefCell<Vec<GlobalUninstall>>,
}

// `fs` is a trait object, so we derive doesn't work.
impl fmt::Debug for Test {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Test")
            .field("latest_versions", &self.latest_versions)
            .field("global_selector", &self.global_selector)
            .field("global_installed", &self.global_installed)
            .field("selector_current", &self.selector_current)
            .field("packages", &self.packages)
            .field("global_packages", &self.global_packages)
            .field("global_uninstalls", &self.global_uninstalls)
            .finish()
    }
}

impl Default for Test {
    fn default() -> Self {
        Self::new(fs::new())
    }
}

impl Test {
    pub fn new(fs: &'static dyn Fs) -> Self {
        Self {
            _fs: fs,
            latest_versions: HashMap::new(),
            global_selector: HashMap::new(),
            global_installed: HashMap::new(),
            selector_current: HashMap::new(),
            packages: HashMap::new(),
            global_packages: RefCell::new(HashMap::new()),
            global_uninstalls: RefCell::new(vec![]),
        }
    }

    /// Seed `mise latest <spec>` output.
    pub fn latest(&mut self, spec: impl Into<String>, version: impl Into<String>) -> &mut Self {
        self.latest_versions.insert(spec.into(), version.into());
        self
    }

    /// Seed `mise use -g <runtime>@<selector>` plus the resolved concrete current version.
    pub fn use_g(
        &mut self,
        runtime: Runtime,
        selector: impl Into<String>,
        current: impl Into<String>,
    ) -> &mut Self {
        let selector = selector.into();

        self.global_selector
            .insert(runtime.clone(), selector.clone());
        self.global_installed.insert(runtime.clone(), true);
        self.selector_current
            .insert((runtime, selector), current.into());

        self
    }

    /// Seed `mise current <runtime>` for a selector (used for explicit `--use` or exact selectors).
    pub fn current(
        &mut self,
        runtime: Runtime,
        selector: impl Into<String>,
        current: impl Into<String>,
    ) -> &mut Self {
        self.selector_current
            .insert((runtime, selector.into()), current.into());
        self
    }

    /// Seed the globally configured selector for a runtime.
    pub fn global_selector(&mut self, runtime: Runtime, selector: impl Into<String>) -> &mut Self {
        self.global_selector.insert(runtime, selector.into());
        self
    }

    /// Seed whether a globally configured runtime is installed.
    pub fn global_installed(&mut self, runtime: Runtime, installed: bool) -> &mut Self {
        self.global_installed.insert(runtime, installed);
        self
    }

    /// Register a fake package version and its exported executable names.
    pub fn register_package(
        &mut self,
        tool_id: impl Into<String>,
        version: impl Into<String>,
        bins: impl IntoIterator<Item = impl Into<String>>,
    ) -> &mut Self {
        let tool_id = tool_id.into();
        let version = version.into();

        debug_assert!(
            !tool_id.contains('@'),
            "register_package() expects a tool id like '<backend>:<name>' without @version"
        );
        debug_assert!(
            !version.is_empty(),
            "register_package() expects a non-empty package version"
        );

        let latest_key = format!("{tool_id}@latest");
        let exact_key = format!("{tool_id}@{version}");
        let bins = bins.into_iter().map(Into::into).collect();

        self.latest(latest_key, version);
        self.packages.insert(exact_key, Package { bins });

        self
    }

    /// Seed the version currently present in the runtime global package store.
    pub fn global_package_version(
        &mut self,
        tool_id: impl Into<String>,
        version: impl Into<String>,
    ) -> &mut Self {
        self.global_packages
            .borrow_mut()
            .insert(tool_id.into(), version.into());
        self
    }

    pub fn global_uninstalls(&self) -> Vec<(String, String)> {
        self.global_uninstalls
            .borrow()
            .iter()
            .map(|uninstall| {
                (
                    uninstall.tool_id.to_string(),
                    uninstall.project_dir.to_string(),
                )
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
struct Package {
    bins: Vec<String>,
}

#[derive(Debug, Clone)]
struct GlobalUninstall {
    tool_id: ToolId,
    project_dir: PathBuf,
}

impl Mise for Test {
    fn resolve_latest_version(
        &self,
        _runtime_versions: &RuntimePins,
        spec: &ToolSpec,
    ) -> Result<ToolSpec, Error> {
        let key = spec.to_string();
        let version =
            self.latest_versions
                .get(&key)
                .cloned()
                .ok_or_else(|| Error::MiseCommandFailed {
                    command: format!("mise latest {key}"),
                    stderr: "no fake latest mapping".to_string(),
                })?;

        if version.is_empty() {
            return Err(invariant!(
                "fake latest mapping for '{key}' returned empty version"
            ));
        }

        Ok(spec.with_version(&version))
    }

    fn resolve_global_runtime_selector(
        &self,
        runtime: &Runtime,
    ) -> Result<Option<RuntimeSpec>, Error> {
        self.global_selector
            .get(runtime)
            .map(|selector| {
                if selector.is_empty() {
                    return Err(invariant!(
                        "fake selector mapping for '{runtime}' returned empty selector"
                    ));
                }

                Ok(RuntimeSpec::new(runtime.clone(), selector.clone()))
            })
            .transpose()
    }

    fn resolve_global_runtime_installed(&self, runtime: &Runtime) -> Result<bool, Error> {
        Ok(*self.global_installed.get(runtime).unwrap_or(&false))
    }

    fn resolve_current_runtime_version(&self, spec: &RuntimeSpec) -> Result<RuntimeSpec, Error> {
        let runtime = spec.runtime();
        let selector = spec.selector();
        let version = self
            .selector_current
            .get(&(runtime.clone(), selector.to_string()))
            .cloned()
            .ok_or_else(|| Error::MiseCommandFailed {
                command: format!(
                    "mise exec {} -- mise current {}",
                    spec.runtime_pin(),
                    runtime.as_ref()
                ),
                stderr: "no fake selector->current mapping".to_string(),
            })?;

        if version.is_empty() {
            return Err(invariant!(
                "fake selector->current mapping for '{runtime}' returned empty version"
            ));
        }

        Ok(RuntimeSpec::new(runtime.clone(), version))
    }

    fn install_global(&self, tool_spec: &ToolSpec, _project_dir: &Path) -> Result<(), Error> {
        let Some(version) = tool_spec.version() else {
            return Err(invariant!(
                "fake global install requires exact tool spec, got '{tool_spec}'"
            ));
        };

        self.global_packages
            .borrow_mut()
            .insert(tool_spec.tool_id().to_string(), version.to_string());

        Ok(())
    }

    fn install_into(
        &self,
        _runtime_versions: &RuntimePins,
        tool_spec: &ToolSpec,
        target_dir: &Path,
    ) -> Result<(), Error> {
        let Some(package) = self.packages.get(&tool_spec.to_string()) else {
            return Ok(());
        };

        let bin_dir = target_dir.join("bin");
        self._fs.mkdir_p(&bin_dir)?;

        for bin in &package.bins {
            let path = bin_dir.join(bin);
            self._fs
                .write_executable_file(&path, "#!/bin/sh\nexit 0\n")?;
        }

        Ok(())
    }

    fn installed_command_targets(
        &self,
        tool_spec: &ToolSpec,
        _project_dir: &Path,
    ) -> Result<BTreeMap<String, CommandTarget>, Error> {
        let commands = self
            .packages
            .get(&tool_spec.to_string())
            .map(|package| package.bins.clone())
            .unwrap_or_default();

        Ok(commands
            .into_iter()
            .filter(|command| !command.is_empty())
            .map(|command| {
                let target = _project_dir.join(".npm-global").join(&command);
                (command, CommandTarget::env_wrapped(target))
            })
            .collect())
    }

    fn installed_global_package_version(
        &self,
        tool_id: &ToolId,
        _project_dir: &Path,
    ) -> Result<Option<String>, Error> {
        Ok(self
            .global_packages
            .borrow()
            .get(&tool_id.to_string())
            .cloned())
    }

    fn uninstall_global(&self, tool_id: &ToolId, project_dir: &Path) -> Result<(), Error> {
        if tool_id.backend().as_ref() != "npm" {
            return Ok(());
        }

        self.global_uninstalls.borrow_mut().push(GlobalUninstall {
            tool_id: tool_id.clone(),
            project_dir: project_dir.to_path_buf(),
        });
        Ok(())
    }

    fn bin_paths(&self, _tool_id: &ToolId, install_dir: &Path) -> Result<Vec<PathBuf>, Error> {
        let bin_dir = install_dir.join("bin");
        if bin_dir.exists() {
            return Ok(vec![bin_dir]);
        }

        Ok(vec![])
    }

    fn trust_config(&self, _config_path: &Path) -> Result<(), Error> {
        Ok(())
    }
}
