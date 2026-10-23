//! Character-safe truncation. Ledger text is mostly Korean, so cutting by
//! bytes would split a syllable.

/// The first `max` characters.
pub(crate) fn head_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// The last `max` characters — the part of a long context closest to the
/// question that follows it.
pub(crate) fn tail_chars(text: &str, max: usize) -> String {
    let total = text.chars().count();
    text.chars().skip(total.saturating_sub(max)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(head_chars("가나다라", 2), "가나");
        assert_eq!(tail_chars("가나다라", 2), "다라");
        assert_eq!(head_chars("ab", 5), "ab");
        assert_eq!(tail_chars("ab", 5), "ab");
    }
}
