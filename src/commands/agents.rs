use clap::Subcommand;
use pk_cli_core::CliError;

use super::rooms::HomeFlag;
use super::{emit_list, Ctx};

#[derive(Subcommand, Debug)]
pub enum AgentsCmd {
    /// Partner integrations that own devices, with device counts (agent-list/v1).
    #[command(visible_alias = "ls")]
    List(HomeFlag),
}

pub fn run(ctx: &Ctx, cmd: &AgentsCmd) -> Result<(), CliError> {
    match cmd {
        AgentsCmd::List(flag) => {
            let graph = ctx.graph()?;
            let items = super::agent_counts(ctx, &graph, flag.home.as_deref())?;
            emit_list(ctx.json, "agent", items, &["agent_id", "label", "devices"]);
            Ok(())
        }
    }
}
