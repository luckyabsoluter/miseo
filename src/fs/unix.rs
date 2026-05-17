use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    error::{Error, invariant},
    launch::CommandTarget,
};

use super::{Fs, Path, PathBuf};

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct UnixFs;

impl Fs for UnixFs {
    fn is_executable(&self, path: &Path) -> Result<bool, Error> {
        Ok(fs::metadata(path)?.permissions().mode() & 0o111 != 0)
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
        Ok(fs::create_dir_all(path)?)
    }

    fn readlink(&self, path: &Path) -> Result<Option<PathBuf>, Error> {
        match fs::read_link(path.as_std_path()) {
            Ok(target) => {
                let target = PathBuf::from_path_buf(target)
                    .map_err(|_| invariant!("non-utf8 symlink target path"))?;
                Ok(Some(target))
            }
            Err(err) if is_invalid_input_or_not_found(&err) => Ok(None),
            Err(err) => Err(Error::Io(err)),
        }
    }

    fn ln_s(&self, target: &Path, link_path: &Path) -> Result<(), Error> {
        let parent: &Path = link_path
            .parent()
            .ok_or_else(|| invariant!("path '{link_path}' has no parent"))?;

        match fs::symlink_metadata(link_path.as_std_path()) {
            Ok(meta) if meta.file_type().is_dir() => {
                return Err(invariant!(
                    "cannot replace directory '{link_path}' with symlink"
                ));
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(Error::Io(err)),
        }

        self.mkdir_p(parent)?;

        let tmp = temp_path(parent, ".miseo-link")?;
        symlink(target.as_std_path(), tmp.as_std_path())?;
        fs::rename(tmp.as_std_path(), link_path.as_std_path())?;
        Ok(())
    }

    fn rm(&self, path: &Path) -> Result<(), Error> {
        remove_file_if_exists(path)
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

        let mut perms = fs::metadata(tmp.as_std_path())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(tmp.as_std_path(), perms)?;

        fs::rename(tmp.as_std_path(), path.as_std_path())?;

        Ok(())
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

        let tmp = temp_path(parent, ".miseo-file")?;
        fs::write(tmp.as_std_path(), shim_content(project_dir, target))?;

        let mut perms = fs::metadata(tmp.as_std_path())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(tmp.as_std_path(), perms)?;
        fs::rename(tmp.as_std_path(), path.as_std_path())?;
        Ok(())
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
    match fs::remove_dir_all(path.as_std_path()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::Io(err)),
    }
}

fn is_invalid_input_or_not_found(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::InvalidInput || err.kind() == std::io::ErrorKind::NotFound
}

fn shell_single_quote(input: &str) -> String {
    input.replace('\'', "'\\''")
}

fn shim_content(project_dir: &Path, target: &CommandTarget) -> String {
    match target {
        CommandTarget::EnvWrapped(target) => mise_env_shim_content(project_dir, target),
        CommandTarget::RuntimeEntrypoint {
            runtime,
            entrypoint,
        } => runtime_entrypoint_shim_content(project_dir, runtime.as_ref(), entrypoint),
    }
}

fn runtime_entrypoint_shim_content(project_dir: &Path, runtime: &str, entrypoint: &Path) -> String {
    let escaped_project_dir = shell_single_quote(project_dir.as_str());
    let escaped_runtime = shell_single_quote(runtime);
    let escaped_entrypoint = shell_single_quote(entrypoint.as_str());
    format!(
        "#!/bin/sh\nruntime_path=\"$(mise which '{escaped_runtime}' -C '{escaped_project_dir}')\" || exit $?\nexec \"$runtime_path\" '{escaped_entrypoint}' \"$@\"\n"
    )
}

fn mise_env_shim_content(project_dir: &Path, target: &Path) -> String {
    let escaped_project_dir = shell_single_quote(project_dir.as_str());
    let escaped_target = shell_single_quote(target.as_str());
    format!(
        "#!/bin/sh\neval \"$(mise env -C '{escaped_project_dir}' -s bash)\"\nexec '{escaped_target}' \"$@\"\n"
    )
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use crate::error::Error;

    use crate::launch::CommandTarget;

    use super::{Fs, PathBuf, UnixFs};

    #[test]
    fn ln_s_points_to_new_target() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();

        let target_a = root.join("a");
        let target_b = root.join("b");
        fs::write(target_a.as_std_path(), "a").unwrap();
        fs::write(target_b.as_std_path(), "b").unwrap();

        let link = root.join("ln");
        UnixFs.ln_s(&target_a, &link).unwrap();
        UnixFs.ln_s(&target_b, &link).unwrap();

        let actual = fs::read_link(link.as_std_path()).unwrap();
        assert_eq!(actual, target_b.as_std_path());
    }

    #[test]
    fn ln_s_replaces_existing_file() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();

        let target = root.join("target");
        fs::write(target.as_std_path(), "x").unwrap();

        let link = root.join("link");
        fs::write(link.as_std_path(), "not a symlink").unwrap();

        UnixFs.ln_s(&target, &link).unwrap();

        let actual = fs::read_link(link.as_std_path()).unwrap();
        assert_eq!(actual, target.as_std_path());
    }

    #[test]
    fn ln_s_replaces_symlink_to_directory() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();

        let dir_a = root.join("a");
        let dir_b = root.join("b");
        fs::create_dir_all(dir_a.as_std_path()).unwrap();
        fs::create_dir_all(dir_b.as_std_path()).unwrap();

        let link = root.join("current");
        UnixFs.ln_s(&dir_a, &link).unwrap();
        UnixFs.ln_s(&dir_b, &link).unwrap();

        let actual = fs::read_link(link.as_std_path()).unwrap();
        assert_eq!(actual, dir_b.as_std_path());
    }

    #[test]
    fn ln_s_rejects_existing_directory() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();

        let target = root.join("target");
        fs::write(target.as_std_path(), "x").unwrap();

        let link = root.join("dir");
        fs::create_dir_all(link.as_std_path()).unwrap();

        let err = UnixFs.ln_s(&target, &link).unwrap_err();
        assert!(matches!(err, Error::ManifestInvariant(_)));
    }

    #[test]
    fn write_command_shim_uses_mise_env_for_env_wrapped_targets() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();

        let target = CommandTarget::env_wrapped(root.join("global-bin/target-bin"));
        let shim = root.join("shim");
        UnixFs.write_command_shim(&root, &target, &shim).unwrap();

        let content = fs::read_to_string(shim.as_std_path()).unwrap();
        assert!(content.starts_with("#!/bin/sh\neval \"$(mise env -C '"));
        assert!(content.contains("global-bin/target-bin' \"$@\""));

        let mode = {
            fs::metadata(shim.as_std_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn write_command_shim_runs_node_entrypoint_without_mise_env() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();

        let entrypoint = root.join("entrypoints/tool.js");
        let target = CommandTarget::runtime_entrypoint(crate::spec::Runtime::Node, entrypoint);
        let shim = root.join("shim");
        UnixFs.write_command_shim(&root, &target, &shim).unwrap();

        let content = fs::read_to_string(shim.as_std_path()).unwrap();
        assert!(content.contains("mise which 'node' -C"));
        assert!(content.contains("exec \"$runtime_path\" '"));
        assert!(content.contains("entrypoints/tool.js' \"$@\""));
        assert!(!content.contains("mise env"));
    }

    #[test]
    fn mkdir_p_is_idempotent() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let dir = root.join("a/b/c");

        UnixFs.mkdir_p(&dir).unwrap();
        UnixFs.mkdir_p(&dir).unwrap();

        assert!(dir.exists());
        assert!(dir.is_dir());
    }

    #[test]
    fn is_executable_reports_mode() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();

        let exec = root.join("exec");
        let plain = root.join("plain");

        fs::write(exec.as_std_path(), "x").unwrap();
        fs::write(plain.as_std_path(), "x").unwrap();
        fs::set_permissions(exec.as_std_path(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(plain.as_std_path(), fs::Permissions::from_mode(0o644)).unwrap();

        assert!(UnixFs.is_executable(&exec).unwrap());
        assert!(!UnixFs.is_executable(&plain).unwrap());
    }

    #[test]
    fn ls_lists_directory_entries() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let dir = root.join("d");
        fs::create_dir_all(dir.as_std_path()).unwrap();
        fs::write(dir.join("a").as_std_path(), "a").unwrap();
        fs::write(dir.join("b").as_std_path(), "b").unwrap();

        let mut names = UnixFs
            .ls(&dir)
            .unwrap()
            .into_iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string()))
            .collect::<Vec<_>>();
        names.sort();

        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn readlink_handles_symlink_and_non_symlink() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();

        let target = root.join("target");
        fs::write(target.as_std_path(), "x").unwrap();

        let link = root.join("link");
        std::os::unix::fs::symlink(target.as_std_path(), link.as_std_path()).unwrap();
        assert_eq!(UnixFs.readlink(&link).unwrap(), Some(target.clone()));

        let regular = root.join("regular");
        fs::write(regular.as_std_path(), "x").unwrap();
        assert_eq!(UnixFs.readlink(&regular).unwrap(), None);

        let missing = root.join("missing");
        assert_eq!(UnixFs.readlink(&missing).unwrap(), None);
    }

    #[test]
    fn rm_removes_file_and_ignores_missing() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let file = root.join("f");

        fs::write(file.as_std_path(), "x").unwrap();
        UnixFs.rm(&file).unwrap();
        assert!(!file.exists());

        UnixFs.rm(&file).unwrap();
    }

    #[test]
    fn rm_rf_removes_directory_tree_and_ignores_missing() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let dir = root.join("tree");
        fs::create_dir_all(dir.join("nested").as_std_path()).unwrap();
        fs::write(dir.join("nested/file").as_std_path(), "x").unwrap();

        UnixFs.rm_rf(&dir).unwrap();
        assert!(!dir.exists());

        UnixFs.rm_rf(&dir).unwrap();
    }

    #[test]
    fn write_file_replaces_content() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let file = root.join("f");

        UnixFs.write_file(&file, "a").unwrap();
        assert_eq!(fs::read_to_string(file.as_std_path()).unwrap(), "a");

        UnixFs.write_file(&file, "b").unwrap();
        assert_eq!(fs::read_to_string(file.as_std_path()).unwrap(), "b");
    }
}
