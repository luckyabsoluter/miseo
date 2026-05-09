use crate::{error::Error, mise::Mise, spec::ToolId, workspace::Workspace};

/// Successful uninstall details for UI rendering.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// Stable tool identity (`<backend>:<name>`).
    pub tool_id: ToolId,
    /// Public command links removed from `~/.miseo/.bin`.
    pub removed_commands: Vec<String>,
}

pub fn execute(
    mise: &impl Mise,
    workspace: &mut Workspace,
    tool_id: ToolId,
    force: bool,
) -> Result<Outcome, Error> {
    for project_dir in workspace.global_uninstall_project_dirs(&tool_id)? {
        mise.uninstall_global(&tool_id, &project_dir)?;
    }

    let removed_commands = workspace.uninstall(&tool_id, force)?;

    Ok(Outcome {
        tool_id,
        removed_commands,
    })
}
