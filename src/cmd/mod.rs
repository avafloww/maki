mod acp;
mod migrate;
mod subcmd;
mod tui;

use color_eyre::Result;
use color_eyre::eyre::Context;

use maki_storage::StateDir;

use crate::cli::{AuthAction, Cli, Command, McpAction, MigrateAction};
use crate::update;

/// Seed `plugins.bash.auto_mode` so the loaded bash plugin classifies every
/// command. Call after `into_config`, before `load_builtins`.
pub fn seed_automode(config: &mut maki_config::Config, automode: bool) {
    if automode {
        config
            .plugins
            .opts
            .entry("bash".into())
            .or_default()
            .insert("auto_mode".into(), serde_json::Value::Bool(true));
    }
}

pub fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Some(Command::Auth { action }) => {
            let storage = StateDir::resolve().context("resolve data directory")?;
            match action {
                AuthAction::Login { provider } => {
                    subcmd::auth_login(provider.as_deref(), &storage)?
                }
                AuthAction::Logout { provider } => subcmd::auth_logout(&provider, &storage)?,
                AuthAction::Status => subcmd::auth_status(&storage)?,
            }
        }
        Some(Command::Index { path }) => {
            subcmd::index(&path, cli.no_plugins, cli.no_jit)?;
        }
        Some(Command::Models) => subcmd::models(cli.no_plugins, cli.no_jit)?,
        Some(Command::Mcp { action }) => {
            let storage = StateDir::resolve().context("resolve data directory")?;
            match action {
                McpAction::Auth { server } => subcmd::mcp_auth(&server, &storage)?,
                McpAction::Logout { server } => subcmd::mcp_logout(&server, &storage)?,
            }
        }
        Some(Command::Update { yes, no_color }) => {
            update::update(yes, no_color).map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
        }
        Some(Command::Rollback) => {
            update::rollback().map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
        }
        Some(Command::Acp {
            model,
            yolo,
            automode,
        }) => {
            acp::run(
                model,
                yolo,
                automode,
                cli.no_plugins,
                cli.no_jit,
                cli.system_prompt,
                cli.append_system_prompt,
            )?;
        }
        Some(Command::Migrate { action }) => match action {
            MigrateAction::Xdg => migrate::xdg()?,
        },
        Some(Command::Prompt {
            variant,
            plan,
            tools,
            names,
        }) => {
            subcmd::prompt(&variant, plan, tools, names, cli.no_plugins, cli.no_jit)?;
        }
        None => {
            tui::run(cli)?;
        }
    }
    Ok(())
}
