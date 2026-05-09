use crate::{cli::output::Ui, error::Error, mise::Mise, spec::ToolId, tasks, workspace::Workspace};

use super::Command;

#[derive(Debug, clap::Args, Clone)]
pub struct Uninstall {
    /// Installed tool id: <backend>:<name> (version suffix is not accepted).
    pub tool_spec: ToolId,

    /// Also clean orphaned ~/.miseo tool directories and matching stale public symlinks.
    #[arg(short = 'f', long = "force")]
    pub force: bool,
}

impl Command for Uninstall {
    fn execute(
        self,
        workspace: &mut Workspace,
        mise: &impl Mise,
        ui: &Ui<'_>,
    ) -> Result<(), Error> {
        let outcome = tasks::uninstall::execute(mise, workspace, self.tool_spec, self.force)?;
        ui.uninstalled(&outcome);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        cli::{Cli, Commands, app::TestApp},
        error::Error,
        spec::Runtime,
    };
    use clap::Parser;

    use super::*;

    fn cli_command(input: &str) -> Commands {
        Cli::try_parse_from(input.split_whitespace())
            .unwrap()
            .command
    }

    fn command(input: &str) -> Uninstall {
        let Commands::Uninstall(args) = cli_command(input) else {
            panic!("wrong command");
        };

        args
    }

    #[test]
    fn parses_uninstall_and_alias() {
        let args = command("miseo uninstall npm:http-server");
        assert_eq!(args.tool_spec.to_string(), "npm:http-server");
        assert!(!args.force);

        let args = command("miseo remove npm:http-server");
        assert_eq!(args.tool_spec.to_string(), "npm:http-server");
        assert!(!args.force);
    }

    #[test]
    fn parses_uninstall_force() {
        let args = command("miseo uninstall npm:prettier --force");
        assert_eq!(args.tool_spec.to_string(), "npm:prettier");
        assert!(args.force);
    }

    #[test]
    fn rejects_versioned_tool_id_for_uninstall() {
        assert!(Cli::try_parse_from(["miseo", "uninstall", "npm:prettier@3"]).is_err());
    }

    #[test]
    fn uninstall_cleans_manifest_links_and_tool_dir() {
        let mut test = TestApp::new();

        test.mise_mut()
            .use_g(Runtime::Node, "24", "24.13.1")
            .register_package("npm:prettier", "3.8.1", ["prettier"]);

        test.run(cli_command("miseo install npm:prettier")).unwrap();
        test.out().clear();

        test.run(command("miseo uninstall npm:prettier")).unwrap();

        let fs = test.fs();
        assert!(!fs.exists(&test.path(".bin/prettier")).unwrap());
        assert!(!fs.exists(&test.path("npm-prettier")).unwrap());
        assert!(test.mise().global_uninstalls().is_empty());

        test.out().assert_out(&[
            "miseo uninstalled npm:prettier",
            "  miseo removed command: prettier",
        ]);

        let loaded = test.manifest();
        assert!(loaded.tools_is_empty());
        assert!(loaded.owners_is_empty());
    }

    #[test]
    fn uninstall_global_install_runs_npm_uninstall() {
        let mut test = TestApp::new();

        test.mise_mut()
            .use_g(Runtime::Node, "24", "24.13.1")
            .register_package("npm:prettier", "3.8.1", ["prettier"]);

        test.run(cli_command("miseo install -g npm:prettier"))
            .unwrap();
        test.out().clear();

        test.run(command("miseo uninstall npm:prettier")).unwrap();

        assert_eq!(
            test.mise().global_uninstalls(),
            vec![(
                "npm:prettier".to_string(),
                test.path("npm-prettier")
                    .join("3.8.1+node-24.13.1")
                    .to_string()
            )]
        );
    }

    #[test]
    fn uninstall_orphan_requires_force() {
        let test = TestApp::new();
        let fs = test.fs();

        fs.mkdir_p(&test.path(".bin")).unwrap();
        fs.mkdir_p(&test.path("gem-rack")).unwrap();
        fs.ln_s(
            &test.path("gem-rack/current/.miseo/rack"),
            &test.path(".bin/rack"),
        )
        .unwrap();

        let err = test.run(command("miseo uninstall gem:rack")).unwrap_err();

        assert!(matches!(err, Error::ToolNotInstalledOrphanFound { .. }));

        assert!(fs.exists(&test.path(".bin/rack")).unwrap());
        assert!(fs.exists(&test.path("gem-rack")).unwrap());
        assert_eq!(
            fs.readlink(&test.path(".bin/rack")).unwrap(),
            Some(test.path("gem-rack/current/.miseo/rack"))
        );
    }

    #[test]
    fn uninstall_cleans_orphan_dir_with_force() {
        let test = TestApp::new();
        let fs = test.fs();

        fs.mkdir_p(&test.path(".bin")).unwrap();
        fs.mkdir_p(&test.path("gem-rack")).unwrap();
        fs.ln_s(
            &test.path("gem-rack/current/.miseo/rack"),
            &test.path(".bin/rack"),
        )
        .unwrap();

        test.run(command("miseo uninstall gem:rack --force"))
            .unwrap();

        assert!(!fs.exists(&test.path(".bin/rack")).unwrap());
        assert!(!fs.exists(&test.path("gem-rack")).unwrap());

        test.out().assert_out(&[
            "miseo uninstalled gem:rack",
            "  miseo removed command: rack",
        ]);
    }

    #[test]
    fn uninstall_force_orphan_cleanup_preserves_unrelated_links() {
        let test = TestApp::new();
        let fs = test.fs();

        fs.mkdir_p(&test.path(".bin")).unwrap();
        fs.mkdir_p(&test.path("gem-rack")).unwrap();
        fs.ln_s(
            &test.path("gem-rack/current/.miseo/rack"),
            &test.path(".bin/rack"),
        )
        .unwrap();
        fs.ln_s(
            &test.path("npm-prettier/current/.miseo/prettier"),
            &test.path(".bin/prettier"),
        )
        .unwrap();

        test.run(command("miseo uninstall gem:rack --force"))
            .unwrap();

        assert!(!fs.exists(&test.path(".bin/rack")).unwrap());
        assert!(!fs.exists(&test.path("gem-rack")).unwrap());
        assert!(fs.exists(&test.path(".bin/prettier")).unwrap());
        assert_eq!(
            fs.readlink(&test.path(".bin/prettier")).unwrap(),
            Some(test.path("npm-prettier/current/.miseo/prettier"))
        );

        test.out().assert_out(&[
            "miseo uninstalled gem:rack",
            "  miseo removed command: rack",
        ]);
    }
}
