use crate::{
    error::Error,
    mise::Mise,
    spec::{RuntimePins, ToolId},
    workspace::Workspace,
};

use super::install::{self, Action as InstallAction};

pub type Outcome = Result<Success, AlreadyCurrent>;

/// Successful upgrade details for UI rendering.
#[derive(Debug, Clone)]
pub struct Success {
    /// Stable tool identity (`<backend>:<name>`).
    pub tool_id: ToolId,
    /// New resolved package version.
    pub package_version: String,
    /// Runtime pins used for the upgraded variant.
    pub runtimes: Vec<String>,
    /// Exported command names now linked in `~/.miseo/.bin`.
    pub commands: Vec<String>,
    /// End-to-end task duration in milliseconds.
    pub elapsed_ms: u128,
    /// Old variant keys removed after successful switch.
    pub removed_variants: Vec<String>,
}

/// Already-current upgrade details.
pub type AlreadyCurrent = super::install::AlreadyCurrent;

pub fn execute(
    mise: &impl Mise,
    workspace: &mut Workspace,
    tool_id: ToolId,
    uses: RuntimePins,
) -> Result<Outcome, Error> {
    let uses = workspace.upgrade_uses(&tool_id, uses)?;
    let install_mode = workspace.current_install_mode(&tool_id)?;

    let install_outcome = install::execute(
        mise,
        workspace,
        tool_id.clone().into(),
        uses,
        false,
        install_mode,
    )?;

    match install_outcome {
        Ok(success) => {
            debug_assert_eq!(success.action, InstallAction::Installed);
            let removed_variants = workspace.prune_variants_and_cleanup(&tool_id)?;
            Ok(Ok(Success {
                tool_id: success.tool_id,
                package_version: success.package_version,
                runtimes: success.runtimes,
                commands: success.commands,
                elapsed_ms: success.elapsed_ms,
                removed_variants,
            }))
        }
        Err(already_current) => Ok(Err(already_current)),
    }
}
