//! CLI runtime wiring, workspace transaction boundary, and test harness.

use super::output::{self, Output, Ui};

use crate::{
    cli::{Cli, commands::Command as CliCommand, output::Term},
    error::{Error, invariant},
    fs::{self, Fs, PathBuf},
    mise::{self, Mise},
    workspace::Workspace,
};

#[cfg(test)]
use crate::{fs::Path, workspace::manifest::Manifest};

pub struct App<M, O> {
    mise: M,
    root: PathBuf,
    fs: &'static dyn Fs,
    out: O,
    verbose: bool,
}

impl App<mise::Cli, output::Term> {
    pub fn execute(cli: Cli) {
        let app = Self::for_cli(&cli);

        if let Err(err) = app.run(cli.command) {
            app.report(&err);
            std::process::exit(err.exit_code());
        }
    }

    fn for_cli(cli: &Cli) -> Self {
        let fs = fs::new();
        let out = Term;
        let mise = mise::new(out.is_tty(), cli.verbose);
        let verbose = cli.verbose > 0;

        Self::from_env(mise, fs, out, verbose).unwrap_or_else(|err| {
            Ui::new(&out, false).error(&err);
            std::process::exit(err.exit_code())
        })
    }
}

impl<M: Mise, O: Output> App<M, O> {
    pub fn new(
        root: PathBuf,
        mise: M,
        fs: &'static dyn crate::fs::Fs,
        out: O,
        verbose: bool,
    ) -> Self {
        Self {
            mise,
            root,
            fs,
            out,
            verbose,
        }
    }

    fn from_env(
        mise: M,
        fs: &'static dyn crate::fs::Fs,
        out: O,
        verbose: bool,
    ) -> Result<Self, Error> {
        let dir = home::home_dir().ok_or_else(|| invariant!("could not resolve home directory"))?;
        let root = PathBuf::from_path_buf(dir.join(".miseo"))
            .map_err(|_| invariant!("home directory path is not valid UTF-8"))?;

        Ok(Self::new(root, mise, fs, out, verbose))
    }

    fn mise(&self) -> &M {
        &self.mise
    }

    #[cfg(test)]
    pub(super) fn fs(&self) -> &'static dyn Fs {
        self.fs
    }

    fn interactive(&self) -> bool {
        self.out.is_tty()
    }

    fn ui(&self) -> Ui<'_> {
        let concise = self.interactive() && !self.verbose;
        Ui::new(&self.out, concise)
    }

    fn error_ui(&self) -> Ui<'_> {
        Ui::new(&self.out, false)
    }

    pub(super) fn run(&self, command: impl CliCommand) -> Result<(), Error> {
        let ui = self.ui();
        self.with_workspace(|workspace| command.execute(workspace, self.mise(), &ui))
    }

    fn with_workspace<T>(
        &self,
        f: impl FnOnce(&mut Workspace) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut workspace = Workspace::open(self.root.clone(), self.fs)?;
        let out = f(&mut workspace)?;

        // FIXME: this gives logical transaction boundaries for manifest state,
        // but external side effects (filesystem/mise commands) are not rollback-safe.
        workspace.commit()?;

        Ok(out)
    }

    fn report(&self, err: &Error) {
        self.error_ui().error(err);
    }
}

#[cfg(test)]
pub(super) struct TestApp {
    app: App<mise::Test, output::Test>,
    _tmp: tempfile::TempDir,
}

#[cfg(test)]
impl TestApp {
    pub fn new() -> Self {
        let _tmp: tempfile::TempDir = tempfile::tempdir().unwrap();
        let root = PathBuf::from_path_buf(_tmp.path().to_path_buf()).unwrap();
        let mise = mise::Test::default();
        let fs = fs::new();
        let out = output::Test::new(false);

        let app = App::new(root, mise, fs, out, false);

        Self { app, _tmp }
    }

    pub fn root(&self) -> &PathBuf {
        &self.app.root
    }

    pub fn path(&self, path: impl AsRef<Path>) -> PathBuf {
        self.app.root.join(path)
    }

    pub fn mise_mut(&mut self) -> &mut mise::Test {
        &mut self.app.mise
    }

    pub fn mise(&self) -> &mise::Test {
        &self.app.mise
    }

    pub fn out(&self) -> &output::Test {
        &self.app.out
    }

    pub fn manifest(&self) -> Manifest {
        Manifest::load(&self.root.join(".miseo-installs.toml")).unwrap()
    }
}

#[cfg(test)]
impl std::ops::Deref for TestApp {
    type Target = App<mise::Test, output::Test>;

    fn deref(&self) -> &Self::Target {
        &self.app
    }
}

#[cfg(test)]
impl std::ops::DerefMut for TestApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.app
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::{
        cli::{
            commands::Command as TestCommand,
            output::{self, Ui},
        },
        error::Error,
        mise::Mise,
        spec::Runtime,
        workspace::{InstallRecord, Workspace},
    };

    use super::{App, TestApp};

    struct RecordInstallAndSucceed;

    impl TestCommand for RecordInstallAndSucceed {
        fn execute(
            self,
            workspace: &mut Workspace,
            _mise: &impl Mise,
            _ui: &Ui<'_>,
        ) -> Result<(), Error> {
            record_dummy_install(workspace);
            Ok(())
        }
    }

    struct RecordInstallAndFail;

    impl TestCommand for RecordInstallAndFail {
        fn execute(
            self,
            workspace: &mut Workspace,
            _mise: &impl Mise,
            _ui: &Ui<'_>,
        ) -> Result<(), Error> {
            record_dummy_install(workspace);
            Err(Error::ToolNotInstalled {
                tool_id: "npm:missing".to_string(),
            })
        }
    }

    fn record_dummy_install(workspace: &mut Workspace) {
        let mut runtimes = BTreeMap::new();
        runtimes.insert(Runtime::Node, "24.13.1".to_string());

        workspace.record_install(InstallRecord {
            tool_id: "npm:prettier".parse().unwrap(),
            variant_key: "3.8.1+node-24.13.1".to_string(),
            package_version: "3.8.1".to_string(),
            runtimes,
            install_dir: workspace.root().join("npm-prettier/3.8.1+node-24.13.1"),
            commands: vec!["prettier".to_string()],
            stale_commands: vec![],
            install_mode: crate::workspace::InstallMode::Isolated,
        });
    }

    #[test]
    fn report_writes_human_lines() {
        let test = TestApp::new();
        let app = &*test;

        app.report(&crate::error::Error::ToolNotInstalledOrphanFound {
            tool_id: "gem:webrick".to_string(),
            path: "/tmp/miseo/gem-webrick".to_string(),
        });

        test.out().assert_err(&[
            "error: tool gem:webrick is not installed",
            "hint: found unmanaged install at /tmp/miseo/gem-webrick; \
             run miseo uninstall gem:webrick --force to clean it up",
        ]);
    }

    #[test]
    fn run_commits_manifest_changes_on_success() {
        let test = TestApp::new();
        let manifest_path = test.path(".miseo-installs.toml");

        test.run(RecordInstallAndSucceed).unwrap();

        assert!(test.fs().exists(&manifest_path).unwrap());

        let manifest = test.manifest();
        assert!(!manifest.tools_is_empty());
        assert!(!manifest.owners_is_empty());
    }

    #[test]
    fn run_does_not_commit_manifest_changes_on_error() {
        let test = TestApp::new();
        let manifest_path = test.path(".miseo-installs.toml");

        let err = test.run(RecordInstallAndFail).unwrap_err();

        assert!(matches!(err, Error::ToolNotInstalled { .. }));
        assert!(!test.fs().exists(&manifest_path).unwrap());
    }

    #[test]
    fn ui_is_concise_only_when_tty_and_not_verbose() {
        let cases = [
            (false, false, false),
            (false, true, false),
            (true, false, true),
            (true, true, false),
        ];

        for (tty, verbose, concise) in cases {
            let out = output::Test::new(tty);
            let app = App::new(
                "/tmp/miseo-test".to_string().into(),
                crate::mise::Test::default(),
                crate::fs::new(),
                out,
                verbose,
            );

            assert_eq!(
                app.ui().is_concise(),
                concise,
                "unexpected concise mode for tty={tty}, verbose={verbose}"
            );
        }
    }
}
