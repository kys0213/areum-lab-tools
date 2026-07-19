mod cli;
mod commands;
mod common;
mod output;

use std::io::Read;
use std::path::PathBuf;

use clap::Parser;

use cli::{Cli, Command};
use commands::TokioSleeper;
use common::config;
use common::http::HttpDiscordApi;
use output::{AppError, Payload, Sink};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let command_name = cli.command.name();
    let json = cli.json;

    let result = run(cli).await;
    let (sink, line, code) = output::render(command_name, &result, json);
    match sink {
        Sink::Stdout => println!("{line}"),
        Sink::Stderr => eprintln!("{line}"),
    }
    std::process::exit(code);
}

/// Wires config/token resolution to the HTTP client and dispatches to the
/// command handlers. Pure delegation — no business logic lives here.
///
/// `init` is dispatched before token resolution: it is exactly the command
/// that creates a missing token file, so it must not require one to already
/// exist (unlike send/read/wait, which are network calls and do).
async fn run(cli: Cli) -> Result<Payload, AppError> {
    let config_path = match &cli.config {
        Some(path) => PathBuf::from(path),
        None => config::default_config_path()?,
    };

    match cli.command {
        Command::Init { token, force } => commands::run_init(
            &config_path,
            token.as_deref(),
            force,
            read_stdin,
            || config_path.exists(),
            || config::load_config(&config_path),
            |cfg| config::write_config_file(&config_path, cfg),
        ),
        Command::Send {
            channel,
            body,
            text,
            reply_to,
            files,
        } => {
            let (api, channels) = authenticated_api(&config_path, cli.token.as_deref())?;
            let channel_id = config::resolve_channel(&channel, &channels);
            commands::run_send(
                &api,
                &channel_id,
                body.as_deref(),
                text.as_deref(),
                reply_to.as_deref(),
                &files,
                read_stdin,
                |p| std::fs::read(p),
            )
            .await
        }
        Command::Read {
            channel,
            after,
            limit,
        } => {
            let (api, channels) = authenticated_api(&config_path, cli.token.as_deref())?;
            let channel_id = config::resolve_channel(&channel, &channels);
            commands::run_read(&api, &channel_id, after.as_deref(), limit).await
        }
        Command::Wait {
            channel,
            after,
            timeout,
            interval,
            limit,
        } => {
            let (api, channels) = authenticated_api(&config_path, cli.token.as_deref())?;
            let channel_id = config::resolve_channel(&channel, &channels);
            let sleeper = TokioSleeper;
            commands::run_wait(
                &api,
                &sleeper,
                &channel_id,
                after.as_deref(),
                timeout,
                interval,
                limit,
            )
            .await
        }
    }
}

/// Loads config and resolves the bot token (flag > env > config file) for
/// the network-calling commands (send/read/wait). Not used by `init`, which
/// creates the config file rather than reading a token out of it.
fn authenticated_api(
    config_path: &std::path::Path,
    token_flag: Option<&str>,
) -> Result<(HttpDiscordApi, std::collections::HashMap<String, String>), AppError> {
    let cfg = config::load_config(config_path)?.unwrap_or_default();
    let env_token = std::env::var("DISCORD_BOT_TOKEN").ok();
    let token = config::resolve_token(token_flag, env_token.as_deref(), cfg.token.as_deref())?;
    Ok((HttpDiscordApi::new(token), cfg.channels))
}

fn read_stdin() -> std::io::Result<String> {
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer)?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the full `--config` wiring in `run()`: init must create the
    /// file at the override path, not `default_config_path()`. The command
    /// layer only tests `run_init` with an already-resolved path — this is
    /// the one seam that actually derives that path from the CLI flag.
    #[tokio::test]
    async fn init_creates_config_at_config_flag_override_path() {
        let dir = std::env::temp_dir().join(format!(
            "areum-discord-main-init-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("override-config.json");

        let cli = Cli {
            token: None,
            config: Some(path.display().to_string()),
            json: false,
            command: Command::Init {
                token: Some("mytoken".to_owned()),
                force: false,
            },
        };

        let payload = run(cli).await.unwrap();
        match payload {
            Payload::Init(data) => {
                assert_eq!(data.path, path.display().to_string());
                assert!(data.created);
            }
            other => panic!("expected Init, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"token":"mytoken"}"#
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
