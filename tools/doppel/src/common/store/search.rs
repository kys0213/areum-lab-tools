//! Precedent search over `golden_fts` (docs/doppel-judgment.md §4). Same
//! database + same query = same hits in the same order: ties on the bm25
//! score break by newest decision, then by id.

use rusqlite::{Row, params};

use super::golden::{GOLDEN_COLUMNS, row_to_raw_golden};
use super::{Store, map_sqlite_err};
use crate::common::error::AppError;
use crate::common::store::GoldenRecord;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SearchFilter<'a> {
    pub(crate) category: Option<&'a str>,
    pub(crate) project: Option<&'a str>,
    pub(crate) include_candidates: bool,
    pub(crate) limit: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SearchHit {
    pub(crate) golden: GoldenRecord,
    /// Negated bm25: higher is a better match. Only comparable within one
    /// query's results.
    pub(crate) score: f64,
}

/// Turns free text into an FTS5 expression: every word becomes a quoted
/// prefix term and the terms are OR-ed. Prefix matching lets a query stem
/// ("저장소") find an indexed word with a particle attached ("저장소에").
/// For the other direction — the query carries the particle ("테스트는") —
/// a Hangul word also contributes itself minus one and two trailing
/// syllables, never shorter than two. `None` when the text has no word.
pub(crate) fn fts_query(text: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        for variant in word_variants(&word.to_lowercase()) {
            let term = format!("\"{variant}\"*");
            if !terms.contains(&term) {
                terms.push(term);
            }
        }
    }
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

/// Korean particles are one or two syllables long.
const MAX_PARTICLE_CHARS: usize = 2;
const MIN_STEM_CHARS: usize = 2;

fn word_variants(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut variants = vec![word.to_owned()];
    if chars.iter().any(|c| is_hangul_syllable(*c)) {
        for drop in 1..=MAX_PARTICLE_CHARS {
            if chars.len() >= MIN_STEM_CHARS + drop {
                variants.push(chars[..chars.len() - drop].iter().collect());
            }
        }
    }
    variants
}

fn is_hangul_syllable(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c)
}

impl Store {
    /// Runs an expression built by [`fts_query`]. Retired entries are never
    /// returned; candidates only when asked for.
    pub(crate) fn search_golden(
        &self,
        expression: &str,
        filter: &SearchFilter<'_>,
    ) -> Result<Vec<SearchHit>, AppError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {GOLDEN_COLUMNS}, bm25(golden_fts) AS rank
                 FROM golden_fts JOIN golden g ON g.id = golden_fts.golden_id
                 WHERE golden_fts MATCH ?1
                   AND (g.state = 'confirmed' OR (?2 = 1 AND g.state = 'candidate'))
                   AND (?3 IS NULL OR g.category = ?3)
                   AND (?4 IS NULL OR g.project = ?4)
                 ORDER BY rank, g.decided_at DESC, g.id
                 LIMIT ?5"
            ))
            .map_err(map_sqlite_err)?;
        let rows = stmt
            .query_map(
                params![
                    expression,
                    filter.include_candidates,
                    filter.category,
                    filter.project,
                    filter.limit
                ],
                row_to_hit,
            )
            .map_err(map_sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_err)?;
        rows.into_iter()
            .map(|(raw, rank)| {
                Ok(SearchHit {
                    golden: raw.decode()?,
                    score: -rank,
                })
            })
            .collect()
    }
}

fn row_to_hit(row: &Row<'_>) -> rusqlite::Result<(super::golden::RawGolden, f64)> {
    Ok((row_to_raw_golden(row)?, row.get(16)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_become_or_joined_prefix_terms() {
        assert_eq!(
            fts_query("전략, Rust!").as_deref(),
            Some(r#""전략"* OR "rust"*"#)
        );
    }

    #[test]
    fn hangul_words_also_try_shorter_stems() {
        assert_eq!(
            fts_query("테스트는").as_deref(),
            Some(r#""테스트는"* OR "테스트"* OR "테스"*"#)
        );
        // Never below two syllables; Latin words get no variants.
        assert_eq!(fts_query("설계").as_deref(), Some(r#""설계"*"#));
        assert_eq!(fts_query("tests").as_deref(), Some(r#""tests"*"#));
    }

    #[test]
    fn repeated_terms_appear_once() {
        assert_eq!(fts_query("캐시 캐시").as_deref(), Some(r#""캐시"*"#));
    }

    #[test]
    fn punctuation_only_text_has_no_query() {
        assert_eq!(fts_query("  ?! -- "), None);
    }

    #[test]
    fn fts_syntax_in_the_input_cannot_escape_quoting() {
        // Quotes and operators are separators, so user text never becomes
        // FTS syntax.
        assert_eq!(
            fts_query(r#"a" OR "b*"#).as_deref(),
            Some(r#""a"* OR "or"* OR "b"*"#)
        );
    }
}
