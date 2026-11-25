//! Masks known secret shapes before text enters the ledger
//! (docs/doppel-judgment.md §1). Deterministic: the same input always
//! masks the same spans. It recognizes shapes, not secrets — anything not
//! listed here passes through.

const MASK: &str = "[REDACTED]";

/// Token prefixes whose remainder is the secret.
const TOKEN_PREFIXES: [&str; 10] = [
    "sk-",
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "xoxa-",
    "xoxb-",
    "xoxp-",
    "xoxr-",
    "AKIA",
];

/// A token body shorter than this is too short to be a credential — `sk-1`
/// in prose stays readable.
const MIN_TOKEN_BODY: usize = 8;

pub(crate) fn redact(text: &str) -> String {
    mask_tokens(&mask_bearer(&mask_pem_blocks(text)))
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Length of the token body starting at `from`.
fn token_len(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .take_while(|b| is_token_byte(**b))
        .count()
}

/// `<prefix><body>` where the prefix starts a word, replaced whole.
fn mask_tokens(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        let at_word_start = i == 0 || !is_token_byte(bytes[i - 1]);
        let matched = at_word_start
            .then(|| {
                TOKEN_PREFIXES.iter().find_map(|prefix| {
                    let body = i + prefix.len();
                    (bytes[i..].starts_with(prefix.as_bytes())
                        && body <= bytes.len()
                        && token_len(bytes, body) >= MIN_TOKEN_BODY)
                        .then(|| body + token_len(bytes, body))
                })
            })
            .flatten();
        match matched {
            Some(end) => {
                out.push_str(&text[copied..i]);
                out.push_str(MASK);
                copied = end;
                i = end;
            }
            None => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// `Bearer <token>` keeps the scheme word and masks the credential.
fn mask_bearer(text: &str) -> String {
    const SCHEME: &str = "Bearer ";
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut search_from = 0;
    while let Some(offset) = text[search_from..].find(SCHEME) {
        let start = search_from + offset;
        let body = start + SCHEME.len();
        let len = bytes[body..]
            .iter()
            .take_while(|b| is_token_byte(**b) || **b == b'.' || **b == b'=')
            .count();
        if len >= MIN_TOKEN_BODY {
            out.push_str(&text[copied..body]);
            out.push_str(MASK);
            copied = body + len;
        }
        search_from = body + len;
    }
    out.push_str(&text[copied..]);
    out
}

/// A PEM private-key block, header to footer, becomes one mask. An unclosed
/// block masks to the end — a truncated key is still a key.
fn mask_pem_blocks(text: &str) -> String {
    const BEGIN: &str = "-----BEGIN ";
    const KEY_MARK: &str = "PRIVATE KEY-----";
    const END: &str = "-----END ";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(BEGIN) {
        let header_end = rest[start..].find('\n').map_or(rest.len(), |n| start + n);
        if !rest[start..header_end].contains(KEY_MARK) {
            out.push_str(&rest[..header_end]);
            rest = &rest[header_end..];
            continue;
        }
        out.push_str(&rest[..start]);
        out.push_str(MASK);
        let block_end = rest[start..]
            .find(END)
            .and_then(|n| {
                let footer = start + n;
                rest[footer..]
                    .find(KEY_MARK)
                    .map(|m| footer + m + KEY_MARK.len())
            })
            .unwrap_or(rest.len());
        rest = &rest[block_end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_token_shapes_are_masked_whole() {
        assert_eq!(
            redact("key sk-ant-api03-abcdEFGH1234 here"),
            "key [REDACTED] here"
        );
        assert_eq!(redact("ghp_0123456789abcdef"), "[REDACTED]");
        assert_eq!(redact("token=xoxb-1234-5678-abcd"), "token=[REDACTED]");
        assert_eq!(redact("AKIAIOSFODNN7EXAMPLE"), "[REDACTED]");
        assert_eq!(redact("github_pat_11ABCDEFG0123456789"), "[REDACTED]");
    }

    #[test]
    fn prefixes_inside_a_word_or_with_short_bodies_pass() {
        assert_eq!(redact("task-runner"), "task-runner");
        assert_eq!(redact("risk-free"), "risk-free");
        assert_eq!(redact("sk-1 is a label"), "sk-1 is a label");
    }

    #[test]
    fn bearer_credentials_keep_the_scheme() {
        assert_eq!(
            redact("Authorization: Bearer eyJhbGciOi.payload.sig"),
            "Authorization: Bearer [REDACTED]"
        );
        assert_eq!(redact("Bearer short"), "Bearer short");
    }

    #[test]
    fn private_key_blocks_become_one_mask() {
        let text = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIE\nabc\n-----END RSA PRIVATE KEY-----\nafter";
        assert_eq!(redact(text), "before\n[REDACTED]\nafter");
    }

    #[test]
    fn unclosed_private_key_masks_to_the_end() {
        assert_eq!(
            redact("x -----BEGIN PRIVATE KEY-----\nMIIE"),
            "x [REDACTED]"
        );
    }

    #[test]
    fn public_certificates_are_not_keys() {
        let text = "-----BEGIN CERTIFICATE-----\nMIIC\n-----END CERTIFICATE-----";
        assert_eq!(redact(text), text);
    }

    #[test]
    fn non_ascii_text_is_untouched() {
        assert_eq!(redact("한국어 설명 — sk 없음"), "한국어 설명 — sk 없음");
    }
}
