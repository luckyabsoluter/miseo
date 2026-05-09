//! Workspace manifest schema, validation, and persistence helpers.

use std::fs;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, invariant},
    fs::Path,
    spec::ToolId,
};

use super::InstallMode;

pub const SCHEMA_VERSION: u64 = 1;

/// Serialized workspace manifest (`~/.miseo/.miseo-installs.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Manifest {
    schema_version: u64,
    #[serde(default)]
    tools: IndexMap<String, ToolEntry>,
    #[serde(default)]
    owners: IndexMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ToolEntry {
    pub(super) tool_key: String,
    pub(super) backend: String,
    pub(super) name: String,
    pub(super) current_variant: String,
    pub(super) variants: IndexMap<String, VariantEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct VariantEntry {
    pub(super) package_spec: String,
    pub(super) package_version: String,
    pub(super) runtimes: IndexMap<String, String>,
    pub(super) install_dir: String,
    pub(super) commands: Vec<String>,
    #[serde(default = "default_install_mode")]
    pub(super) install_mode: String,
}

#[derive(Debug, Clone)]
pub(super) struct UpsertInstall {
    pub(super) tool_id: ToolId,
    pub(super) variant_key: String,
    pub(super) variant: VariantEntry,
    pub(super) stale_commands: Vec<String>,
}

impl Manifest {
    pub(crate) fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            tools: IndexMap::new(),
            owners: IndexMap::new(),
        }
    }

    pub(crate) fn load(path: &Path) -> Result<Self, Error> {
        if !path.exists() {
            return Ok(Self::empty());
        }

        let raw = fs::read_to_string(path)?;
        let manifest: Manifest = toml::from_str(&raw)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub(crate) fn save_atomic(&self, path: &Path) -> Result<(), Error> {
        self.validate()?;

        let parent = path
            .parent()
            .ok_or_else(|| invariant!("manifest path '{path}' has no parent"))?;
        fs::create_dir_all(parent)?;

        let tmp = path.with_extension("toml.tmp");
        let rendered = toml::to_string_pretty(self)?;
        fs::write(tmp.as_std_path(), rendered)?;
        fs::rename(tmp.as_std_path(), path.as_std_path())?;
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        self.validate_schema()?;
        self.validate_tools()?;
        self.validate_owners()?;
        Ok(())
    }

    fn has_str(&self, tool_id: &str) -> bool {
        self.tools.contains_key(tool_id)
    }

    pub(super) fn has(&self, tool_id: &ToolId) -> bool {
        self.has_str(&tool_id.to_string())
    }

    pub(super) fn tool(&self, tool_id: &str) -> Option<&ToolEntry> {
        self.tools.get(tool_id)
    }

    pub(super) fn tool_by_id(&self, tool_id: &ToolId) -> Option<&ToolEntry> {
        self.tool(&tool_id.to_string())
    }

    fn tool_mut(&mut self, tool_id: &str) -> Option<&mut ToolEntry> {
        self.tools.get_mut(tool_id)
    }

    pub(super) fn tool_mut_by_id(&mut self, tool_id: &ToolId) -> Option<&mut ToolEntry> {
        self.tool_mut(&tool_id.to_string())
    }

    fn remove_tool(&mut self, tool_id: &str) -> Option<ToolEntry> {
        self.tools.shift_remove(tool_id)
    }

    pub(super) fn remove_tool_by_id(&mut self, tool_id: &ToolId) -> Option<ToolEntry> {
        self.remove_tool(&tool_id.to_string())
    }

    pub(super) fn owner(&self, command: &str) -> Option<&str> {
        self.owners.get(command).map(String::as_str)
    }

    pub(super) fn remove_owner(&mut self, command: &str) -> Option<String> {
        self.owners.shift_remove(command)
    }

    fn owned_commands(&self, tool_id: &str) -> Vec<String> {
        let mut commands = self
            .owners
            .iter()
            .filter(|(_, owner)| *owner == tool_id)
            .map(|(command, _)| command.clone())
            .collect::<Vec<_>>();
        commands.sort();
        commands
    }

    pub(super) fn owned_commands_by_id(&self, tool_id: &ToolId) -> Vec<String> {
        self.owned_commands(&tool_id.to_string())
    }

    #[cfg(test)]
    pub(crate) fn tools_is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn owners_is_empty(&self) -> bool {
        self.owners.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn has_tool_id_str(&self, tool_id: &str) -> bool {
        self.tools.contains_key(tool_id)
    }

    #[cfg(test)]
    pub(crate) fn current_variant_str(&self, tool_id: &str) -> Option<&str> {
        self.tools
            .get(tool_id)
            .map(|tool| tool.current_variant.as_str())
    }

    #[cfg(test)]
    pub(crate) fn variant_count_str(&self, tool_id: &str) -> usize {
        self.tools
            .get(tool_id)
            .map_or(0, |tool| tool.variants.len())
    }

    #[cfg(test)]
    pub(crate) fn command_owner_str(&self, command: &str) -> Option<&str> {
        self.owners.get(command).map(String::as_str)
    }

    #[cfg(test)]
    pub(crate) fn current_install_mode_str(&self, tool_id: &str) -> Option<&str> {
        self.tools
            .get(tool_id)
            .and_then(|tool| tool.variants.get(&tool.current_variant))
            .map(|variant| variant.install_mode.as_str())
    }

    fn current_commands_str(&self, tool_id: &str) -> Vec<String> {
        self.tools
            .get(tool_id)
            .and_then(|tool| tool.variants.get(&tool.current_variant))
            .map(|v| v.commands.clone())
            .unwrap_or_default()
    }

    pub(super) fn current_commands(&self, tool_id: &ToolId) -> Vec<String> {
        self.current_commands_str(&tool_id.to_string())
    }

    fn assert_no_conflicts_str(&self, tool_id: &str, commands: &[String]) -> Result<(), Error> {
        for cmd in commands {
            if let Some(owner) = self.owners.get(cmd)
                && owner != tool_id
            {
                return Err(Error::CommandOwnershipConflict {
                    command: cmd.clone(),
                    owner: owner.clone(),
                    requested: tool_id.to_string(),
                });
            }
        }
        Ok(())
    }

    pub(super) fn assert_no_conflicts(
        &self,
        tool_id: &ToolId,
        commands: &[String],
    ) -> Result<(), Error> {
        self.assert_no_conflicts_str(&tool_id.to_string(), commands)
    }

    pub(super) fn upsert_install(&mut self, install: UpsertInstall) {
        let UpsertInstall {
            tool_id,
            variant_key,
            variant,
            stale_commands,
        } = install;

        let tool_id_str = tool_id.to_string();
        let tool_key = tool_id.key();
        let backend = tool_id.backend().as_ref().to_string();
        let name = tool_id.name().to_string();
        let commands = variant.commands.clone();

        self.upsert_tool_entry(&tool_id_str, tool_key, backend, name, variant_key, variant);
        self.clear_stale_owned_commands(&tool_id_str, &stale_commands);
        self.assign_owned_commands(&tool_id_str, &commands);
    }

    fn validate_schema(&self) -> Result<(), Error> {
        if self.schema_version == SCHEMA_VERSION {
            return Ok(());
        }

        Err(invariant!(
            "unsupported schema_version {current}, expected {SCHEMA_VERSION}",
            current = self.schema_version
        ))
    }

    fn validate_tools(&self) -> Result<(), Error> {
        for (tool_id, tool) in &self.tools {
            for variant in tool.variants.values() {
                InstallMode::parse(&variant.install_mode)?;
            }

            if tool.variants.contains_key(&tool.current_variant) {
                continue;
            }

            return Err(invariant!(
                "tool '{tool_id}' current_variant '{current_variant}' missing from variants",
                current_variant = tool.current_variant
            ));
        }

        Ok(())
    }

    fn validate_owners(&self) -> Result<(), Error> {
        for (command, owner_tool_id) in &self.owners {
            let Some(tool) = self.tools.get(owner_tool_id) else {
                return Err(invariant!(
                    "owners entry '{command}' references missing tool '{owner_tool_id}'"
                ));
            };

            let Some(current) = tool.variants.get(&tool.current_variant) else {
                return Err(invariant!(
                    "tool '{owner_tool_id}' current variant '{current_variant}' missing",
                    current_variant = tool.current_variant
                ));
            };

            if current.commands.iter().any(|cmd| cmd == command) {
                continue;
            }

            return Err(invariant!(
                "owners entry '{command}' not in current commands for '{owner_tool_id}'"
            ));
        }

        Ok(())
    }

    fn upsert_tool_entry(
        &mut self,
        tool_id: &str,
        tool_key: String,
        backend: String,
        name: String,
        variant_key: String,
        variant: VariantEntry,
    ) {
        let tool = self
            .tools
            .entry(tool_id.to_string())
            .or_insert_with(|| ToolEntry {
                tool_key: tool_key.clone(),
                backend: backend.clone(),
                name: name.clone(),
                current_variant: variant_key.clone(),
                variants: IndexMap::new(),
            });

        tool.tool_key = tool_key;
        tool.backend = backend;
        tool.name = name;
        tool.current_variant = variant_key.clone();
        tool.variants.insert(variant_key, variant);
    }

    fn clear_stale_owned_commands(&mut self, tool_id: &str, stale_commands: &[String]) {
        for command in stale_commands {
            if self
                .owners
                .get(command)
                .is_some_and(|owner| owner == tool_id)
            {
                self.owners.shift_remove(command);
            }
        }
    }

    fn assign_owned_commands(&mut self, tool_id: &str, commands: &[String]) {
        for command in commands {
            self.owners.insert(command.clone(), tool_id.to_string());
        }
    }
}

fn default_install_mode() -> String {
    InstallMode::Isolated.as_str().to_string()
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use tempfile::tempdir;

    use super::*;
    use crate::fs::PathBuf;

    fn sample_variant(commands: Vec<&str>) -> VariantEntry {
        let mut runtimes = IndexMap::new();
        runtimes.insert("node".to_string(), "22.13.1".to_string());
        VariantEntry {
            package_spec: "npm:http-server".to_string(),
            package_version: "14.1.1".to_string(),
            runtimes,
            install_dir: "/tmp/install".to_string(),
            commands: commands.into_iter().map(ToString::to_string).collect(),
            install_mode: InstallMode::Isolated.as_str().to_string(),
        }
    }

    #[test]
    fn round_trip_and_validate_manifest() {
        let tmp = tempdir().unwrap();
        let path = PathBuf::from_path_buf(tmp.path().join(".miseo-installs.toml")).unwrap();

        let mut manifest = Manifest::empty();
        manifest.upsert_install(UpsertInstall {
            tool_id: "npm:http-server".parse().unwrap(),
            variant_key: "14.1.1+node-22.13.1".to_string(),
            variant: sample_variant(vec!["http-server"]),
            stale_commands: vec![],
        });

        manifest.save_atomic(&path).unwrap();
        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(loaded.owners.get("http-server").unwrap(), "npm:http-server");
    }

    #[test]
    fn validates_owner_consistency() {
        let mut manifest = Manifest::empty();
        manifest
            .owners
            .insert("rg".to_string(), "cargo:ripgrep".to_string());
        let err = manifest.validate().unwrap_err();
        assert!(matches!(err, Error::ManifestInvariant(_)));
    }

    #[test]
    fn same_owner_relink_is_allowed_and_other_owner_rejected() {
        let mut manifest = Manifest::empty();
        manifest
            .owners
            .insert("http-server".to_string(), "npm:http-server".to_string());
        manifest
            .assert_no_conflicts(
                &"npm:http-server".parse().unwrap(),
                &["http-server".to_string()],
            )
            .unwrap();

        let err = manifest
            .assert_no_conflicts(
                &"cargo:ripgrep".parse().unwrap(),
                &["http-server".to_string()],
            )
            .unwrap_err();
        assert!(matches!(err, Error::CommandOwnershipConflict { .. }));
    }

    #[test]
    fn current_variant_must_exist() {
        let mut manifest = Manifest::empty();
        manifest.tools.insert(
            "npm:http-server".to_string(),
            ToolEntry {
                tool_key: "npm-http-server".to_string(),
                backend: "npm".to_string(),
                name: "http-server".to_string(),
                current_variant: "missing".to_string(),
                variants: IndexMap::new(),
            },
        );

        let err = manifest.validate().unwrap_err();
        assert!(matches!(err, Error::ManifestInvariant(_)));
    }
}
