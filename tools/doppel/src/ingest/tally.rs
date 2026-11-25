use std::collections::BTreeMap;

/// Records an ingester could not use, counted by reason. Ingest keeps going
/// past an unreadable record but must say so — this is what it says.
/// `BTreeMap` keeps the reasons in a stable order for output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SkipTally {
    counts: BTreeMap<&'static str, usize>,
}

impl SkipTally {
    pub(crate) fn add(&mut self, reason: &'static str) {
        *self.counts.entry(reason).or_default() += 1;
    }

    pub(crate) fn merge(&mut self, other: SkipTally) {
        for (reason, count) in other.counts {
            *self.counts.entry(reason).or_default() += count;
        }
    }

    #[cfg(test)]
    pub(crate) fn get(&self, reason: &str) -> usize {
        self.counts.get(reason).copied().unwrap_or(0)
    }

    pub(crate) fn into_map(self) -> BTreeMap<String, usize> {
        self.counts
            .into_iter()
            .map(|(reason, count)| (reason.to_owned(), count))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_accumulate_and_merge_by_reason() {
        let mut a = SkipTally::default();
        a.add("invalid_json");
        a.add("invalid_json");
        let mut b = SkipTally::default();
        b.add("invalid_json");
        b.add("unreadable_file");
        a.merge(b);
        assert_eq!(a.get("invalid_json"), 3);
        assert_eq!(a.get("unreadable_file"), 1);
        assert_eq!(a.get("other"), 0);
        let map = a.into_map();
        assert_eq!(
            map.keys().collect::<Vec<_>>(),
            ["invalid_json", "unreadable_file"]
        );
    }
}
