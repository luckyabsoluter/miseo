use crate::{
    cli::output::Ui,
    error::Error,
    mise::Mise,
    spec::{RuntimeSpec, ToolSpec},
    tasks,
    workspace::Workspace,
};
use clap::ArgAction;

use super::Command;

#[derive(Debug, clap::Args, Clone)]
pub struct Install {
    /// Tool spec: <backend>:<name>[@version] (for example `npm:prettier` or `npm:prettier@3`).
    pub tool_spec: ToolSpec,

    /// Runtime selector(s): <runtime@selector> (repeatable, for example `--use node@22`).
    #[arg(long = "use", value_name = "RUNTIME@SELECTOR", action = ArgAction::Append)]
    pub uses: Vec<RuntimeSpec>,

    /// Reinstall even if the current installed variant already matches.
    #[arg(short = 'f', long = "force")]
    pub force: bool,

    /// Install through the runtime's global package manager instead of isolated install-into.
    #[arg(short = 'g', long = "global")]
    pub global: bool,
}

impl Command for Install {
    fn execute(
        self,
        workspace: &mut Workspace,
        mise: &impl Mise,
        ui: &Ui<'_>,
    ) -> Result<(), Error> {
        let tool_spec = self.tool_spec;

        match tasks::install::execute(
            mise,
            workspace,
            tool_spec.clone(),
            self.uses.try_into()?,
            self.force,
            if self.global {
                crate::workspace::InstallMode::Global
            } else {
                crate::workspace::InstallMode::Isolated
            },
        )? {
            Ok(success) => ui.install_success(&success),
            Err(already_current) => {
                ui.install_already_current(&already_current);
                ui.install_hints(&tool_spec, &already_current.tool_id);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    use crate::fs::PathBuf;
    use crate::{
        cli::{Cli, Commands, app::TestApp},
        spec::Runtime,
    };
    use clap::Parser;

    use super::*;

    fn command(input: &str) -> Install {
        let cli = Cli::try_parse_from(input.split_whitespace()).unwrap();

        let Commands::Install(args) = cli.command else {
            panic!("wrong command");
        };

        args
    }

    #[test]
    fn parses_install_force() {
        let args = command("miseo install npm:prettier --force");
        assert_eq!(args.tool_spec.to_string(), "npm:prettier");
        assert!(args.uses.is_empty());
        assert!(args.force);
        assert!(!args.global);
    }

    #[test]
    fn parses_install_global() {
        let args = command("miseo install -g npm:prettier");
        assert_eq!(args.tool_spec.to_string(), "npm:prettier");
        assert!(args.global);
    }

    #[test]
    fn parses_install_with_repeatable_use() {
        let args = command("miseo install npm:http-server --use node@22 --use python@3.12");
        assert_eq!(args.tool_spec.to_string(), "npm:http-server");
        assert_eq!(
            args.uses,
            vec![
                RuntimeSpec::new(Runtime::Node, "22"),
                RuntimeSpec::new(Runtime::Python, "3.12"),
            ]
        );
    }

    #[test]
    fn install_succeeds_and_reports_commands() {
        let mut test = TestApp::new();

        test.mise_mut()
            .use_g(Runtime::Node, "24", "24.13.1")
            .register_package("npm:prettier", "3.8.1", ["prettier"]);

        test.run(command("miseo install npm:prettier")).unwrap();

        let fs = test.fs();
        assert_eq!(fs.readlink(&test.path("npm-prettier/current")).unwrap(), {
            #[cfg(not(windows))]
            {
                Some(PathBuf::from("3.8.1+node-24.13.1"))
            }
            #[cfg(windows)]
            {
                Some(test.path("npm-prettier/3.8.1+node-24.13.1"))
            }
        });
        assert_eq!(
            fs.readlink(&test.path(".bin/prettier")).unwrap(),
            Some(test.root().join("npm-prettier/current/.miseo/prettier"))
        );
        assert!(
            fs.is_executable(&{
                #[cfg(not(windows))]
                {
                    test.path("npm-prettier/current/.miseo/prettier")
                }
                #[cfg(windows)]
                {
                    test.path("npm-prettier/current/.miseo/prettier.ps1")
                }
            })
            .unwrap()
        );

        test.out().assert_out(&[
            "miseo installed npm:prettier@3.8.1 [node@24.13.1] in ...ms",
            "  miseo installed command: prettier",
        ]);

        let manifest = test.manifest();
        assert!(manifest.has_tool_id_str("npm:prettier"));
        assert_eq!(
            manifest.current_variant_str("npm:prettier"),
            Some("3.8.1+node-24.13.1")
        );
        assert_eq!(manifest.variant_count_str("npm:prettier"), 1);
        assert_eq!(manifest.command_owner_str("prettier"), Some("npm:prettier"));
        assert_eq!(
            manifest.current_install_mode_str("npm:prettier"),
            Some("isolated")
        );
    }

    #[test]
    fn install_global_records_global_mode() {
        let mut test = TestApp::new();

        test.mise_mut()
            .use_g(Runtime::Node, "24", "24.13.1")
            .register_package("npm:prettier", "3.8.1", ["prettier"]);

        test.run(command("miseo install -g npm:prettier")).unwrap();

        let manifest = test.manifest();
        assert_eq!(
            manifest.current_install_mode_str("npm:prettier"),
            Some("global")
        );
    }

    #[test]
    fn install_with_use_uses_explicit_runtime_selector() {
        let mut test = TestApp::new();

        test.mise_mut()
            .current(Runtime::Node, "lts", "24.13.1")
            .register_package("npm:prettier", "3.8.1", ["prettier"]);

        test.run(command("miseo install npm:prettier --use node@lts"))
            .unwrap();

        let fs = test.fs();
        assert_eq!(
            fs.readlink(&test.path(".bin/prettier")).unwrap(),
            Some(test.path("npm-prettier/current/.miseo/prettier"))
        );
        assert_eq!(fs.readlink(&test.path("npm-prettier/current")).unwrap(), {
            #[cfg(not(windows))]
            {
                Some(PathBuf::from("3.8.1+node-24.13.1"))
            }
            #[cfg(windows)]
            {
                Some(test.path("npm-prettier/3.8.1+node-24.13.1"))
            }
        });
        assert!(
            fs.is_executable(&{
                #[cfg(not(windows))]
                {
                    test.path("npm-prettier/current/.miseo/prettier")
                }
                #[cfg(windows)]
                {
                    test.path("npm-prettier/current/.miseo/prettier.ps1")
                }
            })
            .unwrap()
        );

        test.out().assert_out(&[
            "miseo installed npm:prettier@3.8.1 [node@24.13.1] in ...ms",
            "  miseo installed command: prettier",
        ]);

        let mise_toml = test.path("npm-prettier/3.8.1+node-24.13.1/mise.toml");
        let content = test.fs().read_file(&mise_toml).unwrap();
        assert_eq!(content, "[tools]\nnode = \"24.13.1\"\n");

        let manifest = test.manifest();
        assert_eq!(
            manifest.current_variant_str("npm:prettier"),
            Some("3.8.1+node-24.13.1")
        );
        assert_eq!(manifest.command_owner_str("prettier"), Some("npm:prettier"));
    }

    #[test]
    fn install_rejects_conflicting_owner() {
        let mut test = TestApp::new();

        test.mise_mut()
            .use_g(Runtime::Node, "22", "22.13.1")
            .register_package("npm:http-server", "14.1.1", ["http-server"])
            .register_package("npm:other-tool", "1.0.0", ["http-server"]);

        test.run(command("miseo install npm:http-server")).unwrap();

        let err = test
            .run(command("miseo install npm:other-tool"))
            .unwrap_err();

        assert!(matches!(err, Error::CommandOwnershipConflict { .. }));

        let manifest = test.manifest();
        assert_eq!(
            manifest.command_owner_str("http-server"),
            Some("npm:http-server")
        );
        assert!(manifest.has_tool_id_str("npm:http-server"));
        assert!(!manifest.has_tool_id_str("npm:other-tool"));
    }

    #[test]
    fn install_force_reinstalls_current_variant() {
        let mut test = TestApp::new();

        test.mise_mut()
            .use_g(Runtime::Node, "24", "24.13.1")
            .register_package("npm:prettier", "3.8.1", ["prettier"]);

        test.run(command("miseo install npm:prettier")).unwrap();

        test.run(command("miseo install npm:prettier --force"))
            .unwrap();

        let fs = test.fs();
        assert_eq!(fs.readlink(&test.path("npm-prettier/current")).unwrap(), {
            #[cfg(not(windows))]
            {
                Some(PathBuf::from("3.8.1+node-24.13.1"))
            }
            #[cfg(windows)]
            {
                Some(test.path("npm-prettier/3.8.1+node-24.13.1"))
            }
        });
        assert_eq!(
            fs.readlink(&test.path(".bin/prettier")).unwrap(),
            Some(test.path("npm-prettier/current/.miseo/prettier"))
        );
        assert!(
            fs.is_executable(&{
                #[cfg(not(windows))]
                {
                    test.path("npm-prettier/current/.miseo/prettier")
                }
                #[cfg(windows)]
                {
                    test.path("npm-prettier/current/.miseo/prettier.ps1")
                }
            })
            .unwrap()
        );

        test.out().assert_out(&[
            "miseo installed npm:prettier@3.8.1 [node@24.13.1] in ...ms",
            "  miseo installed command: prettier",
            "miseo reinstalled npm:prettier@3.8.1 [node@24.13.1] in ...ms",
            "  miseo installed command: prettier",
        ]);

        let manifest = test.manifest();
        assert_eq!(manifest.variant_count_str("npm:prettier"), 1);
        assert_eq!(
            manifest.current_variant_str("npm:prettier"),
            Some("3.8.1+node-24.13.1")
        );
    }

    #[test]
    fn install_reports_already_current_and_hints() {
        let mut test = TestApp::new();

        test.mise_mut()
            .use_g(Runtime::Node, "24", "24.13.1")
            .register_package("npm:prettier", "3.8.1", ["prettier"]);

        test.run(command("miseo install npm:prettier")).unwrap();
        test.run(command("miseo install npm:prettier")).unwrap();

        test.out().assert_out(&[
            "miseo installed npm:prettier@3.8.1 [node@24.13.1] in ...ms",
            "  miseo installed command: prettier",
            "miseo npm:prettier@3.8.1 [node@24.13.1] already current",
            "  hint: miseo install npm:prettier --force    # reinstall current variant",
            "  hint: miseo upgrade npm:prettier            # upgrade to newer version",
        ]);

        let manifest = test.manifest();
        assert_eq!(manifest.variant_count_str("npm:prettier"), 1);
    }

    #[test]
    fn install_rejects_duplicate_runtime_use() {
        let test = TestApp::new();

        let err = test
            .run(command(
                "miseo install npm:prettier --use node@22 --use node@lts",
            ))
            .unwrap_err();

        assert!(matches!(err, Error::DuplicateRuntimeUse { .. }));
    }

    #[test]
    fn install_cleans_new_tool_dir_on_no_commands_discovered() {
        let mut test = TestApp::new();

        test.mise_mut()
            .use_g(Runtime::Node, "24", "24.13.1")
            .register_package("npm:no-bin", "1.0.0", [""; 0]);

        let err = test.run(command("miseo install npm:no-bin")).unwrap_err();

        assert!(matches!(err, Error::ManifestInvariant(_)));
        assert!(!test.fs().exists(&test.path("npm-no-bin")).unwrap());
        let manifest = test.manifest();
        assert!(manifest.tools_is_empty());
        assert!(manifest.owners_is_empty());
    }
}
