//! Message detection: decides whether a gateway `MESSAGE_CREATE` should fire
//! the configured `on_message` command, and builds the JSON line handed to it
//! on stdin. The daemon knows nothing about what the command does — it only
//! guarantees the filter rules and the payload contract (see
//! `docs/discord-message-trigger.md`).
//!
//! Filter rules: bots (including this bot) never trigger; inside a thread only
//! a bot mention triggers; a top-level post in an issue channel triggers even
//! without a mention (once, as `issue_channel`); anywhere else only a bot
//! mention triggers.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::common::api::{ChannelInfo, DiscordApi};
use crate::common::config::{Config, resolve_channel};
use crate::common::error::{AppError, ErrorKind};

use super::hook_runner::HookRunner;
use super::workdir::Workdirs;

/// Discord message types a person produces by posting (`DEFAULT`, `REPLY`).
/// System messages (thread created, pins, joins) never trigger.
const USER_MESSAGE_TYPES: [u8; 2] = [0, 19];

/// Channel types that are threads (announcement, public, private).
const THREAD_CHANNEL_TYPES: [u8; 3] = [10, 11, 12];

/// Upper bound for the REST channel lookup. The lookup runs on the gateway
/// event loop, so a slow Discord response must not stall every other event.
const CHANNEL_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Validated message-trigger settings derived from config. Exists only when
/// `on_message` is configured — its absence is how the feature is disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageTriggerSettings {
    issue_channels: HashSet<String>,
    on_message: Vec<String>,
    workdirs: Workdirs,
}

impl MessageTriggerSettings {
    /// `Ok(None)` when `on_message` is absent (feature off). Issue-channel
    /// aliases resolve through `channels` the same way `send`/`read` do. An
    /// empty `on_message` array, or no working directory setting at all,
    /// cannot be executed, so both fail fast.
    pub(crate) fn from_config(config: &Config) -> Result<Option<Self>, AppError> {
        let Some(on_message) = &config.on_message else {
            return Ok(None);
        };
        if on_message.is_empty() {
            return Err(AppError::new(
                ErrorKind::Config,
                "on_message must be a non-empty argv array, e.g. [\"/path/to/command\"]",
            ));
        }
        let issue_channels = config
            .issue_channels
            .iter()
            .map(|entry| resolve_channel(entry, &config.channels))
            .collect();
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let workdirs = Workdirs::from_config(config, home.as_deref())?;
        Ok(Some(Self {
            issue_channels,
            on_message: on_message.clone(),
            workdirs,
        }))
    }
}

/// Why a message fired the hook — the `trigger` field of the stdin JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Trigger {
    Mention,
    IssueChannel,
}

/// Where a message lives: a top-level channel or a thread under `parent_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Placement {
    TopLevel,
    Thread { parent_id: String },
}

/// The subset of a Discord message object (gateway `MESSAGE_CREATE`) the
/// trigger reads. Field names are Discord's wire names.
#[derive(Debug, Deserialize)]
struct IncomingMessage {
    id: String,
    channel_id: String,
    guild_id: String,
    #[serde(rename = "type")]
    kind: u8,
    author: IncomingAuthor,
    content: String,
    timestamp: String,
    #[serde(default)]
    mentions: Vec<MentionedUser>,
    #[serde(default)]
    message_reference: Option<IncomingReference>,
}

#[derive(Debug, Deserialize)]
struct IncomingAuthor {
    id: String,
    username: String,
    #[serde(default)]
    bot: bool,
}

#[derive(Debug, Deserialize)]
struct MentionedUser {
    id: String,
}

#[derive(Debug, Deserialize)]
struct IncomingReference {
    #[serde(default)]
    message_id: Option<String>,
}

/// The stdin JSON contract for `on_message`. Field names are fixed by the
/// consumer contract — do not rename.
#[derive(Debug, Serialize)]
struct TriggerEvent<'a> {
    trigger: Trigger,
    guild_id: &'a str,
    channel_id: &'a str,
    parent_channel_id: Option<&'a str>,
    is_thread: bool,
    message_id: &'a str,
    content: &'a str,
    author: EventAuthor<'a>,
    timestamp: &'a str,
    bot_user_id: &'a str,
    message_reference: Option<&'a str>,
    /// Canonical absolute directory the hook was started in.
    cwd: &'a str,
}

#[derive(Debug, Serialize)]
struct EventAuthor<'a> {
    id: &'a str,
    username: &'a str,
    bot: bool,
}

/// Stateful handler the gateway loop owns: settings, the hook runner, the bot
/// user id learned from READY, and a channel-placement cache (a channel's
/// thread-ness and parent never change, so successful lookups are kept for
/// the daemon's lifetime).
pub(super) struct MessageTrigger<R: HookRunner> {
    settings: MessageTriggerSettings,
    runner: R,
    bot_user_id: Option<String>,
    placements: HashMap<String, Placement>,
}

impl<R: HookRunner> MessageTrigger<R> {
    pub(super) fn new(settings: MessageTriggerSettings, runner: R) -> Self {
        Self {
            settings,
            runner,
            bot_user_id: None,
            placements: HashMap::new(),
        }
    }

    pub(super) fn set_bot_user_id(&mut self, id: String) {
        self.bot_user_id = Some(id);
    }

    /// Handles one raw `MESSAGE_CREATE` payload. `Ok(Some(trigger))` means the
    /// hook was started; `Ok(None)` means the filter dropped the message;
    /// `Err` means the message could not be judged or the hook could not be
    /// started (the caller logs it and keeps serving).
    pub(super) async fn handle(
        &mut self,
        api: &impl DiscordApi,
        payload: &serde_json::Value,
    ) -> Result<Option<Trigger>, AppError> {
        let bot_user_id = self.bot_user_id.as_deref().ok_or_else(|| {
            AppError::new(
                ErrorKind::Internal,
                "MESSAGE_CREATE arrived before READY; bot user id is unknown",
            )
        })?;
        let message: IncomingMessage = serde_json::from_value(payload.clone()).map_err(|e| {
            AppError::new(
                ErrorKind::Api,
                format!("malformed MESSAGE_CREATE payload: {e}"),
            )
        })?;
        let issue_channels = &self.settings.issue_channels;
        // A thread only ever publishes on a mention, which would also publish
        // at top level — so a message that fails the top-level rule fails
        // both, and the REST lookup can be skipped.
        if decide(&message, bot_user_id, &Placement::TopLevel, issue_channels).is_none() {
            return Ok(None);
        }
        let placement = match self.placements.get(&message.channel_id) {
            Some(cached) => cached.clone(),
            None => {
                let info = tokio::time::timeout(
                    CHANNEL_LOOKUP_TIMEOUT,
                    api.get_channel(&message.channel_id),
                )
                .await
                .map_err(|_| {
                    AppError::new(
                        ErrorKind::Network,
                        format!(
                            "channel lookup for {} timed out after {CHANNEL_LOOKUP_TIMEOUT:?}, \
                             message {} not published",
                            message.channel_id, message.id
                        ),
                    )
                })?
                .map_err(|e| {
                    AppError::new(
                        e.kind,
                        format!(
                            "channel lookup for {} failed, message {} not published: {}",
                            message.channel_id, message.id, e.message
                        ),
                    )
                })?;
                let placement = placement_of(&info)?;
                self.placements
                    .insert(message.channel_id.clone(), placement.clone());
                placement
            }
        };
        let Some(trigger) = decide(&message, bot_user_id, &placement, issue_channels) else {
            return Ok(None);
        };
        let parent_id = match &placement {
            Placement::TopLevel => None,
            Placement::Thread { parent_id } => Some(parent_id.as_str()),
        };
        let cwd = self
            .settings
            .workdirs
            .resolve(&message.channel_id, parent_id)
            .map_err(|e| {
                AppError::new(
                    e.kind,
                    format!("message {} not published: {}", message.id, e.message),
                )
            })?;
        let line = event_line(&message, trigger, &placement, bot_user_id, &cwd);
        self.runner
            .run(&self.settings.on_message, Path::new(&cwd), line)?;
        Ok(Some(trigger))
    }
}

/// The filter table. Pure: given the message, the bot's id, and where the
/// message lives, returns the trigger to publish (if any).
fn decide(
    message: &IncomingMessage,
    bot_user_id: &str,
    placement: &Placement,
    issue_channels: &HashSet<String>,
) -> Option<Trigger> {
    if message.author.bot || message.author.id == bot_user_id {
        return None;
    }
    if !USER_MESSAGE_TYPES.contains(&message.kind) {
        return None;
    }
    // Only a direct user mention counts; role and @everyone mentions live in
    // other fields (`mention_roles`, `mention_everyone`) and are ignored.
    let mentioned = message.mentions.iter().any(|m| m.id == bot_user_id);
    match placement {
        Placement::Thread { .. } => mentioned.then_some(Trigger::Mention),
        Placement::TopLevel if issue_channels.contains(&message.channel_id) => {
            Some(Trigger::IssueChannel)
        }
        Placement::TopLevel => mentioned.then_some(Trigger::Mention),
    }
}

/// Classifies a channel. A thread without `parent_id` violates Discord's
/// channel contract, so it is an error rather than a guessed top level.
fn placement_of(info: &ChannelInfo) -> Result<Placement, AppError> {
    if !THREAD_CHANNEL_TYPES.contains(&info.kind) {
        return Ok(Placement::TopLevel);
    }
    match &info.parent_id {
        Some(parent_id) => Ok(Placement::Thread {
            parent_id: parent_id.clone(),
        }),
        None => Err(AppError::new(
            ErrorKind::Api,
            format!(
                "thread channel {} (type {}) has no parent_id",
                info.id, info.kind
            ),
        )),
    }
}

/// Serializes the stdin contract as exactly one line terminated by `\n`.
fn event_line(
    message: &IncomingMessage,
    trigger: Trigger,
    placement: &Placement,
    bot_user_id: &str,
    cwd: &str,
) -> String {
    let parent_channel_id = match placement {
        Placement::TopLevel => None,
        Placement::Thread { parent_id } => Some(parent_id.as_str()),
    };
    let event = TriggerEvent {
        trigger,
        guild_id: &message.guild_id,
        channel_id: &message.channel_id,
        parent_channel_id,
        is_thread: parent_channel_id.is_some(),
        message_id: &message.id,
        content: &message.content,
        author: EventAuthor {
            id: &message.author.id,
            username: &message.author.username,
            bot: message.author.bot,
        },
        timestamp: &message.timestamp,
        bot_user_id,
        message_reference: message
            .message_reference
            .as_ref()
            .and_then(|r| r.message_id.as_deref()),
        cwd,
    };
    // serde_json escapes control characters, so the output never contains a
    // raw newline and stays a single line.
    let mut line = serde_json::to_string(&event)
        .expect("TriggerEvent holds only strings and bools; serialization is infallible");
    line.push('\n');
    line
}

#[cfg(test)]
mod tests;
