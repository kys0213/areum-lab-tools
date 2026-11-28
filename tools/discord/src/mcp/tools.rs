use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::commands;
use crate::common::api::DiscordApi;
use crate::common::error::{AppError, ErrorKind};
use crate::output::Payload;

use super::channel_access::resolve_allowed_channel;
use super::request_headers::RequestHeaders;

/// Same default as the CLI's `read --limit`.
const DEFAULT_READ_LIMIT: u8 = 50;

const CHANNEL_DESCRIPTION: &str =
    "Channel or thread id, or an alias from the X-Discord-Channels header.";

/// A tool as advertised in `tools/list`.
pub(super) struct ToolSpec {
    pub(super) name: &'static str,
    pub(super) description: &'static str,
    pub(super) input_schema: Value,
}

pub(super) fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "send",
            description: "Send a text message (max 2000 characters) to a channel or thread, optionally as a reply.",
            input_schema: object_schema(
                json!({
                    "channel": { "type": "string", "description": CHANNEL_DESCRIPTION },
                    "text": { "type": "string", "description": "Message body." },
                    "reply_to": { "type": "string", "description": "Id of a message in the same channel to reply to." }
                }),
                &["channel", "text"],
            ),
        },
        ToolSpec {
            name: "read",
            description: "Read recent messages, oldest first. Pass the returned cursor as `after` to read only newer ones.",
            input_schema: object_schema(
                json!({
                    "channel": { "type": "string", "description": CHANNEL_DESCRIPTION },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Max messages (default 50)." },
                    "after": { "type": "string", "description": "Only messages newer than this message id." }
                }),
                &["channel"],
            ),
        },
        ToolSpec {
            name: "react",
            description: "Add the bot's emoji reaction to a message, or remove it with remove=true.",
            input_schema: object_schema(
                json!({
                    "channel": { "type": "string", "description": CHANNEL_DESCRIPTION },
                    "message_id": { "type": "string" },
                    "emoji": { "type": "string", "description": "A unicode emoji, or a custom emoji as name:id." },
                    "remove": { "type": "boolean", "description": "Remove instead of add (default false)." }
                }),
                &["channel", "message_id", "emoji"],
            ),
        },
        ToolSpec {
            name: "thread_create",
            description: "Create a thread in a channel, or on an existing message with from_message.",
            input_schema: object_schema(
                json!({
                    "channel": { "type": "string", "description": CHANNEL_DESCRIPTION },
                    "name": { "type": "string" },
                    "from_message": { "type": "string", "description": "Message id to start the thread from." }
                }),
                &["channel", "name"],
            ),
        },
    ]
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArgs {
    channel: String,
    text: String,
    reply_to: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    channel: String,
    limit: Option<u8>,
    after: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReactArgs {
    channel: String,
    message_id: String,
    emoji: String,
    #[serde(default)]
    remove: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThreadCreateArgs {
    channel: String,
    name: String,
    from_message: Option<String>,
}

/// Runs one tool call through the same command functions the CLI uses, so
/// the result is the CLI's `--json` envelope `data`.
pub(super) async fn call_tool(
    api: &impl DiscordApi,
    headers: &RequestHeaders,
    name: &str,
    arguments: Map<String, Value>,
) -> Result<Payload, AppError> {
    match name {
        "send" => {
            let args: SendArgs = parse_args(name, arguments)?;
            let channel_id = resolve_allowed_channel(api, headers, &args.channel).await?;
            // Text goes through the `--text` path, which never reads stdin.
            // No `files`: a path would be read from the server's disk on a
            // remote caller's behalf.
            commands::run_send(
                api,
                &channel_id,
                None,
                Some(&args.text),
                args.reply_to.as_deref(),
                &[],
                no_stdin,
                no_files,
            )
            .await
        }
        "read" => {
            let args: ReadArgs = parse_args(name, arguments)?;
            let channel_id = resolve_allowed_channel(api, headers, &args.channel).await?;
            let limit = args.limit.unwrap_or(DEFAULT_READ_LIMIT);
            commands::run_read(api, &channel_id, args.after.as_deref(), limit).await
        }
        "react" => {
            let args: ReactArgs = parse_args(name, arguments)?;
            let channel_id = resolve_allowed_channel(api, headers, &args.channel).await?;
            commands::run_react(api, &channel_id, &args.message_id, &args.emoji, args.remove).await
        }
        "thread_create" => {
            let args: ThreadCreateArgs = parse_args(name, arguments)?;
            let channel_id = resolve_allowed_channel(api, headers, &args.channel).await?;
            commands::run_thread_create(api, &channel_id, &args.name, args.from_message.as_deref())
                .await
        }
        other => Err(AppError::new(
            ErrorKind::Usage,
            format!("unknown tool `{other}`"),
        )),
    }
}

fn parse_args<T: DeserializeOwned>(
    tool: &str,
    arguments: Map<String, Value>,
) -> Result<T, AppError> {
    serde_json::from_value(Value::Object(arguments))
        .map_err(|e| AppError::new(ErrorKind::Usage, format!("invalid {tool} arguments: {e}")))
}

fn no_stdin() -> std::io::Result<String> {
    Err(std::io::Error::other("the MCP server has no stdin"))
}

fn no_files(_: &str) -> std::io::Result<Vec<u8>> {
    Err(std::io::Error::other(
        "the MCP server does not read local files",
    ))
}

#[cfg(test)]
mod tests;
