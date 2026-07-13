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
        /// Reply to the given message id in the same channel.
        #[arg(long = "reply-to")]
        reply_to: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_parses_body_positional() {
        let cli = Cli::try_parse_from(["discord", "send", "123", "hello world"]).unwrap();
        match cli.command {
            Command::Send {
                channel,
                body,
                text,
                reply_to,
            } => {
                assert_eq!(channel, "123");
                assert_eq!(body.as_deref(), Some("hello world"));
                assert_eq!(text, None);
                assert_eq!(reply_to, None);
            }
            other => panic!("expected Send, got {other:?}"),
        }
    }

    #[test]
    fn send_parses_text_flag() {
        let cli = Cli::try_parse_from(["discord", "send", "123", "--text", "hi"]).unwrap();
        match cli.command {
            Command::Send {
                channel,
                body,
                text,
                reply_to,
            } => {
                assert_eq!(channel, "123");
                assert_eq!(body, None);
                assert_eq!(text.as_deref(), Some("hi"));
                assert_eq!(reply_to, None);
            }
            other => panic!("expected Send, got {other:?}"),
        }
    }

    #[test]
    fn send_parses_reply_to_flag() {
        let cli =
            Cli::try_parse_from(["discord", "send", "123", "hello", "--reply-to", "999"]).unwrap();
        match cli.command {
            Command::Send {
                channel,
                body,
                text,
                reply_to,
            } => {
                assert_eq!(channel, "123");
                assert_eq!(body.as_deref(), Some("hello"));
                assert_eq!(text, None);
                assert_eq!(reply_to.as_deref(), Some("999"));
            }
            other => panic!("expected Send, got {other:?}"),
        }
    }

    #[test]
    fn read_uses_default_limit_when_omitted() {
        let cli = Cli::try_parse_from(["discord", "read", "123"]).unwrap();
        match cli.command {
            Command::Read {
                channel,
                after,
                limit,
            } => {
                assert_eq!(channel, "123");
                assert_eq!(after, None);
                assert_eq!(limit, 50);
            }
            other => panic!("expected Read, got {other:?}"),
        }
    }

    #[test]
    fn read_parses_after_and_limit_overrides() {
        let cli = Cli::try_parse_from(["discord", "read", "123", "--after", "99", "--limit", "10"])
            .unwrap();
        match cli.command {
            Command::Read {
                channel,
                after,
                limit,
            } => {
                assert_eq!(channel, "123");
                assert_eq!(after.as_deref(), Some("99"));
                assert_eq!(limit, 10);
            }
            other => panic!("expected Read, got {other:?}"),
        }
    }

    #[test]
    fn wait_uses_default_timeout_interval_and_limit_when_omitted() {
        let cli = Cli::try_parse_from(["discord", "wait", "123"]).unwrap();
        match cli.command {
            Command::Wait {
                channel,
                after,
                timeout,
                interval,
                limit,
            } => {
                assert_eq!(channel, "123");
                assert_eq!(after, None);
                assert_eq!(timeout, 60);
                assert_eq!(interval, 5);
                assert_eq!(limit, 50);
            }
            other => panic!("expected Wait, got {other:?}"),
        }
    }

    #[test]
    fn wait_parses_all_overrides() {
        let cli = Cli::try_parse_from([
            "discord",
            "wait",
            "123",
            "--after",
            "5",
            "--timeout",
            "30",
            "--interval",
            "2",
            "--limit",
            "20",
        ])
        .unwrap();
        match cli.command {
            Command::Wait {
                channel,
                after,
                timeout,
                interval,
                limit,
            } => {
                assert_eq!(channel, "123");
                assert_eq!(after.as_deref(), Some("5"));
                assert_eq!(timeout, 30);
                assert_eq!(interval, 2);
                assert_eq!(limit, 20);
            }
            other => panic!("expected Wait, got {other:?}"),
        }
    }

    #[test]
    fn global_flags_apply_alongside_subcommand() {
        let cli =
            Cli::try_parse_from(["discord", "--human", "--token", "abc", "send", "123", "hi"])
                .unwrap();
        assert!(cli.human);
        assert_eq!(cli.token.as_deref(), Some("abc"));
        assert_eq!(cli.command.name(), "send");
    }

    #[test]
    fn missing_subcommand_is_a_usage_error() {
        assert!(Cli::try_parse_from(["discord"]).is_err());
    }
}
