use crate::output::{AppError, ErrorKind};

const FENCE: &str = "```";

/// Splits `content` into chunks of at most `max` codepoints, preferring line
/// boundaries and hard-cutting only lines that cannot fit on their own. A code
/// fence open at a cut is closed at the end of the chunk and reopened (same
/// info string) at the start of the next, with both markers counted against
/// `max`. An unterminated fence in the source is left unterminated.
pub(super) fn split_message(content: &str, max: usize) -> Result<Vec<String>, AppError> {
    let mut chunker = Chunker::new(max);
    let mut lines = content.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        chunker.push_line(line, lines.peek().is_none())?;
    }
    Ok(chunker.finish())
}

struct Chunker {
    max: usize,
    chunks: Vec<String>,
    cur: String,
    cur_len: usize,
    /// Whether `cur` holds source text, not just a reopening fence prefix.
    has_content: bool,
    /// Info string of the fence open at the end of `cur`, if any.
    fence: Option<String>,
}

impl Chunker {
    fn new(max: usize) -> Self {
        Self {
            max,
            chunks: Vec::new(),
            cur: String::new(),
            cur_len: 0,
            has_content: false,
            fence: None,
        }
    }

    fn push_line(&mut self, line: &str, is_last: bool) -> Result<(), AppError> {
        let after = next_fence(&self.fence, line);
        if self.fits(line, &after, is_last) {
            self.append(line, after);
            return Ok(());
        }
        if self.has_content {
            self.flush()?;
        }
        if self.fits(line, &after, is_last) {
            self.append(line, after);
            return Ok(());
        }
        self.force_split(line, after, is_last)
    }

    fn fits(&self, text: &str, after: &Option<String>, is_last: bool) -> bool {
        let reserve = if is_last {
            0
        } else {
            closing_len(after.is_some(), text.ends_with('\n'))
        };
        self.cur_len + text.chars().count() + reserve <= self.max
    }

    fn append(&mut self, text: &str, after: Option<String>) {
        self.cur.push_str(text);
        self.cur_len += text.chars().count();
        self.has_content = true;
        self.fence = after;
    }

    /// Closes the open fence (if any), stores the chunk, and starts the next
    /// one with the fence reopened.
    fn flush(&mut self) -> Result<(), AppError> {
        if self.fence.is_some() {
            self.cur.push_str(if self.cur.ends_with('\n') {
                FENCE
            } else {
                "\n```"
            });
        }
        self.chunks.push(std::mem::take(&mut self.cur));
        self.cur = match &self.fence {
            Some(info) => format!("{FENCE}{info}\n"),
            None => String::new(),
        };
        self.cur_len = self.cur.chars().count();
        self.has_content = false;
        // Reopen prefix + worst-case closing must leave room for >= 1 char,
        // otherwise force-splitting could never make progress.
        if self.fence.is_some() && self.cur_len + closing_len(true, false) >= self.max {
            return Err(AppError::new(
                ErrorKind::Usage,
                "code fence info string is too long to reopen the fence in a split message",
            ));
        }
        Ok(())
    }

    /// Cuts a line that does not fit even in an empty chunk. The fence state
    /// stays what it was before the line until the final piece lands.
    fn force_split(
        &mut self,
        line: &str,
        after: Option<String>,
        is_last: bool,
    ) -> Result<(), AppError> {
        let mut rest = line;
        loop {
            if self.fits(rest, &after, is_last) {
                self.append(rest, after);
                return Ok(());
            }
            let budget = self.max - self.cur_len - closing_len(self.fence.is_some(), false);
            let cut = rest
                .char_indices()
                .nth(budget)
                .map_or(rest.len(), |(idx, _)| idx);
            let (piece, remainder) = rest.split_at(cut);
            let state = self.fence.clone();
            self.append(piece, state);
            self.flush()?;
            rest = remainder;
        }
    }

    fn finish(mut self) -> Vec<String> {
        if self.has_content || self.chunks.is_empty() {
            self.chunks.push(self.cur);
        }
        self.chunks
    }
}

/// Fence state after `line`: a fence line toggles it, anything else keeps it.
fn next_fence(state: &Option<String>, line: &str) -> Option<String> {
    let trimmed = line.trim();
    match trimmed.strip_prefix(FENCE) {
        Some(_) if state.is_some() => None,
        Some(info) => Some(info.trim().to_owned()),
        None => state.clone(),
    }
}

/// Codepoints needed to close a fence at the end of a chunk.
fn closing_len(fence_open: bool, ends_with_newline: bool) -> usize {
    match (fence_open, ends_with_newline) {
        (false, _) => 0,
        (true, true) => FENCE.len(),
        (true, false) => FENCE.len() + 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: usize = 2000;

    fn chars(s: &str) -> usize {
        s.chars().count()
    }

    #[test]
    fn short_content_is_a_single_chunk() {
        assert_eq!(
            split_message("hello\nworld", MAX).unwrap(),
            ["hello\nworld"]
        );
    }

    #[test]
    fn exactly_max_chars_is_a_single_chunk() {
        let text = "a".repeat(MAX);
        assert_eq!(split_message(&text, MAX).unwrap(), [text]);
    }

    #[test]
    fn empty_content_yields_one_empty_chunk() {
        assert_eq!(split_message("", MAX).unwrap(), [""]);
    }

    #[test]
    fn prefers_line_boundaries() {
        let line = format!("{}\n", "a".repeat(999));
        let text = line.repeat(3);
        let chunks = split_message(&text, MAX).unwrap();
        assert_eq!(chunks, [line.repeat(2), line]);
    }

    #[test]
    fn oversized_line_is_force_split_without_losing_chars() {
        let text = "가".repeat(4500);
        let chunks = split_message(&text, MAX).unwrap();
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| chars(c) <= MAX));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn code_block_is_closed_and_reopened_with_same_language() {
        let body = "let x = 1;\n".repeat(300);
        let text = format!("intro\n```rust\n{body}```\noutro\n");
        let chunks = split_message(&text, MAX).unwrap();
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|c| chars(c) <= MAX));
        assert!(chunks[0].ends_with("\n```"));
        assert!(chunks[1].starts_with("```rust\n"));
        for c in &chunks {
            let fences = c.lines().filter(|l| l.starts_with("```")).count();
            assert_eq!(fences % 2, 0, "unbalanced fences in chunk: {c}");
        }
        let first_body = chunks[0].strip_suffix("```").unwrap();
        let rest: String = chunks[1..].concat();
        assert!(first_body.starts_with("intro\n```rust\n"));
        assert!(rest.ends_with("```\noutro\n"));
    }

    #[test]
    fn force_split_line_inside_code_block_keeps_every_chunk_fenced() {
        let text = format!("```py\n{}\n```\n", "x".repeat(4500));
        let chunks = split_message(&text, MAX).unwrap();
        assert!(chunks.iter().all(|c| chars(c) <= MAX));
        for c in &chunks {
            assert!(c.starts_with("```py\n"), "{c:.20}");
            assert!(c.trim_end().ends_with("```"));
        }
    }

    #[test]
    fn unterminated_fence_that_fits_is_left_alone() {
        let text = "```sh\nls";
        assert_eq!(split_message(text, MAX).unwrap(), [text]);
    }

    #[test]
    fn rejects_fence_info_string_too_long_to_reopen() {
        let text = format!("```{}\n{}", "l".repeat(MAX), "x\n".repeat(MAX));
        assert!(split_message(&text, MAX).is_err());
    }
}
