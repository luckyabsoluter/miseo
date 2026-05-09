use std::{collections::BTreeMap, time::Instant};

use crate::{
    error::Error,
    fs::PathBuf,
    mise::Mise,
    spec::{RuntimePins, ToolId, ToolSpec},
    workspace::{InstallMode, InstallPlan, Workspace},
};

use super::with_new_tool_cleanup;

pub type Outcome = Result<Success, AlreadyCurrent>;

/// Successful install details for UI rendering.
#[derive(Debug, Clone)]
pub struct Success {
    /// Whether this was a fresh install or forced reinstall.
    pub action: Action,
    /// Stable tool identity (`<backend>:<name>`).
    pub tool_id: ToolId,
    /// Resolved package version.
    pub package_version: String,
    /// Resolved runtime pins rendered as labels (`node@24.13.1`, ...).
    pub runtimes: Vec<String>,
    /// Exported command names now linked in `~/.miseo/.bin`.
    pub commands: Vec<String>,
    /// End-to-end task duration in milliseconds.
    pub elapsed_ms: u128,
}

/// Install action verb used in output lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Installed,
    Reinstalled,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Reinstalled => "reinstalled",
        }
    }
}

/// Already-current install details.
#[derive(Debug, Clone)]
pub struct AlreadyCurrent {
    /// Stable tool identity (`<backend>:<name>`).
    pub tool_id: ToolId,
    /// Resolved package version.
    pub package_version: String,
    /// Resolved runtime pins rendered as labels (`node@24.13.1`, ...).
    pub runtimes: Vec<String>,
}

pub fn execute(
    mise: &impl Mise,
    workspace: &mut Workspace,
    tool_spec: ToolSpec,
    uses: RuntimePins,
    force: bool,
    install_mode: InstallMode,
) -> Result<Outcome, Error> {
    let runtime_pins = mise.resolve_runtime_pins(tool_spec.backend(), &uses)?;

    let requested = tool_spec.or_latest();
    let exact_spec = mise.resolve_latest_version(&runtime_pins, &requested)?;
    let plan = workspace.plan_install(&exact_spec, &runtime_pins)?;

    let tool_id = plan.tool_id().clone();
    let runtime_labels = plan.runtime_labels();

    if plan.current_matches() && !force {
        return Ok(Err(AlreadyCurrent {
            tool_id,
            package_version: plan.package_version().to_string(),
            runtimes: runtime_labels,
        }));
    }

    let action = if plan.current_matches() {
        Action::Reinstalled
    } else {
        Action::Installed
    };

    let started = Instant::now();
    let commands = match install_mode {
        InstallMode::Isolated => install_isolated(mise, workspace, &exact_spec, &plan)?,
        InstallMode::Global => install_global(mise, workspace, &exact_spec, &plan)?,
    };
    let finalized = workspace.finalize_install(&plan, install_mode, commands);
    let exported_commands = with_new_tool_cleanup(workspace, &plan, finalized)?;

    Ok(Ok(Success {
        action,
        tool_id,
        package_version: plan.package_version().to_string(),
        runtimes: runtime_labels,
        commands: exported_commands,
        elapsed_ms: started.elapsed().as_millis(),
    }))
}

fn install_isolated(
    mise: &impl Mise,
    workspace: &Workspace,
    exact_spec: &ToolSpec,
    plan: &InstallPlan,
) -> Result<BTreeMap<String, PathBuf>, Error> {
    with_new_tool_cleanup(
        workspace,
        plan,
        mise.install_into(
            plan.runtime_pins(),
            exact_spec,
            plan.variant().variant_dir(),
        ),
    )?;
    let mise_toml = with_new_tool_cleanup(
        workspace,
        plan,
        workspace.initialize_variant(plan.variant(), plan.runtime_pins()),
    )?;
    with_new_tool_cleanup(workspace, plan, mise.trust_config(&mise_toml))?;

    let bin_paths = with_new_tool_cleanup(
        workspace,
        plan,
        mise.bin_paths(plan.tool_id(), plan.variant().variant_dir()),
    )?;

    with_new_tool_cleanup(workspace, plan, workspace.discover_executables(&bin_paths))
}

fn install_global(
    mise: &impl Mise,
    workspace: &Workspace,
    exact_spec: &ToolSpec,
    plan: &InstallPlan,
) -> Result<BTreeMap<String, PathBuf>, Error> {
    let mise_toml = with_new_tool_cleanup(
        workspace,
        plan,
        workspace.initialize_variant(plan.variant(), plan.runtime_pins()),
    )?;
    with_new_tool_cleanup(workspace, plan, mise.trust_config(&mise_toml))?;
    with_new_tool_cleanup(
        workspace,
        plan,
        mise.install_global(exact_spec, plan.variant().variant_dir()),
    )?;

    with_new_tool_cleanup(
        workspace,
        plan,
        mise.installed_command_targets(exact_spec, plan.variant().variant_dir()),
    )
}
