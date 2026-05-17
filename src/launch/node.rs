//! Node-specific command launch extension.

use std::fs;

use crate::{
    fs::{Path, PathBuf},
    launch::CommandTarget,
    spec::Runtime,
};

/// Build a command target for a package bin that may be a Node entrypoint.
pub fn package_bin_target(
    package_dir: &Path,
    relative_entrypoint: &str,
    fallback_executable: PathBuf,
) -> CommandTarget {
    let entrypoint = package_dir.join(relative_entrypoint);

    if is_entrypoint(&entrypoint) {
        CommandTarget::runtime_entrypoint(Runtime::Node, entrypoint)
    } else {
        CommandTarget::env_wrapped(fallback_executable)
    }
}

fn is_entrypoint(path: &Path) -> bool {
    if path.extension().is_some_and(is_script_extension) {
        return true;
    }

    let Ok(content) = fs::read_to_string(path.as_std_path()) else {
        return false;
    };

    content
        .lines()
        .next()
        .is_some_and(|line| line.trim_start().starts_with("#!") && line.contains("node"))
}

fn is_script_extension(ext: &str) -> bool {
    matches!(ext.to_ascii_lowercase().as_str(), "js" | "cjs" | "mjs")
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::{
        fs::PathBuf,
        launch::{CommandTarget, node::package_bin_target},
        spec::Runtime,
    };

    #[test]
    fn package_bin_target_uses_direct_node_entrypoint_for_js_bin() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let package_dir = root.join("node_modules/pkg");
        let fallback = root.join("bin/pkg");

        let target = package_bin_target(&package_dir, "bin/pkg.js", fallback);

        assert_eq!(
            target,
            CommandTarget::runtime_entrypoint(Runtime::Node, package_dir.join("bin/pkg.js"))
        );
    }

    #[test]
    fn package_bin_target_uses_direct_node_entrypoint_for_node_shebang() {
        let tmp = tempdir().unwrap();
        let root = PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let package_dir = root.join("node_modules/pkg");
        let entrypoint = package_dir.join("bin/pkg");
        let fallback = root.join("bin/pkg");
        std::fs::create_dir_all(entrypoint.parent().unwrap().as_std_path()).unwrap();
        std::fs::write(entrypoint.as_std_path(), "#!/usr/bin/env node\n").unwrap();

        let target = package_bin_target(&package_dir, "bin/pkg", fallback);

        assert_eq!(
            target,
            CommandTarget::runtime_entrypoint(Runtime::Node, package_dir.join("bin/pkg"))
        );
    }

    #[test]
    fn package_bin_target_falls_back_for_non_node_bin() {
        let package_dir = PathBuf::from("/tmp/node_modules/pkg");
        let fallback = PathBuf::from("/tmp/bin/pkg");

        let target = package_bin_target(&package_dir, "bin/pkg", fallback.clone());

        assert_eq!(target, CommandTarget::env_wrapped(fallback));
    }
}
