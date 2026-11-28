//! Message detection: decides whether a gateway `MESSAGE_CREATE` should fire,
//! then hands the message to each configured target — the `on_message`
//! command (JSON line on stdin) and/or an Orca bot (`on_message_orca`). The
//! daemon knows nothing about what a target does — it only guarantees the
//! filter rules and the payload contract (see
//! `docs/discord-message-trigger.md`).
//!
//! Filter rules: bots (including this bot) never trigger unless listed in
//! `trigger_bots`, in which case they are judged like a person; inside a thread a
//! bot mention triggers, and so does any message in a thread that already has
//! an Orca chat (`follow_up`, forwarded to Orca only); a top-level post in an
//! issue channel triggers even without a mention (once, as `issue_channel`);
//! anywhere else only a bot mention triggers.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::common::api::{ChannelInfo, DiscordApi};
use crate::common::config::{Config, resolve_channel};
use crate::common::error::{AppError, ErrorKind};
use crate::common::store::AskStore;

use super::hook_runner::HookRunner;
use super::orca_cli::OrcaCli;
use super::orca_forward::{ForwardedMessage, OrcaForward};
use super::workdir::Workdirs;

/// Discord message types a person produces by posting (`DEFAULT`, `REPLY`).
/// System messages (thread created, pins, joins) never trigger.
const USER_MESSAGE_TYPES: [u8; 2] = [0, 19];

/// Channel types that are threads (announcement, public, private).
const THREAD_CHANNEL_TYPES: [u8; 3] = [10, 11, 12];

/// Upper bound for the REST channel lookup. The lookup runs on the gateway
/// event loop, so a slow Discord response must not stall every other event.
const CHANNEL_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Upper bound for the ack reaction call. It runs on the gateway event loop
/// after every target has started, so it only delays the next event.
const ACK_REACTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Validated message-trigger settings derived from config. Exists only when
/// `on_message` or `on_message_orca` is configured — absence of both is how
/// the feature is disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageTriggerSettings {
    issue_channels: HashSet<String>,
    trigger_bots: HashSet<String>,
    hook: Option<HookTarget>,
    orca: Option<OrcaForward>,
    ack_reaction: Option<String>,
}

/// `on_message` argv and where it runs. Workdirs belong to the hook only.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HookTarget {
    argv: Vec<String>,
    workdirs: Workdirs,
}

impl MessageTriggerSettings {
    /// `Ok(None)` when neither `on_message` nor `on_message_orca` is set
    /// (feature off). Issue-channel aliases resolve through `channels` the
    /// same way `send`/`read` do. An empty `on_message` array, an
    /// `on_message` without any working directory setting, or an invalid
    /// `on_message_orca` cannot be executed, so each fails fast, as does a
    /// `trigger_bots` entry that is not a numeric user id.
    pub(crate) fn from_config(config: &Config) -> Result<Option<Self>, AppError> {
        if config.on_message.is_none() && config.on_message_orca.is_none() {
            return Ok(None);
        }
        if let Some(bad) = config
            .trigger_bots
            .iter()
            .find(|id| id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(AppError::new(
                ErrorKind::Config,
                format!("trigger_bots entries must be numeric bot user ids, got {bad:?}"),
            ));
        }
        if config
            .ack_reaction
            .as_deref()
            .is_some_and(|emoji| emoji.trim().is_empty())
        {
            return Err(AppError::new(
                ErrorKind::Config,
                "ack_reaction must be an emoji (e.g. \"👀\") or omitted",
            ));
        }
        let issue_channels = config
            .issue_channels
            .iter()
            .map(|entry| resolve_channel(entry, &config.channels))
            .collect();
        let hook = config
            .on_message
            .as_deref()
            .map(|argv| HookTarget::from_config(argv, config))
            .transpose()?;
        let orca = config
            .on_message_orca
            .as_ref()
            .map(|orca| OrcaForward::from_config(orca, &config.channels))
            .transpose()?;
        Ok(Some(Self {
            issue_channels,
            trigger_bots: config.trigger_bots.iter().cloned().collect(),
            hook,
            orca,
            ack_reaction: config.ack_reaction.clone(),
        }))
    }
}

impl HookTarget {
    fn from_config(argv: &[String], config: &Config) -> Result<Self, AppError> {
        if argv.is_empty() {
            return Err(AppError::new(
                ErrorKind::Config,
                "on_message must be a non-empty argv array, e.g. [\"/path/to/command\"]",
            ));
        }
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let workdirs = Workdirs::from_config(config, home.as_deref())?;
        Ok(Self {
            argv: argv.to_vec(),
            workdirs,
        })
    }
}

/// Why a message fired the hook — the `trigger` field of the stdin JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Trigger {
    Mention,
    IssueChannel,
    /// A message without a mention in a thread that already has an Orca
    /// chat. Only the Orca forward sees it; `on_message` never runs for it.
    FollowUp,
}

impl Trigger {
    /// Short Korean label for the Orca chat title.
    fn label(self) -> &'static str {
        match self {
            Trigger::Mention => "멘션",
            Trigger::IssueChannel => "이슈 채널",
            Trigger::FollowUp => "후속",
        }
    }
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
    message_url: &'a str,
    content: &'a str,
    author: EventAuthor<'a>,
    timestamp: &'a str,
    bot_user_id: &'a str,
    message_reference: Option<&'a str>,
    /// Canonical absolute directory the hook was started in; `null` in the
    /// copy forwarded to Orca, which runs no command in a workdir.
    cwd: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct EventAuthor<'a> {
    id: &'a str,
    username: &'a str,
    bot: bool,
}

/// Stateful handler the gateway loop owns: settings, the hook runner, the
/// orca CLI, the store holding thread -> Orca chat mappings, the bot user id
/// learned from READY, and a channel-placement cache (a channel's
/// thread-ness and parent never change, so successful lookups are kept for
/// the daemon's lifetime).
pub(super) struct MessageTrigger<'s, R: HookRunner, O: OrcaCli> {
    settings: MessageTriggerSettings,
    runner: R,
    orca_cli: O,
    store: &'s AskStore,
    bot_user_id: Option<String>,
    placements: HashMap<String, Placement>,
}

impl<'s, R: HookRunner, O: OrcaCli> MessageTrigger<'s, R, O> {
    pub(super) fn new(
        settings: MessageTriggerSettings,
        runner: R,
        orca_cli: O,
        store: &'s AskStore,
    ) -> Self {
        Self {
            settings,
            runner,
            orca_cli,
            store,
            bot_user_id: None,
            placements: HashMap::new(),
        }
    }

    pub(super) fn set_bot_user_id(&mut self, id: String) {
        self.bot_user_id = Some(id);
    }

    /// Handles one raw `MESSAGE_CREATE` payload. `Ok(Some(trigger))` means
    /// every configured target was started; `Ok(None)` means the filter
    /// dropped the message; `Err` means the message could not be judged or at
    /// least one target could not be started (the caller logs it and keeps
    /// serving). Targets are independent: one failing never skips the other.
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
        let trigger_bots = &self.settings.trigger_bots;
        // A thread publishes on a mention, which would also publish at top
        // level, or as a follow-up in a thread with an Orca chat — so a
        // message that fails the top-level rule and has no chat fails both,
        // and the REST lookup can be skipped.
        let top_level = decide(
            &message,
            bot_user_id,
            &Placement::TopLevel,
            issue_channels,
            trigger_bots,
            false,
        );
        let in_conversation = top_level.is_none() && self.has_orca_chat(&message.channel_id)?;
        if top_level.is_none() && !in_conversation {
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
        let Some(trigger) = decide(
            &message,
            bot_user_id,
            &placement,
            issue_channels,
            trigger_bots,
            in_conversation,
        ) else {
            return Ok(None);
        };
        let failures: Vec<String> = [
            self.run_hook(&message, trigger, &placement, bot_user_id),
            self.forward_to_orca(&message, trigger, &placement, bot_user_id)
                .await,
        ]
        .into_iter()
        .filter_map(Result::err)
        .collect();
        self.acknowledge(api, &message).await;
        if failures.is_empty() {
            Ok(Some(trigger))
        } else {
            Err(AppError::new(
                ErrorKind::Internal,
                format!("message {}: {}", message.id, failures.join("; ")),
            ))
        }
    }

    /// Whether `channel_id` is a thread with a recorded Orca chat. Without
    /// `on_message_orca` there is no conversation to continue.
    fn has_orca_chat(&self, channel_id: &str) -> Result<bool, AppError> {
        if self.settings.orca.is_none() {
            return Ok(false);
        }
        Ok(self.store.thread_chat(channel_id)?.is_some())
    }

    /// Reacts with `ack_reaction` so the author sees the message was picked
    /// up. Runs after the targets started, so it never delays them; it is a
    /// courtesy, so a failure or timeout is one log line and no retry.
    async fn acknowledge(&self, api: &impl DiscordApi, message: &IncomingMessage) {
        let Some(emoji) = &self.settings.ack_reaction else {
            return;
        };
        let result = tokio::time::timeout(
            ACK_REACTION_TIMEOUT,
            api.add_reaction(&message.channel_id, &message.id, emoji),
        )
        .await;
        let reason = match result {
            Ok(Ok(())) => return,
            Ok(Err(e)) => e.to_human(),
            Err(_) => format!("timed out after {ACK_REACTION_TIMEOUT:?}"),
        };
        eprintln!(
            "daemon: ack reaction on message {} failed: {reason}",
            message.id
        );
    }

    /// `Err` carries a one-line reason naming the target.
    fn run_hook(
        &self,
        message: &IncomingMessage,
        trigger: Trigger,
        placement: &Placement,
        bot_user_id: &str,
    ) -> Result<(), String> {
        let Some(hook) = &self.settings.hook else {
            return Ok(());
        };
        if trigger == Trigger::FollowUp {
            return Ok(());
        }
        let not_started = |e: AppError| format!("on_message not started: {}", e.message);
        let cwd = hook
            .workdirs
            .resolve(&message.channel_id, parent_id_of(placement))
            .map_err(not_started)?;
        let mut line = event_json(message, trigger, placement, bot_user_id, Some(&cwd));
        line.push('\n');
        self.runner
            .run(&hook.argv, Path::new(&cwd), line)
            .map_err(not_started)
    }

    /// `Err` carries a one-line reason naming the target.
    async fn forward_to_orca(
        &self,
        message: &IncomingMessage,
        trigger: Trigger,
        placement: &Placement,
        bot_user_id: &str,
    ) -> Result<(), String> {
        let Some(orca) = &self.settings.orca else {
            return Ok(());
        };
        let event_json = event_json(message, trigger, placement, bot_user_id, None);
        let message_url = message_url(message);
        let forwarded = ForwardedMessage {
            trigger_label: trigger.label(),
            author_username: &message.author.username,
            channel_id: &message.channel_id,
            parent_channel_id: parent_id_of(placement),
            message_id: &message.id,
            message_url: &message_url,
            event_json: &event_json,
        };
        orca.forward(&self.orca_cli, self.store, &forwarded)
            .await
            .map_err(|e| format!("on_message_orca failed: {}", e.message))
    }
}

fn parent_id_of(placement: &Placement) -> Option<&str> {
    match placement {
        Placement::TopLevel => None,
        Placement::Thread { parent_id } => Some(parent_id.as_str()),
    }
}

fn message_url(message: &IncomingMessage) -> String {
    format!(
        "https://discord.com/channels/{}/{}/{}",
        message.guild_id, message.channel_id, message.id
    )
}

/// The filter table. Pure: given the message, the bot's id, where the
/// message lives, and whether its thread has an Orca chat, returns the
/// trigger to publish (if any).
fn decide(
    message: &IncomingMessage,
    bot_user_id: &str,
    placement: &Placement,
    issue_channels: &HashSet<String>,
    trigger_bots: &HashSet<String>,
    in_conversation: bool,
) -> Option<Trigger> {
    // A bot (this one included) is judged like a person only when listed.
    let is_bot = message.author.bot || message.author.id == bot_user_id;
    if is_bot && !trigger_bots.contains(&message.author.id) {
        return None;
    }
    if !USER_MESSAGE_TYPES.contains(&message.kind) {
        return None;
    }
    // Only a direct user mention counts; role and @everyone mentions live in
    // other fields (`mention_roles`, `mention_everyone`) and are ignored.
    let mentioned = message.mentions.iter().any(|m| m.id == bot_user_id);
    match placement {
        Placement::Thread { .. } if mentioned => Some(Trigger::Mention),
        Placement::Thread { .. } => in_conversation.then_some(Trigger::FollowUp),
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

/// Serializes the payload contract as one JSON line (no trailing newline).
fn event_json(
    message: &IncomingMessage,
    trigger: Trigger,
    placement: &Placement,
    bot_user_id: &str,
    cwd: Option<&str>,
) -> String {
    let parent_channel_id = parent_id_of(placement);
    let message_url = message_url(message);
    let event = TriggerEvent {
        trigger,
        guild_id: &message.guild_id,
        channel_id: &message.channel_id,
        parent_channel_id,
        is_thread: parent_channel_id.is_some(),
        message_id: &message.id,
        message_url: &message_url,
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
    serde_json::to_string(&event)
        .expect("TriggerEvent holds only strings and bools; serialization is infallible")
}

#[cfg(test)]
mod tests;
