use std::collections::{HashMap, HashSet};

use axum::http::HeaderMap;

use crate::common::error::{AppError, ErrorKind};

const CHANNELS_HEADER: &str = "x-discord-channels";
const ALLOWED_CHANNELS_HEADER: &str = "x-discord-allowed-channels";

/// Per-request identity and channel settings, read from HTTP headers. The
/// server holds no token of its own: every request names the bot it acts as,
/// so one server can front several bots.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct RequestHeaders {
    pub(super) token: String,
    /// `X-Discord-Channels`: alias → channel id.
    pub(super) channels: HashMap<String, String>,
    /// `X-Discord-Allowed-Channels`: `None` means every channel is allowed.
    pub(super) allowed_channels: Option<HashSet<String>>,
}

// Hand-written so a stray `{:?}` can never print the token.
impl std::fmt::Debug for RequestHeaders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestHeaders")
            .field("token", &"<redacted>")
            .field("channels", &self.channels)
            .field("allowed_channels", &self.allowed_channels)
            .finish()
    }
}

/// Parses the request headers. Error messages name the header and the rule it
/// broke, never the header value — `Authorization` carries the bot token.
pub(super) fn parse_request_headers(headers: &HeaderMap) -> Result<RequestHeaders, AppError> {
    Ok(RequestHeaders {
        token: parse_bot_token(headers)?,
        channels: parse_channel_aliases(header_str(headers, CHANNELS_HEADER)?)?,
        allowed_channels: header_str(headers, ALLOWED_CHANNELS_HEADER)?
            .map(parse_allowed_channels)
            .transpose()?,
    })
}

fn parse_bot_token(headers: &HeaderMap) -> Result<String, AppError> {
    let value = header_str(headers, "authorization")?.ok_or_else(|| {
        auth_error("missing Authorization header (expected `Authorization: Bot <token>`)")
    })?;
    let invalid = || auth_error("Authorization header must be `Bot <token>`");
    let (scheme, token) = value.trim().split_once(' ').ok_or_else(invalid)?;
    let token = token.trim();
    if !scheme.eq_ignore_ascii_case("bot")
        || token.is_empty()
        || token.contains(char::is_whitespace)
    {
        return Err(invalid());
    }
    Ok(token.to_owned())
}

/// `alias=id,alias=id`. Empty items (e.g. a trailing comma) are skipped; an
/// item without `=` or with an empty side is rejected rather than guessed at.
fn parse_channel_aliases(value: Option<&str>) -> Result<HashMap<String, String>, AppError> {
    let mut channels = HashMap::new();
    for item in list_items(value.unwrap_or_default()) {
        let (alias, id) = item
            .split_once('=')
            .map(|(a, i)| (a.trim(), i.trim()))
            .filter(|(a, i)| !a.is_empty() && !i.is_empty())
            .ok_or_else(|| {
                usage_error(format!(
                    "X-Discord-Channels entry `{item}` must be `alias=channel_id`"
                ))
            })?;
        channels.insert(alias.to_owned(), id.to_owned());
    }
    Ok(channels)
}

/// `id,id`. A header that is present but lists nothing is rejected: an empty
/// allow-list would silently block every call.
fn parse_allowed_channels(value: &str) -> Result<HashSet<String>, AppError> {
    let ids: HashSet<String> = list_items(value).map(str::to_owned).collect();
    if ids.is_empty() {
        return Err(usage_error(
            "X-Discord-Allowed-Channels is present but lists no channel id",
        ));
    }
    Ok(ids)
}

fn list_items(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').map(str::trim).filter(|s| !s.is_empty())
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, AppError> {
    headers
        .get(name)
        .map(|v| {
            v.to_str()
                .map_err(|_| usage_error(format!("{name} header must be visible ASCII")))
        })
        .transpose()
}

fn auth_error(message: &str) -> AppError {
    AppError::new(ErrorKind::Auth, message)
}

fn usage_error(message: impl Into<String>) -> AppError {
    AppError::new(ErrorKind::Usage, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "MTIz.secret-token-value";

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse().unwrap());
        }
        map
    }

    #[test]
    fn bot_token_only_parses_with_empty_channel_settings() {
        let parsed =
            parse_request_headers(&headers(&[("authorization", &format!("Bot {SECRET}"))]))
                .unwrap();
        assert_eq!(parsed.token, SECRET);
        assert!(parsed.channels.is_empty());
        assert_eq!(parsed.allowed_channels, None);
    }

    #[test]
    fn channel_aliases_and_allow_list_parse_with_spaces_and_trailing_comma() {
        let parsed = parse_request_headers(&headers(&[
            ("authorization", "bot abc"),
            ("x-discord-channels", " dev = 111 , ops=222,"),
            ("x-discord-allowed-channels", "111, 333"),
        ]))
        .unwrap();
        assert_eq!(
            parsed.channels,
            HashMap::from([
                ("dev".to_owned(), "111".to_owned()),
                ("ops".to_owned(), "222".to_owned()),
            ])
        );
        assert_eq!(
            parsed.allowed_channels,
            Some(HashSet::from(["111".to_owned(), "333".to_owned()]))
        );
    }

    #[test]
    fn missing_or_malformed_authorization_is_an_auth_error_without_the_value() {
        let cases = [
            headers(&[]),
            headers(&[("authorization", SECRET)]),
            headers(&[("authorization", &format!("Bearer {SECRET}"))]),
            headers(&[("authorization", "Bot ")]),
            headers(&[("authorization", &format!("Bot {SECRET} extra"))]),
        ];
        for case in cases {
            let err = parse_request_headers(&case).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Auth, "{case:?}");
            assert!(!err.message.contains("secret-token"), "{}", err.message);
        }
    }

    #[test]
    fn malformed_alias_entry_is_a_usage_error() {
        for value in ["dev", "dev=", "=111"] {
            let err = parse_request_headers(&headers(&[
                ("authorization", "Bot abc"),
                ("x-discord-channels", value),
            ]))
            .unwrap_err();
            assert_eq!(err.kind, ErrorKind::Usage, "{value}");
        }
    }

    #[test]
    fn present_but_empty_allow_list_is_rejected() {
        let err = parse_request_headers(&headers(&[
            ("authorization", "Bot abc"),
            ("x-discord-allowed-channels", " , "),
        ]))
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
    }

    #[test]
    fn debug_output_redacts_the_token() {
        let parsed =
            parse_request_headers(&headers(&[("authorization", &format!("Bot {SECRET}"))]))
                .unwrap();
        assert!(!format!("{parsed:?}").contains(SECRET));
    }
}
