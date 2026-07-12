use clap::{Parser, Subcommand};

/// Discord bot CLI for AI agents: send, read, and wait for channel messages.
///
/// Output is JSON by default (machine-parseable). Pass --human for text.
/// Token resolution order: --token flag > env DISCORD_BOT_TOKEN > config file.
#[derive(Parser, Debug)]
#[command(name = "discord", version, about)]
pub struct Cli {
    /// Bot token. Overrides DISCORD_BOT_TOKEN and the config file.
    #[arg(long, global = true)]
    pub token: Option<String>,

    /// Config path override (default: ~/.areum/discord/config.json).
    #[arg(long, global = true)]
    pub config: Option<String>,

    /// Human-readable text output (default is JSON).
    #[arg(long, global = true)]
    pub human: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Send a message to a channel (id or config alias).
    ///
    /// BODY is the message text. Omit BODY or pass '-' to read the whole
    /// message from stdin. --text is mutually exclusive with BODY (validated
    /// in-app, not by clap).
    Send {
        /// Channel id, or an alias defined in config.channels.
        channel: String,
        /// Message body. Omit or '-' to read stdin. Exclusive with --text.
        body: Option<String>,
        /// Message text. Exclusive with the BODY positional argument.
        #[arg(long)]
        text: Option<String>,
    },

    /// Read recent messages from a channel (id or config alias).
    Read {
        /// Channel id, or an alias defined in config.channels.
        channel: String,
        /// Only return messages after this message id.
        #[arg(long)]
        after: Option<String>,
        /// Max messages to return (default 50, Discord max 100).
        #[arg(long, default_value_t = 50)]
        limit: u8,
    },

    /// Poll a channel until a new message arrives or the timeout elapses.
    ///
    /// Exits 0 on timeout with data.timed_out = true (a timeout is normal).
    Wait {
        /// Channel id, or an alias defined in config.channels.
        channel: String,
        /// Only consider messages after this message id.
        #[arg(long)]
        after: Option<String>,
        /// Total seconds to wait before giving up (default 60).
        #[arg(long, default_value_t = 60)]
        timeout: u64,
        /// Seconds between polls (default 5).
        #[arg(long, default_value_t = 5)]
        interval: u64,
        /// Max messages to return per poll (default 50, Discord max 100).
        #[arg(long, default_value_t = 50)]
        limit: u8,
    },
}

impl Command {
    /// Stable command name used as the `command` field in the JSON envelope.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Send { .. } => "send",
            Command::Read { .. } => "read",
            Command::Wait { .. } => "wait",
        }
    }
}
