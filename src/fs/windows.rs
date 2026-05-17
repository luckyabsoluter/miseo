use std::{
    fs::{self, OpenOptions},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    error::{Error, invariant},
    launch::CommandTarget,
};

use super::{Fs, Path, PathBuf};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_EXECUTE: u32 = 0x20;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const FILE_SHARE_DELETE: u32 = 0x4;

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct WindowsFs;

impl Fs for WindowsFs {
    fn is_executable(&self, path: &Path) -> Result<bool, Error> {
        let meta = fs::metadata(path.as_std_path())?;

        if !meta.is_file() {
            return Ok(false);
        }

        has_execute_permission(path)
    }

    fn ls(&self, dir: &Path) -> Result<Vec<PathBuf>, Error> {
        fs::read_dir(dir.as_std_path())?
            .map(|entry| -> Result<PathBuf, Error> {
                let entry = entry?;
                PathBuf::from_path_buf(entry.path())
                    .map_err(|_| invariant!("non-utf8 path in directory"))
            })
            .collect()
    }

    fn mkdir_p(&self, path: &Path) -> Result<(), Error> {
        Ok(fs::create_dir_all(path.as_std_path())?)
    }

    fn readlink(&self, path: &Path) -> Result<Option<PathBuf>, Error> {
        if let Some(target) = read_public_powershell_proxy_target(path)? {
            return Ok(Some(target));
        }

        match fs::read_link(path.as_std_path()) {
            Ok(target) => {
                let target = PathBuf::from_path_buf(target)
                    .map_err(|_| invariant!("non-utf8 link target path"))?;
                Ok(Some(target))
            }
            Err(err) if is_invalid_input_or_not_found(&err) => Ok(None),
            Err(err) => Err(Error::Io(err)),
        }
    }

    fn ln_s(&self, target: &Path, link_path: &Path) -> Result<(), Error> {
        let parent = link_path
            .parent()
            .ok_or_else(|| invariant!("path '{link_path}' has no parent"))?;
        self.mkdir_p(parent)?;

        if is_relative_current_target(target) {
            write_current_junction(parent, target, link_path)
        } else {
            write_command_proxy(target, link_path)
        }
    }

    fn rm(&self, path: &Path) -> Result<(), Error> {
        remove_link_or_file_if_exists(path)
    }

    fn rm_rf(&self, path: &Path) -> Result<(), Error> {
        remove_dir_all_if_exists(path)
    }

    fn write_file(&self, path: &Path, content: &str) -> Result<(), Error> {
        Ok(fs::write(path.as_std_path(), content)?)
    }

    #[cfg(test)]
    fn exists(&self, path: &Path) -> Result<bool, Error> {
        match fs::symlink_metadata(path.as_std_path()) {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(Error::Io(err)),
        }
    }

    #[cfg(test)]
    fn read_file(&self, path: &Path) -> Result<String, Error> {
        Ok(fs::read_to_string(path.as_std_path())?)
    }

    #[cfg(test)]
    fn write_executable_file(&self, path: &Path, content: &str) -> Result<(), Error> {
        let parent = path
            .parent()
            .ok_or_else(|| invariant!("path '{path}' has no parent"))?;
        self.mkdir_p(parent)?;

        let tmp = temp_path(parent, ".miseo-file")?;
        self.write_file(&tmp, content)?;
        replace_file(&tmp, path)
    }

    fn write_command_shim(
        &self,
        project_dir: &Path,
        target: &CommandTarget,
        path: &Path,
    ) -> Result<(), Error> {
        let parent = path
            .parent()
            .ok_or_else(|| invariant!("path '{path}' has no parent"))?;
        self.mkdir_p(parent)?;
        remove_link_or_file_if_exists(path)?;

        let ps_path = powershell_shim_path(path);
        let ps_tmp = temp_path(parent, ".miseo-ps1")?;
        fs::write(
            ps_tmp.as_std_path(),
            powershell_shim_content(project_dir, target),
        )?;
        replace_file(&ps_tmp, &ps_path)
    }
}

fn has_execute_permission(path: &Path) -> Result<bool, Error> {
    match OpenOptions::new()
        .access_mode(FILE_EXECUTE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path.as_std_path())
    {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => Ok(false),
        Err(err) => Err(Error::Io(err)),
    }
}

fn is_relative_current_target(target: &Path) -> bool {
    target.is_relative() && target.components().count() == 1
}

fn write_current_junction(parent: &Path, target: &Path, link_path: &Path) -> Result<(), Error> {
    let target_path = parent.join(target);
    let tmp = temp_path(parent, ".miseo-current")?;

    create_junction(&target_path, &tmp)?;

    if let Err(err) = remove_link_or_file_if_exists(link_path) {
        let _ = fs::remove_dir(tmp.as_std_path());
        return Err(err);
    }

    match fs::rename(tmp.as_std_path(), link_path.as_std_path()) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_dir(tmp.as_std_path());
            Err(Error::Io(err))
        }
    }
}

fn create_junction(target: &Path, link_path: &Path) -> Result<(), Error> {
    let target = cmd_path(target);
    let link_path = cmd_path(link_path);

    let output = Command::new("cmd")
        .args(["/C", "mklink", "/J", &link_path, &target])
        .output()?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Err(invariant!(
        "failed to create junction '{link_path}' -> '{target}': {}",
        if stderr.is_empty() { stdout } else { stderr }
    ))
}

fn write_command_proxy(target: &Path, link_path: &Path) -> Result<(), Error> {
    let parent = link_path
        .parent()
        .ok_or_else(|| invariant!("path '{link_path}' has no parent"))?;
    remove_link_or_file_if_exists(link_path)?;

    let script = powershell_shim_path(link_path);
    let script_tmp = temp_path(parent, ".miseo-link-ps1")?;
    fs::write(
        script_tmp.as_std_path(),
        public_powershell_proxy_content(target),
    )?;
    replace_file(&script_tmp, &script)?;

    let command = command_file_path(link_path);
    let command_tmp = temp_path(parent, ".miseo-link-cmd")?;
    fs::write(command_tmp.as_std_path(), cmd_shim_content(&script))?;
    replace_file(&command_tmp, &command)?;

    let shell_tmp = temp_path(parent, ".miseo-link-sh")?;
    fs::write(shell_tmp.as_std_path(), shell_shim_content(&script))?;
    replace_file(&shell_tmp, link_path)
}

fn read_public_powershell_proxy_target(path: &Path) -> Result<Option<PathBuf>, Error> {
    if path.extension().is_some() {
        return Ok(None);
    }

    let script = powershell_shim_path(path);
    let content = match fs::read_to_string(script.as_std_path()) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Error::Io(err)),
    };

    let Some(target) = parse_public_powershell_proxy_target(&content) else {
        return Ok(None);
    };

    Ok(Some(logical_public_proxy_target(PathBuf::from(target))))
}

fn parse_public_powershell_proxy_target(content: &str) -> Option<String> {
    let line = content.lines().find(|line| line.starts_with("$path = '"))?;
    let target = line
        .strip_prefix("$path = '")?
        .strip_suffix('\'')?
        .replace("''", "'");

    Some(target)
}

fn logical_public_proxy_target(target: PathBuf) -> PathBuf {
    if !target
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ps1"))
    {
        return target;
    }

    let Some(parent) = target.parent() else {
        return target;
    };

    if !parent
        .file_name()
        .is_some_and(|file_name| file_name.eq_ignore_ascii_case(".miseo"))
    {
        return target;
    }

    let Some(stem) = target.file_stem() else {
        return target;
    };

    parent.join(stem)
}

fn public_powershell_proxy_content(target: &Path) -> String {
    let target = powershell_shim_path(target);
    format!(
        "# {}\r\n$path = {}\r\n$exe = if (Get-Command pwsh.exe -ErrorAction SilentlyContinue) {{ 'pwsh.exe' }} else {{ 'powershell.exe' }}\r\nif ($MyInvocation.ExpectingInput) {{ $input | & $exe -noprofile -ex unrestricted -file $path @args }} else {{ & $exe -noprofile -ex unrestricted -file $path @args }}\r\nexit $LASTEXITCODE\r\n",
        cmd_path(&target),
        ps_quote(&target)
    )
}

fn cmd_shim_content(script: &Path) -> String {
    format!(
        "@rem {}\r\n@echo off\r\nwhere /q pwsh.exe\r\nif %errorlevel% equ 0 (\r\n    pwsh -noprofile -ex unrestricted -file {} %*\r\n) else (\r\n    powershell -noprofile -ex unrestricted -file {} %*\r\n)\r\nexit /b %ERRORLEVEL%\r\n",
        cmd_path(script),
        cmd_quote(script),
        cmd_quote(script)
    )
}

fn shell_shim_content(script: &Path) -> String {
    format!(
        "#!/bin/sh\n# {}\nif command -v pwsh.exe > /dev/null 2>&1; then\n    pwsh.exe -noprofile -ex unrestricted -file {} \"$@\"\nelse\n    powershell.exe -noprofile -ex unrestricted -file {} \"$@\"\nfi\n",
        cmd_path(script),
        sh_quote(script),
        sh_quote(script)
    )
}

fn powershell_shim_content(project_dir: &Path, target: &CommandTarget) -> String {
    match target {
        CommandTarget::EnvWrapped(target) => {
            powershell_mise_env_shim_content(project_dir, &powershell_target_path(target))
        }
        CommandTarget::RuntimeEntrypoint {
            runtime,
            entrypoint,
        } => powershell_runtime_entrypoint_shim_content(project_dir, runtime.as_ref(), entrypoint),
    }
}

fn powershell_runtime_entrypoint_shim_content(
    project_dir: &Path,
    runtime: &str,
    entrypoint: &Path,
) -> String {
    format!(
        "$runtime = (& mise which {} -C {}).Trim()\r\nif ($LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}\r\nif ($MyInvocation.ExpectingInput) {{ $input | & $runtime {} @args }} else {{ & $runtime {} @args }}\r\nexit $LASTEXITCODE\r\n",
        ps_quote_str(runtime),
        ps_quote(project_dir),
        ps_quote(entrypoint),
        ps_quote(entrypoint)
    )
}

fn powershell_mise_env_shim_content(project_dir: &Path, target: &Path) -> String {
    format!(
        "Invoke-Expression (& mise env -C {} -s pwsh)\r\n& {} @args\r\nexit $LASTEXITCODE\r\n",
        ps_quote(project_dir),
        ps_quote(target)
    )
}

fn powershell_target_path(target: &Path) -> PathBuf {
    let Some(ext) = target.extension() else {
        let ps1 = target.with_extension("ps1");
        if ps1.exists() {
            return ps1;
        }

        return target.with_extension("cmd");
    };

    if ext.eq_ignore_ascii_case("cmd") {
        let ps1 = target.with_extension("ps1");
        if ps1.exists() {
            return ps1;
        }
    }

    target.to_path_buf()
}

fn powershell_shim_path(path: &Path) -> PathBuf {
    path.with_extension("ps1")
}

fn cmd_quote(path: &Path) -> String {
    format!("\"{}\"", cmd_path(path).replace('"', "\"\""))
}

fn cmd_path(path: &Path) -> String {
    path.as_str().replace('/', "\\")
}

fn ps_quote(path: &Path) -> String {
    format!("'{}'", cmd_path(path).replace('\'', "''"))
}

fn ps_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn sh_quote(path: &Path) -> String {
    format!("'{}'", cmd_path(path).replace('\'', "'\\''"))
}

fn command_file_path(path: &Path) -> PathBuf {
    if path
        .extension()
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "bat" | "cmd"))
    {
        return path.to_path_buf();
    }

    path.with_extension("cmd")
}

fn replace_file(tmp: &Path, path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path.as_std_path()) {
        Ok(meta) if is_directory(&meta) => {
            let _ = fs::remove_file(tmp.as_std_path());
            Err(invariant!("cannot replace directory '{path}' with file"))
        }
        Ok(_) => {
            fs::remove_file(path.as_std_path())?;
            Ok(fs::rename(tmp.as_std_path(), path.as_std_path())?)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(fs::rename(tmp.as_std_path(), path.as_std_path())?)
        }
        Err(err) => {
            let _ = fs::remove_file(tmp.as_std_path());
            Err(Error::Io(err))
        }
    }
}

fn remove_link_or_file_if_exists(path: &Path) -> Result<(), Error> {
    let command_path = command_file_path(path);
    if command_path != path {
        remove_file_if_exists(&command_path)?;
    }

    let powershell_path = powershell_shim_path(path);
    if powershell_path != path {
        remove_file_if_exists(&powershell_path)?;
    }

    match fs::symlink_metadata(path.as_std_path()) {
        Ok(meta) if is_directory(&meta) && !is_reparse_point(&meta) => Err(invariant!(
            "cannot replace directory '{path}' with managed link"
        )),
        Ok(meta) if is_directory(&meta) => Ok(fs::remove_dir(path.as_std_path())?),
        Ok(_) => Ok(fs::remove_file(path.as_std_path())?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::Io(err)),
    }
}

fn remove_file_if_exists(path: &Path) -> Result<(), Error> {
    match fs::remove_file(path.as_std_path()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::Io(err)),
    }
}

fn remove_dir_all_if_exists(path: &Path) -> Result<(), Error> {
    remove_reparse_children(path)?;

    match fs::remove_dir_all(path.as_std_path()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::Io(err)),
    }
}

fn remove_reparse_children(path: &Path) -> Result<(), Error> {
    let entries = match fs::read_dir(path.as_std_path()) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(Error::Io(err)),
    };

    for entry in entries {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;

        if is_reparse_point(&meta) && is_directory(&meta) {
            fs::remove_dir(entry.path())?;
        }
    }

    Ok(())
}

fn is_directory(meta: &fs::Metadata) -> bool {
    meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
}

fn is_reparse_point(meta: &fs::Metadata) -> bool {
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn is_invalid_input_or_not_found(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::InvalidInput
        || err.kind() == std::io::ErrorKind::NotFound
        || err.raw_os_error() == Some(4390)
}

fn temp_path(parent: &Path, prefix: &str) -> Result<PathBuf, Error> {
    let stamp = unique_stamp()?;
    Ok(parent.join(format!("{prefix}-{stamp}")))
}

fn unique_stamp() -> Result<u128, Error> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| invariant!("clock error: {e}"))?
        .as_nanos())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::launch::CommandTarget;

    use super::{Fs, Path, PathBuf, WindowsFs};

    #[test]
    fn command_proxy_reports_managed_target() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let target = root.join("tool/current/.miseo/foo");
        let link = root.join(".bin/foo");

        WindowsFs.mkdir_p(target.parent().unwrap()).unwrap();
        WindowsFs
            .write_file(&target.with_extension("ps1"), "")
            .unwrap();
        WindowsFs.ln_s(&target, &link).unwrap();

        assert!(WindowsFs.exists(&link).unwrap());
        assert!(WindowsFs.exists(&link.with_extension("cmd")).unwrap());
        assert!(WindowsFs.exists(&link.with_extension("ps1")).unwrap());
        assert_eq!(WindowsFs.readlink(&link).unwrap(), Some(target));
        assert_eq!(
            WindowsFs.readlink(&link.with_extension("cmd")).unwrap(),
            None
        );
        assert_eq!(
            WindowsFs.readlink(&link.with_extension("ps1")).unwrap(),
            None
        );

        let shell = WindowsFs.read_file(&link).unwrap();
        let cmd = WindowsFs.read_file(&link.with_extension("cmd")).unwrap();
        let ps1 = WindowsFs.read_file(&link.with_extension("ps1")).unwrap();

        assert!(shell.starts_with("#!/bin/sh"));
        assert!(shell.contains("pwsh.exe -noprofile -ex unrestricted -file"));
        assert!(cmd.contains("pwsh -noprofile -ex unrestricted -file"));
        assert!(ps1.contains("$path ="));
        assert!(ps1.contains("Get-Command pwsh.exe"));
        assert!(ps1.contains("& $exe -noprofile -ex unrestricted -file $path @args"));
        assert!(ps1.contains("\\tool\\current\\.miseo\\foo.ps1"));
    }

    #[test]
    fn is_executable_checks_file_execute_permission() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let executable = root.join("executable.cmd");
        let non_executable = root.join("non-executable.cmd");
        let directory = root.join("directory");

        WindowsFs.write_file(&executable, "@echo off\r\n").unwrap();
        WindowsFs
            .write_file(&non_executable, "@echo off\r\n")
            .unwrap();
        WindowsFs.mkdir_p(&directory).unwrap();
        deny_execute(&non_executable);

        assert!(WindowsFs.is_executable(&executable).unwrap());
        assert!(!WindowsFs.is_executable(&non_executable).unwrap());
        assert!(!WindowsFs.is_executable(&directory).unwrap());

        allow_cleanup(&non_executable);
    }

    fn deny_execute(path: &Path) {
        let user = current_user();
        let status = std::process::Command::new("icacls")
            .arg(path.as_std_path())
            .arg("/inheritance:r")
            .arg("/grant:r")
            .arg(format!("{user}:R"))
            .arg("/deny")
            .arg(format!("{user}:(X)"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();

        assert!(status.success());
    }

    fn allow_cleanup(path: &Path) {
        let user = current_user();
        let status = std::process::Command::new("icacls")
            .arg(path.as_std_path())
            .arg("/remove:d")
            .arg(&user)
            .arg("/grant:r")
            .arg(format!("{user}:F"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();

        assert!(status.success());
    }

    fn current_user() -> String {
        let output = std::process::Command::new("whoami").output().unwrap();

        assert!(output.status.success());

        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn cmd_path_uses_backslashes_for_cmd_builtins() {
        let path = PathBuf::from("C:/Users/user/.miseo/npm-tool/current");

        assert_eq!(
            super::cmd_path(&path),
            "C:\\Users\\user\\.miseo\\npm-tool\\current"
        );
    }

    #[test]
    fn cmd_shim_content_uses_scoop_style_powershell_launcher() {
        let script = PathBuf::from("C:/Users/user/.miseo/npm-tool/current/.miseo/foo.ps1");

        let content = super::cmd_shim_content(&script);

        assert!(content.contains("where /q pwsh.exe"));
        assert!(content.contains("pwsh -noprofile -ex unrestricted -file"));
        assert!(content.contains("powershell -noprofile -ex unrestricted -file"));
        assert!(!content.contains("MISEO_CWD"));
    }

    #[test]
    fn powershell_shim_content_uses_mise_env_without_changing_directory() {
        let project_dir = PathBuf::from("C:/Users/user/.miseo/npm-tool/1.0.0+node-24.15.0");
        let target = CommandTarget::env_wrapped(PathBuf::from(
            "C:/Users/user/.local/share/mise/installs/node/24.15.0/bin/foo.cmd",
        ));

        let content = super::powershell_shim_content(&project_dir, &target);

        assert!(content.contains("Invoke-Expression (& mise env -C"));
        assert!(!content.contains("Set-Location"));
        assert!(!content.contains("MISEO_CWD"));
        assert!(content.contains("\\installs\\node\\24.15.0\\bin\\foo.cmd' @args"));
    }

    #[test]
    fn public_powershell_proxy_runs_target_script_in_subprocess() {
        let target = PathBuf::from("C:/Users/user/.miseo/npm-tool/current/.miseo/foo");

        let content = super::public_powershell_proxy_content(&target);

        assert!(
            content
                .contains("$path = 'C:\\Users\\user\\.miseo\\npm-tool\\current\\.miseo\\foo.ps1'")
        );
        assert!(content.contains("$exe = if (Get-Command pwsh.exe -ErrorAction SilentlyContinue)"));
        assert!(content.contains("$input | & $exe -noprofile -ex unrestricted -file $path @args"));
        assert!(content.contains("else { & $exe -noprofile -ex unrestricted -file $path @args }"));
        assert!(!content.contains("& $path @args"));
    }

    #[test]
    fn write_command_shim_creates_only_ps1_file() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let project_dir = root.join("tool/1.0.0+node-24.15.0");
        let target = CommandTarget::env_wrapped(root.join("global-bin/foo.cmd"));
        let shim = project_dir.join(".miseo/foo");

        WindowsFs.mkdir_p(&project_dir).unwrap();
        WindowsFs
            .write_command_shim(&project_dir, &target, &shim)
            .unwrap();

        assert!(!WindowsFs.exists(&shim).unwrap());
        assert!(!WindowsFs.exists(&shim.with_extension("cmd")).unwrap());
        assert!(WindowsFs.exists(&shim.with_extension("ps1")).unwrap());
    }

    #[test]
    fn write_command_shim_uses_mise_env_for_env_wrapped_cmd_targets() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let project_dir = root.join("tool/1.0.0+node-24.15.0");
        let target = CommandTarget::env_wrapped(root.join("global-bin/foo.cmd"));
        let shim = project_dir.join(".miseo/foo");

        WindowsFs.mkdir_p(&project_dir).unwrap();
        WindowsFs
            .write_command_shim(&project_dir, &target, &shim)
            .unwrap();

        let content = WindowsFs.read_file(&shim.with_extension("ps1")).unwrap();

        assert!(content.contains("\\global-bin\\foo.cmd' @args"));
    }

    #[test]
    fn write_command_shim_prefers_powershell_sibling_for_extensionless_targets() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let project_dir = root.join("tool/1.0.0+node-24.15.0");
        let target_path = root.join("global-bin/foo");
        let target = CommandTarget::env_wrapped(target_path.clone());
        let shim = project_dir.join(".miseo/foo");

        WindowsFs.mkdir_p(target_path.parent().unwrap()).unwrap();
        WindowsFs.write_file(&target_path, "#!/bin/sh\n").unwrap();
        WindowsFs
            .write_file(&target_path.with_extension("ps1"), "")
            .unwrap();
        WindowsFs
            .write_command_shim(&project_dir, &target, &shim)
            .unwrap();

        let content = WindowsFs.read_file(&shim.with_extension("ps1")).unwrap();

        assert!(content.contains("\\global-bin\\foo.ps1' @args"));
    }

    #[test]
    fn write_command_shim_runs_powershell_node_entrypoint_without_mise_env() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let project_dir = root.join("tool/1.0.0+node-24.15.0");
        let entrypoint = root.join("global-bin/entrypoints/tool.js");
        let target = CommandTarget::runtime_entrypoint(crate::spec::Runtime::Node, entrypoint);
        let shim = project_dir.join(".miseo/codex");

        WindowsFs
            .write_command_shim(&project_dir, &target, &shim)
            .unwrap();

        let content = WindowsFs.read_file(&shim.with_extension("ps1")).unwrap();

        assert!(content.contains("$runtime = (& mise which 'node' -C"));
        assert!(content.contains("$input | & $runtime"));
        assert!(content.contains("\\entrypoints\\tool.js' @args"));
        assert!(!content.contains("mise env"));
    }
}
