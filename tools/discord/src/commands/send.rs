use crate::common::api::{DiscordApi, FilePart, SendRequest};
use crate::output::{AppError, ErrorKind, Payload, SendData};

/// Sends a message (optionally with file attachments) to a channel and reports
/// the created message.
///
/// Over clippy's arg ceiling by one: the extra parameters are the two I/O
/// seams (`read_stdin`, `read_file`) kept injectable for black-box tests
/// rather than reaching for `std` directly — bundling them into a struct would
/// obscure that intent.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_send(
    api: &impl DiscordApi,
    channel_id: &str,
    body: Option<&str>,
    text: Option<&str>,
    reply_to: Option<&str>,
    file_paths: &[String],
    read_stdin: impl FnOnce() -> std::io::Result<String>,
    read_file: impl Fn(&str) -> std::io::Result<Vec<u8>>,
) -> Result<Payload, AppError> {
    // An explicit empty --reply-to is a caller mistake, not "no reply": reject
    // it rather than sending message_reference with an empty message_id.
    if reply_to == Some("") {
        return Err(AppError::new(
            ErrorKind::Usage,
            "--reply-to must not be an empty string",
        ));
    }
    let files = resolve_files(file_paths, read_file)?;
    let content = resolve_send_content(body, text, !files.is_empty(), read_stdin)?;
    let req = SendRequest {
        channel_id: channel_id.to_owned(),
        content,
        reply_to: reply_to.map(str::to_owned),
        files,
    };
    let sent = api.send_message(&req).await?;
    Ok(Payload::Send(SendData {
        message_id: sent.id,
        channel_id: sent.channel_id,
        timestamp: sent.timestamp,
        attachments: sent.attachments,
    }))
}

/// Discord rejects empty messages and caps content at 2000 codepoints.
const MAX_BODY_CHARS: usize = 2000;

/// Discord allows at most 10 attachments per message.
const MAX_FILES: usize = 10;

/// Files are read fully into memory before upload, so this bounds how much a
/// single file can pull into RAM before Discord's own 413 would reject it. It
/// is a sanity ceiling, not a spec match — it aligns with Discord's highest
/// boost-tier per-file limit (100 MiB).
const MAX_FILE_BYTES: usize = 100 * 1024 * 1024;

/// Resolves the outgoing caption. BODY vs --text stay mutually exclusive and
/// the 2000-codepoint cap always applies. Emptiness handling depends on files:
/// with attachments present an empty caption is allowed and stdin is NOT read;
/// without attachments the pre-file behaviour is unchanged (empty rejected,
/// stdin read when BODY is `None` or `"-"`).
fn resolve_send_content(
    body: Option<&str>,
    text: Option<&str>,
    has_files: bool,
    read_stdin: impl FnOnce() -> std::io::Result<String>,
) -> Result<String, AppError> {
    if body.is_some() && text.is_some() {
        return Err(AppError::new(
            ErrorKind::Usage,
            "BODY and --text are mutually exclusive; pass only one",
        ));
    }

    let content = match (body, text) {
        (_, Some(text)) => text.to_owned(),
        (Some("-"), None) => read_stdin_to_string(read_stdin)?,
        // Files present with no explicit caption: send an empty caption rather
        // than blocking on stdin (the agent isn't piping one in).
        (None, None) if has_files => String::new(),
        (None, None) => read_stdin_to_string(read_stdin)?,
        (Some(body), None) => body.to_owned(),
    };

    validate_body(&content, has_files)?;
    Ok(content)
}

/// Reads stdin to a string, mapping I/O failure to an internal error. Shared
/// with `init`'s token resolution, which reads stdin the same way.
pub(crate) fn read_stdin_to_string(
    read_stdin: impl FnOnce() -> std::io::Result<String>,
) -> Result<String, AppError> {
    read_stdin()
        .map_err(|e| AppError::new(ErrorKind::Internal, format!("failed to read stdin: {e}")))
}

fn validate_body(content: &str, has_files: bool) -> Result<(), AppError> {
    if content.is_empty() && !has_files {
        return Err(AppError::new(
            ErrorKind::Usage,
            "message body must not be empty",
        ));
    }
    let len = content.chars().count();
    if len > MAX_BODY_CHARS {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!("message body exceeds {MAX_BODY_CHARS} characters (got {len})"),
        ));
    }
    Ok(())
}

/// Reads and validates each attachment path into a [`FilePart`]. All failures
/// map to usage errors (exit 2) with the offending path in the message, since
/// they are caller-side mistakes caught before any network call.
fn resolve_files(
    file_paths: &[String],
    read_file: impl Fn(&str) -> std::io::Result<Vec<u8>>,
) -> Result<Vec<FilePart>, AppError> {
    if file_paths.len() > MAX_FILES {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!(
                "at most {MAX_FILES} files per message (got {})",
                file_paths.len()
            ),
        ));
    }
    file_paths
        .iter()
        .map(|path| resolve_file(path, &read_file))
        .collect()
}

fn resolve_file(
    path: &str,
    read_file: impl Fn(&str) -> std::io::Result<Vec<u8>>,
) -> Result<FilePart, AppError> {
    let bytes = read_file(path)
        .map_err(|e| AppError::new(ErrorKind::Usage, format!("cannot read file '{path}': {e}")))?;
    if bytes.is_empty() {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!("file '{path}' is empty; nothing to upload"),
        ));
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!(
                "file '{path}' exceeds {MAX_FILE_BYTES} bytes (got {})",
                bytes.len()
            ),
        ));
    }
    let filename = file_name_of(path);
    let content_type = mime_guess::from_path(&filename)
        .first_or_octet_stream()
        .to_string();
    Ok(FilePart {
        filename,
        bytes: bytes::Bytes::from(bytes),
        content_type,
    })
}

/// The last path component — Discord (and other viewers) should see the file's
/// name, not the caller's full local path.
fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

#[cfg(test)]
mod tests;
