mod api;
mod cli;
mod command;
mod config;
mod http;
mod output;

use std::io::Read;
use std::path::PathBuf;

use clap::Parser;

use cli::{Cli, Command};
use command::TokioSleeper;
use http::HttpDiscordApi;
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
async fn run(cli: Cli) -> Result<Payload, AppError> {
    let config_path = match &cli.config {
        Some(path) => PathBuf::from(path),
        None => config::default_config_path()?,
    };
    let cfg = config::load_config(&config_path)?.unwrap_or_default();

    let env_token = std::env::var("DISCORD_BOT_TOKEN").ok();
    let token = config::resolve_token(
        cli.token.as_deref(),
        env_token.as_deref(),
        cfg.token.as_deref(),
    )?;
    let api = HttpDiscordApi::new(token);

    match cli.command {
        Command::Send {
            channel,
            body,
            text,
            reply_to,
            files,
        } => {
            let channel_id = config::resolve_channel(&channel, &cfg.channels);
            command::run_send(
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
            let channel_id = config::resolve_channel(&channel, &cfg.channels);
            command::run_read(&api, &channel_id, after.as_deref(), limit).await
        }
        Command::Wait {
            channel,
            after,
            timeout,
            interval,
            limit,
        } => {
            let channel_id = config::resolve_channel(&channel, &cfg.channels);
            let sleeper = TokioSleeper;
            command::run_wait(
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

fn read_stdin() -> std::io::Result<String> {
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer)?;
    Ok(buffer)
}
