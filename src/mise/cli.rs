//! Host `mise` integration backed by subprocess calls.

use std::{collections::BTreeMap, fs, io, process::Command};

use serde::Deserialize;

use crate::{
    error::{Error, invariant},
    fs::{Path, PathBuf},
    launch::{CommandTarget, node},
    spec::{Backend, Runtime, RuntimePins, RuntimeSpec, ToolId, ToolSpec},
};

use super::Mise;

#[derive(Debug, Default, Clone, Copy)]
pub struct Cli {
    interactive: bool,
    verbose: u8,
}

impl Cli {
    pub fn new(interactive: bool, verbose: u8) -> Self {
        Self {
            interactive,
            verbose,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new("mise");

        for _ in 0..self.verbose {
            command.arg("-v");
        }

        command
    }

    fn run_capture(&self, args: &[&str]) -> Result<String, Error> {
        let output = self.command().args(args).output()?;

        ensure_success(
            args.join(" "),
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )?;

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn run_capture_owned(&self, args: &[String]) -> Result<String, Error> {
        let output = self.command().args(args).output()?;

        ensure_success(
            args.join(" "),
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )?;

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn run_status(&self, args: &[String], stream: bool) -> Result<(), Error> {
        if stream {
            self.run_status_streaming(args)
        } else {
            self.run_status_captured(args)
        }
    }

    fn run_status_streaming(&self, args: &[String]) -> Result<(), Error> {
        let status = self.command().args(args).status()?;
        ensure_success(args.join(" "), status.success(), String::new())
    }

    fn run_status_captured(&self, args: &[String]) -> Result<(), Error> {
        let output = self.command().args(args).output()?;
        ensure_success(
            args.join(" "),
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )
    }
}

// Keep internal mise lookups out of the caller's project directory so an
// untrusted local mise.toml cannot block them with a trust error.
fn neutral_cd() -> &'static str {
    "/"
}

fn mise_command_failed(args: String, stderr: String) -> Error {
    Error::MiseCommandFailed {
        command: format!("mise {args}"),
        stderr,
    }
}

fn ensure_success(args: String, ok: bool, stderr: String) -> Result<(), Error> {
    if ok {
        return Ok(());
    }

    Err(mise_command_failed(args, stderr))
}

fn parse_ls_entries(stdout: &str) -> Result<Vec<LsEntry>, Error> {
    Ok(serde_json::from_str(stdout)?)
}

fn install_into_args(
    runtime_versions: &RuntimePins,
    tool_spec: &ToolSpec,
    target_dir: &Path,
) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "--cd".to_string(),
        neutral_cd().to_string(),
    ];

    for pin in runtime_versions.values() {
        args.push(pin.runtime_pin());
    }

    args.push("--".to_string());
    args.push("mise".to_string());
    args.push("install-into".to_string());
    args.push(tool_spec.to_string());
    args.push(target_dir.to_string());

    args
}

fn npm_install_global_args(tool_spec: &ToolSpec, project_dir: &Path) -> Result<Vec<String>, Error> {
    let package = npm_package_arg(tool_spec)?;
    let mut args = vec![
        "exec".to_string(),
        "--cd".to_string(),
        project_dir.to_string(),
    ];

    args.push("--".to_string());
    args.push("npm".to_string());
    args.push("install".to_string());
    args.push("-g".to_string());
    args.push(package);

    Ok(args)
}

fn npm_root_global_args(project_dir: &Path) -> Vec<String> {
    vec![
        "exec".to_string(),
        "--cd".to_string(),
        project_dir.to_string(),
        "--".to_string(),
        "npm".to_string(),
        "root".to_string(),
        "-g".to_string(),
    ]
}

fn npm_prefix_global_args(project_dir: &Path) -> Vec<String> {
    vec![
        "exec".to_string(),
        "--cd".to_string(),
        project_dir.to_string(),
        "--".to_string(),
        "npm".to_string(),
        "prefix".to_string(),
        "-g".to_string(),
    ]
}

fn npm_uninstall_global_args(tool_id: &ToolId, project_dir: &Path) -> Result<Vec<String>, Error> {
    let package = npm_package_name(tool_id)?;
    Ok(vec![
        "exec".to_string(),
        "--cd".to_string(),
        project_dir.to_string(),
        "--".to_string(),
        "npm".to_string(),
        "uninstall".to_string(),
        "-g".to_string(),
        package,
    ])
}

fn latest_args(runtime_versions: &RuntimePins, tool_spec: &ToolSpec) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "--cd".to_string(),
        neutral_cd().to_string(),
    ];

    for pin in runtime_versions.values() {
        args.push(pin.runtime_pin());
    }

    args.push("--".to_string());
    args.push("mise".to_string());
    args.push("latest".to_string());
    args.push(tool_spec.to_string());
    args.push("--cd".to_string());
    args.push(neutral_cd().to_string());

    args
}

fn npm_package_arg(tool_spec: &ToolSpec) -> Result<String, Error> {
    let Some(version) = tool_spec.version() else {
        return Err(invariant!(
            "npm global install requires exact tool spec, got '{tool_spec}'"
        ));
    };

    Ok(format!(
        "{}@{version}",
        npm_package_name(tool_spec.tool_id())?
    ))
}

fn npm_package_name(tool_id: &ToolId) -> Result<String, Error> {
    if tool_id.backend() != &Backend::Npm {
        return Err(invariant!(
            "global npm package operation requires npm backend, got '{tool_id}'"
        ));
    }

    Ok(tool_id.name().to_string())
}

fn require_non_empty(value: String, context: &str) -> Result<String, Error> {
    if value.is_empty() {
        return Err(invariant!("{context} returned empty output"));
    }

    Ok(value)
}

fn npm_package_dir(root: String, tool_id: &ToolId) -> Result<PathBuf, Error> {
    let root = require_non_empty(root, "npm root -g")?;
    Ok(PathBuf::from(root).join(tool_id.name()))
}

fn npm_package_json_path(package_dir: &Path) -> PathBuf {
    package_dir.join("package.json")
}

fn npm_package_bins(package_json: &str) -> Result<BTreeMap<String, String>, Error> {
    let package: NpmPackageJson = serde_json::from_str(package_json)?;
    Ok(bins_from_package_json(package))
}

fn npm_package_version(package_json: &str) -> Result<Option<String>, Error> {
    let package: NpmPackageJson = serde_json::from_str(package_json)?;
    Ok(package.version.filter(|version| !version.is_empty()))
}

fn npm_global_bin_dir(prefix: String) -> Result<PathBuf, Error> {
    let prefix = PathBuf::from(require_non_empty(prefix, "npm prefix -g")?);

    #[cfg(windows)]
    {
        Ok(prefix)
    }

    #[cfg(not(windows))]
    {
        Ok(prefix.join("bin"))
    }
}

fn npm_command_targets(
    package_dir: &Path,
    bin_dir: &Path,
    bins: BTreeMap<String, String>,
) -> BTreeMap<String, CommandTarget> {
    bins.into_iter()
        .filter(|(command, relative)| !command.is_empty() && !relative.is_empty())
        .map(|(command, relative)| {
            let fallback = bin_dir.join(&command);
            let target = node::package_bin_target(package_dir, &relative, fallback);
            (command, target)
        })
        .collect()
}

fn bins_from_package_json(package: NpmPackageJson) -> BTreeMap<String, String> {
    match package.bin {
        Some(NpmBin::Command(path)) if !path.is_empty() => package
            .name
            .as_deref()
            .map(default_npm_bin_name)
            .filter(|name| !name.is_empty())
            .map(|name| BTreeMap::from([(name.to_string(), path)]))
            .unwrap_or_default(),
        Some(NpmBin::Commands(commands)) => commands
            .into_iter()
            .filter(|(command, path)| !command.is_empty() && !path.is_empty())
            .collect(),
        _ => BTreeMap::new(),
    }
}

fn default_npm_bin_name(package_name: &str) -> &str {
    package_name
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(package_name)
}

fn latest_tool_spec(requested: &ToolSpec, version: String) -> Result<ToolSpec, Error> {
    let version = require_non_empty(version, &format!("mise latest for '{requested}'"))?;
    Ok(requested.with_version(&version))
}

fn runtime_spec_from_output(
    runtime: &Runtime,
    selector: String,
    context: &str,
) -> Result<RuntimeSpec, Error> {
    let selector = require_non_empty(selector, context)?;
    Ok(RuntimeSpec::new(runtime.clone(), selector))
}

fn runtime_spec_from_install_path(
    runtime: &Runtime,
    path: String,
    context: &str,
) -> Result<RuntimeSpec, Error> {
    let path = require_non_empty(path, context)?;
    let path = PathBuf::from(path);
    let version = path
        .file_name()
        .ok_or_else(|| invariant!("{context} returned path without a final component"))?;

    Ok(RuntimeSpec::new(runtime.clone(), version.to_string()))
}

// mise parses `@path:` tool specs as slash-separated tool paths. This
// normalization is for Windows install directories; generated Unix install
// paths should not contain backslashes.
fn path_tool_spec(tool_id: &ToolId, install_dir: &Path) -> String {
    format!("{tool_id}@path:{}", install_dir.as_str().replace('\\', "/"))
}

impl Mise for Cli {
    fn resolve_latest_version(
        &self,
        runtime_versions: &RuntimePins,
        spec: &ToolSpec,
    ) -> Result<ToolSpec, Error> {
        let args = latest_args(runtime_versions, spec);
        let version = self.run_capture_owned(&args)?;
        latest_tool_spec(spec, version)
    }

    fn resolve_global_runtime_selector(
        &self,
        runtime: &Runtime,
    ) -> Result<Option<RuntimeSpec>, Error> {
        let runtime_name = runtime.as_ref();
        let stdout = self.run_capture(&[
            "ls",
            "--global",
            "--current",
            "--json",
            runtime_name,
            "--cd",
            neutral_cd(),
        ])?;

        let parsed = parse_ls_entries(&stdout)?;
        parsed
            .into_iter()
            .find_map(|entry| entry.requested_version)
            .map(|selector| runtime_spec_from_output(runtime, selector, "mise ls --global"))
            .transpose()
    }

    fn resolve_global_runtime_installed(&self, runtime: &Runtime) -> Result<bool, Error> {
        let runtime = runtime.as_ref();
        let stdout = self.run_capture(&[
            "ls",
            "--global",
            "--current",
            "--installed",
            "--json",
            runtime,
            "--cd",
            neutral_cd(),
        ])?;

        let parsed = parse_ls_entries(&stdout)?;
        Ok(!parsed.is_empty())
    }

    fn resolve_current_runtime_version(&self, spec: &RuntimeSpec) -> Result<RuntimeSpec, Error> {
        let runtime = spec.runtime();
        let install_path =
            self.run_capture(&["where", &spec.runtime_pin(), "--cd", neutral_cd()])?;
        runtime_spec_from_install_path(runtime, install_path, "mise where")
    }

    fn install_global(&self, tool_spec: &ToolSpec, project_dir: &Path) -> Result<(), Error> {
        let args = npm_install_global_args(tool_spec, project_dir)?;
        self.run_status(&args, self.interactive)
    }

    fn install_into(
        &self,
        runtime_versions: &RuntimePins,
        tool_spec: &ToolSpec,
        target_dir: &Path,
    ) -> Result<(), Error> {
        let args = install_into_args(runtime_versions, tool_spec, target_dir);
        self.run_status(&args, self.interactive)
    }

    fn installed_command_targets(
        &self,
        tool_spec: &ToolSpec,
        project_dir: &Path,
    ) -> Result<BTreeMap<String, CommandTarget>, Error> {
        if tool_spec.backend() != &Backend::Npm {
            return Err(invariant!(
                "global command discovery currently supports npm packages, got '{tool_spec}'"
            ));
        }

        let args = npm_root_global_args(project_dir);
        let root = self.run_capture_owned(&args)?;
        let package_dir = npm_package_dir(root, tool_spec.tool_id())?;
        let package_json_path = npm_package_json_path(&package_dir);
        let package_json = fs::read_to_string(package_json_path.as_std_path())?;
        let bins = npm_package_bins(&package_json)?;

        let args = npm_prefix_global_args(project_dir);
        let prefix = self.run_capture_owned(&args)?;
        let bin_dir = npm_global_bin_dir(prefix)?;

        Ok(npm_command_targets(&package_dir, &bin_dir, bins))
    }

    fn installed_global_package_version(
        &self,
        tool_id: &ToolId,
        project_dir: &Path,
    ) -> Result<Option<String>, Error> {
        if tool_id.backend() != &Backend::Npm {
            return Ok(None);
        }

        let args = npm_root_global_args(project_dir);
        let root = self.run_capture_owned(&args)?;
        let package_dir = npm_package_dir(root, tool_id)?;
        let package_json_path = npm_package_json_path(&package_dir);

        match fs::read_to_string(package_json_path.as_std_path()) {
            Ok(package_json) => npm_package_version(&package_json),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn uninstall_global(&self, tool_id: &ToolId, project_dir: &Path) -> Result<(), Error> {
        if tool_id.backend() != &Backend::Npm {
            return Ok(());
        }

        let args = npm_uninstall_global_args(tool_id, project_dir)?;
        self.run_status(&args, self.interactive)
    }

    fn bin_paths(&self, tool_id: &ToolId, install_dir: &Path) -> Result<Vec<PathBuf>, Error> {
        let path_spec = path_tool_spec(tool_id, install_dir);
        let stdout = self.run_capture(&["--no-config", "bin-paths", &path_spec])?;
        Ok(stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect())
    }

    fn trust_config(&self, config_path: &Path) -> Result<(), Error> {
        let args = vec!["trust".to_string(), config_path.to_string()];
        self.run_status(&args, false)
    }
}

#[derive(Debug, Deserialize)]
struct LsEntry {
    requested_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NpmPackageJson {
    name: Option<String>,
    version: Option<String>,
    bin: Option<NpmBin>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum NpmBin {
    Command(String),
    Commands(BTreeMap<String, String>),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use crate::launch::CommandTarget;
    use crate::spec::{Runtime, RuntimePins, RuntimeSpec, ToolId, ToolSpec};

    use super::*;

    #[test]
    fn ensure_success_returns_mise_command_failed_error() {
        let err = ensure_success(
            "exec -- npm install -g prettier@3.8.1".to_string(),
            false,
            "boom".to_string(),
        )
        .unwrap_err();
        match err {
            Error::MiseCommandFailed { command, stderr } => {
                assert_eq!(command, "mise exec -- npm install -g prettier@3.8.1");
                assert_eq!(stderr, "boom");
            }
            _ => panic!("expected mise command failure"),
        }
    }

    #[test]
    fn ensure_success_accepts_ok_status() {
        ensure_success("latest npm:prettier".to_string(), true, String::new()).unwrap();
    }

    #[test]
    fn parse_ls_entries_parses_requested_versions() {
        let parsed =
            parse_ls_entries(r#"[{"requested_version":"22"},{"requested_version":null}]"#).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].requested_version.as_deref(), Some("22"));
        assert_eq!(parsed[1].requested_version.as_deref(), None);
    }

    #[test]
    fn parse_ls_entries_invalid_json_returns_json_error() {
        let err = parse_ls_entries("{").unwrap_err();
        assert!(matches!(err, Error::Json(_)));
    }

    #[test]
    fn npm_install_global_args_builds_expected_exec_command() {
        let spec: ToolSpec = "npm:prettier@3.8.1".parse().unwrap();
        let project_dir = PathBuf::from("/tmp/miseo/npm-prettier/3.8.1+node-24.13.1");

        let args = npm_install_global_args(&spec, &project_dir).unwrap();
        assert_eq!(
            args,
            vec![
                "exec",
                "--cd",
                "/tmp/miseo/npm-prettier/3.8.1+node-24.13.1",
                "--",
                "npm",
                "install",
                "-g",
                "prettier@3.8.1",
            ]
        );
    }

    #[test]
    fn install_into_args_builds_expected_exec_command() {
        let mut runtime_versions = RuntimePins::new();
        runtime_versions.insert(
            Runtime::Node,
            RuntimeSpec::new(Runtime::Node, "24.13.1".to_string()),
        );
        runtime_versions.insert(
            Runtime::Python,
            RuntimeSpec::new(Runtime::Python, "3.12.8".to_string()),
        );

        let spec: ToolSpec = "npm:prettier@3.8.1".parse().unwrap();
        let target_dir = PathBuf::from("/tmp/miseo/npm-prettier/3.8.1+node-24.13.1");

        let args = install_into_args(&runtime_versions, &spec, &target_dir);
        assert_eq!(
            args,
            vec![
                "exec",
                "--cd",
                neutral_cd(),
                "node@24.13.1",
                "python@3.12.8",
                "--",
                "mise",
                "install-into",
                "npm:prettier@3.8.1",
                "/tmp/miseo/npm-prettier/3.8.1+node-24.13.1",
            ]
        );
    }

    #[test]
    fn npm_package_arg_strips_backend_and_preserves_scope() {
        let spec: ToolSpec = "npm:@openai/codex@1.2.3".parse().unwrap();

        assert_eq!(npm_package_arg(&spec).unwrap(), "@openai/codex@1.2.3");
    }

    #[test]
    fn npm_uninstall_global_args_builds_expected_exec_command() {
        let tool_id: ToolId = "npm:@openai/codex".parse().unwrap();
        let project_dir = PathBuf::from("/tmp/miseo/npm-openai-codex/1.2.3+node-24.13.1");

        let args = npm_uninstall_global_args(&tool_id, &project_dir).unwrap();

        assert_eq!(
            args,
            vec![
                "exec",
                "--cd",
                "/tmp/miseo/npm-openai-codex/1.2.3+node-24.13.1",
                "--",
                "npm",
                "uninstall",
                "-g",
                "@openai/codex",
            ]
        );
    }

    #[test]
    fn latest_args_builds_expected_exec_command() {
        let mut runtime_versions = RuntimePins::new();
        runtime_versions.insert(
            Runtime::Node,
            RuntimeSpec::new(Runtime::Node, "24.13.1".to_string()),
        );

        let spec: ToolSpec = "npm:prettier@latest".parse().unwrap();

        let args = latest_args(&runtime_versions, &spec);
        assert_eq!(
            args,
            vec![
                "exec",
                "--cd",
                neutral_cd(),
                "node@24.13.1",
                "--",
                "mise",
                "latest",
                "npm:prettier@latest",
                "--cd",
                neutral_cd(),
            ]
        );
    }

    #[test]
    fn npm_package_bins_reads_object_bins() {
        let bins =
            npm_package_bins(r#"{"name":"pkg","bin":{"foo":"bin/foo.js","bar":"bin/bar.js"}}"#)
                .unwrap();

        assert_eq!(
            bins,
            BTreeMap::from([
                ("bar".to_string(), "bin/bar.js".to_string()),
                ("foo".to_string(), "bin/foo.js".to_string())
            ])
        );
    }

    #[test]
    fn npm_package_bins_uses_unscoped_name_for_string_bin() {
        let bins = npm_package_bins(r#"{"name":"@openai/codex","bin":"bin/codex.js"}"#).unwrap();

        assert_eq!(
            bins,
            BTreeMap::from([("codex".to_string(), "bin/codex.js".to_string())])
        );
    }

    #[test]
    fn npm_command_targets_run_node_entrypoints_directly() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let package_dir = root.join("node_modules/@openai/codex");
        let bin_dir = PathBuf::from("/tmp/mise/installs/node/24.13.1/bin");
        let entrypoint = package_dir.join("bin/codex.js");
        std::fs::create_dir_all(entrypoint.parent().unwrap().as_std_path()).unwrap();
        std::fs::write(entrypoint.as_std_path(), "#!/usr/bin/env node\n").unwrap();

        let targets = npm_command_targets(
            &package_dir,
            &bin_dir,
            BTreeMap::from([("codex".to_string(), "bin/codex.js".to_string())]),
        );

        assert_eq!(
            targets.get("codex"),
            Some(&CommandTarget::runtime_entrypoint(
                Runtime::Node,
                package_dir.join("bin/codex.js")
            ))
        );
    }

    #[test]
    fn npm_command_targets_fall_back_to_global_bin_for_non_node_bins() {
        let package_dir = PathBuf::from("/tmp/npm/node_modules/native-tool");
        let bin_dir = PathBuf::from("/tmp/mise/installs/node/24.13.1/bin");
        let targets = npm_command_targets(
            &package_dir,
            &bin_dir,
            BTreeMap::from([("native-tool".to_string(), "bin/native-tool".to_string())]),
        );

        assert_eq!(
            targets.get("native-tool"),
            Some(&CommandTarget::env_wrapped(PathBuf::from(
                "/tmp/mise/installs/node/24.13.1/bin/native-tool"
            )))
        );
    }

    #[test]
    fn path_tool_spec_uses_slashes_for_windows_paths() {
        let tool_id: ToolId = "npm:prettier".parse().unwrap();
        let install_dir =
            PathBuf::from(r"C:\Users\runneradmin\.miseo\npm-prettier\3.8.1+node-24.15.0");

        assert_eq!(
            path_tool_spec(&tool_id, &install_dir),
            "npm:prettier@path:C:/Users/runneradmin/.miseo/npm-prettier/3.8.1+node-24.15.0"
        );
    }

    #[test]
    fn runtime_spec_from_install_path_uses_final_path_component() {
        let spec = runtime_spec_from_install_path(
            &Runtime::Node,
            "/home/user/.local/share/mise/installs/node/24.15.0".to_string(),
            "mise where",
        )
        .unwrap();

        assert_eq!(spec, RuntimeSpec::new(Runtime::Node, "24.15.0"));
    }
}
