//! Readers that turn past records into ledger entries
//! (docs/doppel-judgment.md §1). Pure: each parser takes text and answers
//! entries plus a tally of what it skipped; walking the filesystem is the
//! command layer's job.

pub(crate) mod plan_doc;
pub(crate) mod redact;
pub(crate) mod tally;
pub(crate) mod text;
pub(crate) mod transcript;
