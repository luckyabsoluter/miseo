//! Loaded `~/.miseo` workspace state and the high-level operations on it.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;

use crate::{
    error::{Error, invariant},
    fs::{Fs, Path, PathBuf},
    spec::{Runtime, RuntimePins, RuntimeSpec, ToolId, ToolLayout, ToolSpec, VariantLayout},
};

pub mod manifest;
use manifest::Manifest;

/// Install metadata captured after a successful install/upgrade.
#[derive(Debug, Clone)]
pub struct InstallRecord {
    /// Stable tool identity (`<backend>:<name>`).
    pub tool_id: ToolId,
    /// Variant key (`<pkg-version>+<runtime tuple>`).
    pub variant_key: String,
    /// Resolved package version.
    pub package_version: String,
    /// Concrete runtime versions pinned for this variant.
    pub runtimes: BTreeMap<Runtime, String>,
    /// Filesystem install directory for this variant.
    pub install_dir: PathBuf,
    /// Command names exported by this variant.
    pub commands: Vec<String>,
    /// Previously owned command names no longer exported.
    pub stale_commands: Vec<String>,
    /// How package content was installed for this variant.
    pub install_mode: InstallMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    Isolated,
    Global,
}

impl InstallMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Isolated => "isolated",
            Self::Global => "global",
        }
    }

    pub fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "isolated" => Ok(Self::Isolated),
            "global" => Ok(Self::Global),
            _ => Err(invariant!("unknown install mode '{value}'")),
        }
    }
}

/// Result of removing a managed tool from workspace state.
#[derive(Debug, Clone)]
pub struct RemovedTool {
    /// Filesystem tool key used under `~/.miseo`.
    pub tool_key: String,
    /// Public command names that were owned by the removed tool.
    pub removed_commands: Vec<String>,
}

/// Result of pruning one non-current variant.
#[derive(Debug, Clone)]
pub struct RemovedVariant {
    /// Variant key removed from manifest.
    pub key: String,
    /// Variant install directory that should be deleted from disk.
    pub install_dir: PathBuf,
}

/// Install plan derived from requested package/runtime selection.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    tool_id: ToolId,
    package_version: String,
    runtime_pins: RuntimePins,
    variant_key: String,
    variant: VariantLayout,
    had_tool_before: bool,
    current_matches: bool,
}

impl InstallPlan {
    pub fn tool_id(&self) -> &ToolId {
        &self.tool_id
    }

    pub fn package_version(&self) -> &str {
        &self.package_version
    }

    pub fn runtime_pins(&self) -> &RuntimePins {
        &self.runtime_pins
    }

    pub fn runtime_labels(&self) -> Vec<String> {
        self.runtime_pins
            .values()
            .map(|pin| pin.runtime_pin())
            .collect()
    }

    pub fn variant(&self) -> &VariantLayout {
        &self.variant
    }

    pub fn variant_key(&self) -> &str {
        &self.variant_key
    }

    pub fn current_matches(&self) -> bool {
        self.current_matches
    }
}

/// Loaded `~/.miseo` workspace state plus filesystem access.
pub struct Workspace {
    root: PathBuf,
    fs: &'static dyn Fs,
    manifest: Manifest,
    dirty: bool,
}

impl Workspace {
    pub fn open(root: PathBuf, fs: &'static dyn Fs) -> Result<Self, Error> {
        let manifest = Manifest::load(&root.join(".miseo-installs.toml"))?;

        Ok(Self {
            root,
            fs,
            manifest,
            dirty: false,
        })
    }

    #[cfg(test)]
    pub fn from_manifest(root: PathBuf, fs: &'static dyn Fs, manifest: Manifest) -> Self {
        Self {
            root,
            fs,
            manifest,
            dirty: false,
        }
    }

    pub fn commit(self) -> Result<(), Error> {
        if !self.dirty {
            return Ok(());
        }

        self.manifest.save_atomic(&self.manifest_path())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn layout_for_id(&self, tool_id: &ToolId) -> ToolLayout {
        tool_id.layout(self.root())
    }

    pub fn layout_for_spec(&self, spec: &ToolSpec) -> ToolLayout {
        spec.layout(self.root())
    }

    pub fn plan_install(
        &self,
        spec: &ToolSpec,
        runtime_pins: &RuntimePins,
    ) -> Result<InstallPlan, Error> {
        let Some(package_version) = spec.version() else {
            return Err(invariant!(
                "install plan requires exact tool spec with version, got '{spec}'"
            ));
        };

        let tool_id = spec.tool_id().clone();
        let variant_key = install_variant_key(package_version, runtime_pins);
        let variant = self.layout_for_spec(spec).variant(&variant_key);
        let had_tool_before = self.has(&tool_id);
        let current_matches = self.variant_is_current(&tool_id, &variant_key);

        Ok(InstallPlan {
            tool_id,
            package_version: package_version.to_string(),
            runtime_pins: runtime_pins.clone(),
            variant_key,
            variant,
            had_tool_before,
            current_matches,
        })
    }

    pub fn initialize_variant(
        &self,
        variant: &VariantLayout,
        runtime_pins: &RuntimePins,
    ) -> Result<PathBuf, Error> {
        self.ensure_variant_dirs(variant)?;
        self.write_variant_mise_toml(variant.variant_dir(), runtime_pins)
    }

    fn ensure_variant_dirs(&self, variant: &VariantLayout) -> Result<(), Error> {
        self.fs.mkdir_p(self.root())?;
        self.fs.mkdir_p(variant.root_bin_dir())?;
        self.fs.mkdir_p(variant.tool_dir())?;
        self.fs.mkdir_p(variant.variant_dir())?;
        Ok(())
    }

    fn write_variant_mise_toml(
        &self,
        variant_dir: &Path,
        runtime_pins: &RuntimePins,
    ) -> Result<PathBuf, Error> {
        let tools = runtime_pins
            .iter()
            .map(|(runtime, pin)| format!(r#"{} = "{}""#, runtime.as_ref(), pin.selector()))
            .collect::<Vec<_>>()
            .join("\n");
        let content = format!("[tools]\n{tools}\n");
        let path = variant_dir.join("mise.toml");
        self.fs.write_file(&path, &content)?;
        Ok(path)
    }

    pub fn discover_executables(
        &self,
        bin_paths: &[PathBuf],
    ) -> Result<BTreeMap<String, PathBuf>, Error> {
        let mut map = BTreeMap::new();
        let mut seen = BTreeSet::new();

        for dir in bin_paths {
            if !dir.exists() {
                continue;
            }

            for path in self.fs.ls(dir)? {
                if !self.fs.is_executable(&path)? {
                    continue;
                }

                let Some(name) = command_name(&path) else {
                    continue;
                };

                if seen.insert(name.clone()) {
                    map.insert(name, path);
                }
            }
        }

        Ok(map)
    }

    fn point_current_variant(
        &self,
        variant: &VariantLayout,
        variant_key: &str,
    ) -> Result<(), Error> {
        self.fs
            .ln_s(&PathBuf::from(variant_key), variant.current_link())
    }

    fn link_discovered_commands(
        &self,
        variant: &VariantLayout,
        discovered: &BTreeMap<String, PathBuf>,
    ) -> Result<(), Error> {
        self.fs.mkdir_p(variant.local_bin_dir())?;

        for (command, target) in discovered {
            let local_cmd = variant.local_command(command);
            self.fs
                .write_mise_env_shim(variant.variant_dir(), target, &local_cmd)?;

            self.fs.ln_s(
                &variant.public_target(command),
                &variant.public_command(command),
            )?;
        }

        Ok(())
    }

    pub fn finalize_install(
        &mut self,
        plan: &InstallPlan,
        install_mode: InstallMode,
        discovered: BTreeMap<String, PathBuf>,
    ) -> Result<Vec<String>, Error> {
        let exported_commands = discovered.keys().cloned().collect::<Vec<_>>();
        if exported_commands.is_empty() {
            return Err(invariant!(
                "no executable commands discovered for '{}'",
                plan.tool_id()
            ));
        }

        self.assert_no_conflicts(plan.tool_id(), &exported_commands)?;

        let previous_commands = self.current_commands(plan.tool_id());
        let stale_commands = stale_commands(&previous_commands, &exported_commands);

        self.link_discovered_commands(plan.variant(), &discovered)?;
        self.point_current_variant(plan.variant(), plan.variant_key())?;
        self.unlink_owned_commands(plan.tool_id(), &stale_commands)?;

        self.record_install(InstallRecord {
            tool_id: plan.tool_id().clone(),
            variant_key: plan.variant_key().to_string(),
            package_version: plan.package_version().to_string(),
            runtimes: plan
                .runtime_pins()
                .iter()
                .map(|(runtime, pin)| (runtime.clone(), pin.selector().to_string()))
                .collect(),
            install_dir: plan.variant().variant_dir().to_path_buf(),
            commands: exported_commands.clone(),
            stale_commands,
            install_mode,
        });

        Ok(exported_commands)
    }

    fn unlink_owned_commands(&self, tool_id: &ToolId, commands: &[String]) -> Result<(), Error> {
        for command in commands {
            if !self.owns_command(tool_id, command) {
                continue;
            }

            self.fs.rm(&self.root().join(".bin").join(command))?;
        }

        Ok(())
    }

    fn unlink_commands(&self, commands: &[String]) -> Result<(), Error> {
        let root_bin_dir = self.root().join(".bin");
        for command in commands {
            self.fs.rm(&root_bin_dir.join(command))?;
        }
        Ok(())
    }

    fn remove_tool_dir(&self, tool_key: &str) -> Result<(), Error> {
        self.fs.rm_rf(&self.root().join(tool_key))
    }

    pub fn cleanup_plan(&self, plan: &InstallPlan) {
        if plan.had_tool_before {
            return;
        }

        let _ = self.fs.rm_rf(plan.variant().tool_dir());
    }

    fn cleanup_orphan(&self, tool_key: &str, orphan_tool_dir: &Path) -> Result<Vec<String>, Error> {
        let root_bin_dir = self.root().join(".bin");
        let mut removed = vec![];

        if root_bin_dir.exists() {
            for path in self.fs.ls(&root_bin_dir)? {
                let Some(target) = self.fs.readlink(&path)? else {
                    continue;
                };

                if !link_points_into_tool_dir(&target, tool_key) {
                    continue;
                }

                if let Some(name) = path.file_name() {
                    removed.push(name.to_string());
                }

                self.fs.rm(&path)?;
            }
        }

        self.fs.rm_rf(orphan_tool_dir)?;
        removed.sort();
        Ok(removed)
    }

    pub fn uninstall(&mut self, tool_id: &ToolId, force: bool) -> Result<Vec<String>, Error> {
        if let Some(removed) = self.untrack(tool_id) {
            self.unlink_commands(&removed.removed_commands)?;
            self.remove_tool_dir(&removed.tool_key)?;
            return Ok(removed.removed_commands);
        }

        let layout = self.layout_for_id(tool_id);
        let orphan_tool_dir = layout.tool_dir();
        if !orphan_tool_dir.exists() {
            return Err(Error::ToolNotInstalled {
                tool_id: tool_id.to_string(),
            });
        }

        if !force {
            return Err(Error::ToolNotInstalledOrphanFound {
                tool_id: tool_id.to_string(),
                path: orphan_tool_dir.to_string(),
            });
        }

        self.cleanup_orphan(layout.tool_key(), &orphan_tool_dir)
    }

    fn has(&self, tool_id: &ToolId) -> bool {
        self.manifest.has(tool_id)
    }

    fn variant_is_current(&self, tool_id: &ToolId, variant_key: &str) -> bool {
        self.manifest
            .tool_by_id(tool_id)
            .is_some_and(|tool| tool.current_variant == variant_key)
    }

    pub fn upgrade_uses(&self, tool_id: &ToolId, uses: RuntimePins) -> Result<RuntimePins, Error> {
        if !uses.is_empty() {
            return Ok(uses);
        }

        let current = self.current_variant(tool_id)?;
        let mut uses = RuntimePins::new();
        for (runtime, pin) in &current.runtimes {
            uses.insert(
                runtime.as_str().into(),
                RuntimeSpec::new(runtime.as_str().into(), pin.clone()),
            );
        }

        Ok(uses)
    }

    pub fn prune_variants(&mut self, tool_id: &ToolId) -> Result<Vec<RemovedVariant>, Error> {
        let Some(tool) = self.manifest.tool_mut_by_id(tool_id) else {
            return Ok(vec![]);
        };

        let keep = tool.current_variant.clone();
        if !tool.variants.contains_key(&keep) {
            return Err(invariant!(
                "tool '{tool_id}' current variant '{current_variant}' missing",
                current_variant = tool.current_variant
            ));
        }

        let keys = tool
            .variants
            .keys()
            .filter(|key| key.as_str() != keep)
            .cloned()
            .collect::<Vec<_>>();

        let mut removed = vec![];
        for key in keys {
            if let Some(variant) = tool.variants.shift_remove(&key) {
                removed.push(RemovedVariant {
                    key,
                    install_dir: PathBuf::from(variant.install_dir),
                });
            }
        }

        if !removed.is_empty() {
            self.mark_dirty();
            removed.sort_by(|a, b| a.key.cmp(&b.key));
        }

        Ok(removed)
    }

    pub fn prune_variants_and_cleanup(&mut self, tool_id: &ToolId) -> Result<Vec<String>, Error> {
        let removed = self.prune_variants(tool_id)?;
        for variant in &removed {
            self.fs.rm_rf(&variant.install_dir)?;
        }

        Ok(removed.into_iter().map(|variant| variant.key).collect())
    }

    pub fn current_install_mode(&self, tool_id: &ToolId) -> Result<InstallMode, Error> {
        let current = self.current_variant(tool_id)?;
        InstallMode::parse(&current.install_mode)
    }

    pub fn global_uninstall_project_dirs(&self, tool_id: &ToolId) -> Result<Vec<PathBuf>, Error> {
        let Some(tool) = self.manifest.tool_by_id(tool_id) else {
            return Ok(vec![]);
        };

        let mut dirs = vec![];
        for variant in tool.variants.values() {
            if InstallMode::parse(&variant.install_mode)? == InstallMode::Global {
                dirs.push(PathBuf::from(variant.install_dir.clone()));
            }
        }

        Ok(dirs)
    }

    fn untrack(&mut self, tool_id: &ToolId) -> Option<RemovedTool> {
        let removed_commands = self.manifest.owned_commands_by_id(tool_id);
        let tool = self.manifest.remove_tool_by_id(tool_id)?;

        for command in &removed_commands {
            self.manifest.remove_owner(command);
        }

        self.mark_dirty();
        Some(RemovedTool {
            tool_key: tool.tool_key,
            removed_commands,
        })
    }

    fn owns_command(&self, tool_id: &ToolId, command: &str) -> bool {
        let Some(owner) = self.manifest.owner(command) else {
            return false;
        };

        owner == tool_id.to_string()
    }

    fn current_commands(&self, tool_id: &ToolId) -> Vec<String> {
        self.manifest.current_commands(tool_id)
    }

    fn assert_no_conflicts(&self, tool_id: &ToolId, commands: &[String]) -> Result<(), Error> {
        self.manifest.assert_no_conflicts(tool_id, commands)
    }

    pub fn record_install(&mut self, record: InstallRecord) {
        let mut runtimes = IndexMap::new();
        for (runtime, pin) in &record.runtimes {
            runtimes.insert(runtime.as_ref().to_string(), pin.clone());
        }

        let variant = manifest::VariantEntry {
            package_spec: record.tool_id.to_string(),
            package_version: record.package_version,
            runtimes,
            install_dir: record.install_dir.to_string(),
            commands: record.commands,
            install_mode: record.install_mode.as_str().to_string(),
        };

        self.mark_dirty();
        self.manifest.upsert_install(manifest::UpsertInstall {
            tool_id: record.tool_id,
            variant_key: record.variant_key,
            variant,
            stale_commands: record.stale_commands,
        });
    }

    fn current_variant(&self, tool_id: &ToolId) -> Result<&manifest::VariantEntry, Error> {
        let Some(tool) = self.manifest.tool_by_id(tool_id) else {
            return Err(Error::ToolNotInstalled {
                tool_id: tool_id.to_string(),
            });
        };

        let Some(current_variant) = tool.variants.get(&tool.current_variant) else {
            return Err(invariant!(
                "tool '{tool_id}' current variant '{current_variant}' missing",
                current_variant = tool.current_variant
            ));
        };

        Ok(current_variant)
    }

    fn manifest_path(&self) -> PathBuf {
        self.root().join(".miseo-installs.toml")
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

fn install_variant_key(package_version: &str, runtimes: &RuntimePins) -> String {
    let pins = runtimes
        .values()
        .map(|pin| pin.runtime().with_version(pin.selector()))
        .collect::<Vec<_>>()
        .join("+");

    format!("{package_version}+{pins}")
}

fn stale_commands(previous: &[String], current: &[String]) -> Vec<String> {
    let current = current.iter().cloned().collect::<BTreeSet<_>>();

    previous
        .iter()
        .filter(|cmd| !current.contains(*cmd))
        .cloned()
        .collect()
}

fn link_points_into_tool_dir(target: &Path, tool_key: &str) -> bool {
    target
        .components()
        .any(|component| component.as_str() == tool_key)
}

// Return the public command name for a discovered executable path.
//
// Unix commands are usually extensionless, so `bin/prettier` exports `prettier`.
// Windows packages often expose launcher files, so `bin/prettier.cmd` and
// `bin/prettier.ps1` both export `prettier`. Other extension-bearing files are
// not commands: `bin/mise.toml` returns `None` even if Windows ACLs allow
// execute access.
fn command_name(path: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        if let Some(ext) = path.extension() {
            if windows_command_extension(ext) {
                return path.file_stem().map(str::to_string);
            }

            return None;
        }
    }

    path.file_name().map(str::to_string)
}

#[cfg(windows)]
fn windows_command_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "bat" | "cmd" | "com" | "exe" | "ps1"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::{TempDir, tempdir};

    use crate::{
        error::Error,
        fs::{Path, PathBuf},
        spec::{Runtime, RuntimePins, RuntimeSpec, ToolId, ToolSpec},
    };

    use super::{InstallMode, InstallRecord, Workspace, manifest::Manifest};

    struct TestWorkspace {
        root: PathBuf,
        workspace: Workspace,
        _tmp: TempDir,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let _tmp = tempdir().unwrap();
            let root = PathBuf::from_path_buf(_tmp.path().to_path_buf()).unwrap();
            let workspace =
                Workspace::from_manifest(root.clone(), crate::fs::new(), Manifest::empty());

            Self {
                root,
                workspace,
                _tmp,
            }
        }

        fn seeded() -> Self {
            let mut test = Self::new();
            test.record(prettier_record(&test.root, "3.8.1", ["prettier"]));
            test
        }

        fn path(&self, rel: impl AsRef<Path>) -> PathBuf {
            self.root.join(rel)
        }

        fn fs(&self) -> &'static dyn crate::fs::Fs {
            crate::fs::new()
        }

        fn record(&mut self, record: InstallRecord) {
            self.workspace.record_install(record);
        }
    }

    impl std::ops::Deref for TestWorkspace {
        type Target = Workspace;

        fn deref(&self) -> &Self::Target {
            &self.workspace
        }
    }

    impl std::ops::DerefMut for TestWorkspace {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.workspace
        }
    }

    fn node_runtimes(version: &str) -> BTreeMap<Runtime, String> {
        BTreeMap::from([(Runtime::Node, version.to_string())])
    }

    fn prettier_record(
        root: &Path,
        version: &str,
        commands: impl IntoIterator<Item = &'static str>,
    ) -> InstallRecord {
        let variant_key = format!("{version}+node-24.13.1");

        InstallRecord {
            tool_id: "npm:prettier".parse().unwrap(),
            variant_key: variant_key.clone(),
            package_version: version.to_string(),
            runtimes: node_runtimes("24.13.1"),
            install_dir: root.join("npm-prettier").join(&variant_key),
            commands: commands.into_iter().map(str::to_string).collect(),
            stale_commands: vec![],
            install_mode: InstallMode::Isolated,
        }
    }

    #[test]
    fn upgrade_uses_prefers_explicit_inputs() {
        let workspace = TestWorkspace::seeded();
        let tool_id = "npm:prettier".parse::<ToolId>().unwrap();
        let explicit = RuntimePins::try_from(vec![RuntimeSpec::new(Runtime::Node, "22")]).unwrap();

        let resolved = workspace.upgrade_uses(&tool_id, explicit.clone()).unwrap();
        assert_eq!(resolved, explicit);
    }

    #[test]
    fn upgrade_uses_reconstructs_current_variant_when_not_explicit() {
        let workspace = TestWorkspace::seeded();
        let tool_id = "npm:prettier".parse::<ToolId>().unwrap();

        let resolved = workspace
            .upgrade_uses(&tool_id, RuntimePins::new())
            .unwrap();

        assert_eq!(
            resolved,
            RuntimePins::try_from(vec![RuntimeSpec::new(Runtime::Node, "24.13.1")]).unwrap()
        );
    }

    #[test]
    fn upgrade_uses_errors_when_tool_is_not_installed() {
        let workspace = TestWorkspace::new();
        let tool_id = "npm:prettier".parse::<ToolId>().unwrap();

        let err = workspace
            .upgrade_uses(&tool_id, RuntimePins::new())
            .unwrap_err();

        assert!(matches!(err, Error::ToolNotInstalled { .. }));
    }

    #[test]
    fn plan_install_uses_sorted_runtime_tuple_in_variant_key() {
        let workspace = TestWorkspace::seeded();
        let spec: ToolSpec = "npm:prettier@1.2.3".parse().unwrap();

        let mut runtimes = RuntimePins::new();
        runtimes.insert(
            Runtime::Ruby,
            RuntimeSpec::new(Runtime::Ruby, "3.3.0".to_string()),
        );
        runtimes.insert(
            Runtime::Node,
            RuntimeSpec::new(Runtime::Node, "22.13.1".to_string()),
        );

        let plan = workspace.plan_install(&spec, &runtimes).unwrap();
        assert_eq!(plan.variant_key(), "1.2.3+node-22.13.1+ruby-3.3.0");
    }

    #[test]
    fn prune_variants_keeps_current_variant_only() {
        let mut workspace = TestWorkspace::seeded();
        let tool_id = "npm:prettier".parse::<ToolId>().unwrap();

        workspace.record(prettier_record(workspace.root(), "3.8.2", ["prettier"]));

        let removed = workspace.prune_variants(&tool_id).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].key, "3.8.1+node-24.13.1");
        assert_eq!(
            removed[0].install_dir,
            workspace.path("npm-prettier/3.8.1+node-24.13.1")
        );

        let tool = workspace.manifest.tool("npm:prettier").unwrap();
        assert_eq!(tool.current_variant, "3.8.2+node-24.13.1");
        assert_eq!(tool.variants.len(), 1);
    }

    #[test]
    fn untrack_drops_tool_and_owners() {
        let mut workspace = TestWorkspace::seeded();
        let tool_id = "npm:prettier".parse::<ToolId>().unwrap();

        let removed = workspace.untrack(&tool_id).unwrap();
        assert_eq!(removed.tool_key, "npm-prettier");
        assert_eq!(removed.removed_commands, vec!["prettier".to_string()]);

        assert!(!workspace.has(&tool_id));
        assert!(!workspace.owns_command(&tool_id, "prettier"));
    }

    #[test]
    fn initialize_variant_writes_expected_tools_table() {
        let workspace = TestWorkspace::seeded();
        let layout = workspace.layout_for_spec(&"npm:prettier".parse::<ToolSpec>().unwrap());
        let variant = layout.variant("3.8.1+node-24.13.1");

        let mut pins = RuntimePins::new();
        pins.insert(
            Runtime::Node,
            RuntimeSpec::new(Runtime::Node, "24.13.1".to_string()),
        );
        pins.insert(
            Runtime::Python,
            RuntimeSpec::new(Runtime::Python, "3.12.8".to_string()),
        );

        let path = workspace.initialize_variant(&variant, &pins).unwrap();
        let content = workspace.fs().read_file(&path).unwrap();

        assert_eq!(
            content,
            "[tools]\nnode = \"24.13.1\"\npython = \"3.12.8\"\n"
        );
    }

    #[test]
    fn discover_executables_collects_unique_executables() {
        let workspace = TestWorkspace::new();
        let fs = workspace.fs();

        let a = workspace.path("a");
        let b = workspace.path("b");
        fs.mkdir_p(&a).unwrap();
        fs.mkdir_p(&b).unwrap();

        let exe_a = a.join("foo");
        let exe_b = b.join("foo");
        let txt = b.join("not-exec.txt");
        let config = b.join("mise.toml");

        fs.write_executable_file(&exe_a, "a").unwrap();
        fs.write_executable_file(&exe_b, "b").unwrap();
        fs.write_file(&txt, "x").unwrap();
        fs.write_file(&config, "x").unwrap();

        let map = workspace.discover_executables(&[a, b]).unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("foo"));
    }

    #[test]
    fn unlink_owned_commands_only_removes_owned_links() {
        let mut workspace = TestWorkspace::new();
        let fs = workspace.fs();

        let bin_dir = workspace.path(".bin");
        fs.mkdir_p(&bin_dir).unwrap();

        let owned_target = workspace.path("npm-prettier/current/.miseo/prettier");
        let foreign_target = workspace.path("other-tool/current/.miseo/prettierd");
        fs.mkdir_p(owned_target.parent().unwrap()).unwrap();
        fs.mkdir_p(foreign_target.parent().unwrap()).unwrap();
        fs.write_file(&owned_target, "").unwrap();
        fs.write_file(&foreign_target, "").unwrap();

        fs.ln_s(&owned_target, &bin_dir.join("prettier")).unwrap();
        fs.ln_s(&foreign_target, &bin_dir.join("prettierd"))
            .unwrap();

        workspace.record(prettier_record(workspace.root(), "3.8.1", ["prettier"]));

        workspace
            .unlink_owned_commands(
                &"npm:prettier".parse::<ToolId>().unwrap(),
                &["prettier".to_string(), "prettierd".to_string()],
            )
            .unwrap();

        assert!(!fs.exists(&bin_dir.join("prettier")).unwrap());
        assert!(fs.exists(&bin_dir.join("prettierd")).unwrap());
    }

    #[test]
    fn prune_variants_and_cleanup_removes_old_variant_dirs() {
        let mut workspace = TestWorkspace::new();
        let tool_id = "npm:prettier".parse::<ToolId>().unwrap();
        let fs = workspace.fs();

        let old_dir = workspace.path("npm-prettier/3.8.1+node-24.13.1");
        let new_dir = workspace.path("npm-prettier/3.8.2+node-24.13.1");
        fs.mkdir_p(&old_dir).unwrap();
        fs.mkdir_p(&new_dir).unwrap();

        workspace.record(prettier_record(workspace.root(), "3.8.1", ["prettier"]));
        workspace.record(prettier_record(workspace.root(), "3.8.2", ["prettier"]));

        let removed = workspace.prune_variants_and_cleanup(&tool_id).unwrap();
        assert_eq!(removed, vec!["3.8.1+node-24.13.1".to_string()]);
        assert!(!fs.exists(&old_dir).unwrap());
        assert!(fs.exists(&new_dir).unwrap());
    }

    #[test]
    fn uninstall_removes_managed_links_and_directory() {
        let mut workspace = TestWorkspace::new();
        let tool_id = "npm:prettier".parse::<ToolId>().unwrap();
        let fs = workspace.fs();

        let tool_key = "npm-prettier";
        let variant_key = "3.8.1+node-24.13.1";
        let install_dir = workspace.path(tool_key).join(variant_key);
        let local_cmd = install_dir.join(".miseo/prettier");

        fs.mkdir_p(&workspace.path(".bin")).unwrap();
        fs.mkdir_p(local_cmd.parent().unwrap()).unwrap();
        fs.write_file(&local_cmd, "").unwrap();
        fs.ln_s(&local_cmd, &workspace.path(".bin/prettier"))
            .unwrap();

        workspace.record(prettier_record(workspace.root(), "3.8.1", ["prettier"]));

        let removed = workspace.uninstall(&tool_id, false).unwrap();
        assert_eq!(removed, vec!["prettier".to_string()]);
        assert!(!fs.exists(&workspace.path(".bin/prettier")).unwrap());
        assert!(!fs.exists(&workspace.path(tool_key)).unwrap());
    }
}
