/// Error kinds whose wire name and exit code are the same in every tool.
///
/// A tool's own `ErrorKind` stays a flat enum in the tool and delegates these
/// variants here, so a shared kind cannot drift to a different number in one
/// tool. Tool-specific kinds pick their own codes, never reusing one listed
/// here except as a deliberate alias (discord's `auth` shares `config`'s 3).
/// Codes already taken by tool-specific kinds: 4 `api` and 5 `rate_limit`
/// (discord, kanban). A new tool-specific kind starts at 9; existing codes are
/// never renumbered because agents branch on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommonKind {
    Internal,
    Usage,
    Config,
    Network,
    NotFound,
    Conflict,
}

impl CommonKind {
    /// Wire name in the JSON `error.kind` and the human `error [<kind>]` text.
    pub const fn name(self) -> &'static str {
        match self {
            CommonKind::Internal => "internal",
            CommonKind::Usage => "usage",
            CommonKind::Config => "config",
            CommonKind::Network => "network",
            CommonKind::NotFound => "not_found",
            CommonKind::Conflict => "conflict",
        }
    }

    pub const fn exit_code(self) -> i32 {
        match self {
            CommonKind::Internal => 1,
            CommonKind::Usage => 2,
            CommonKind::Config => 3,
            CommonKind::Network => 6,
            CommonKind::NotFound => 7,
            CommonKind::Conflict => 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_keeps_its_published_name_and_exit_code() {
        let table = [
            (CommonKind::Internal, "internal", 1),
            (CommonKind::Usage, "usage", 2),
            (CommonKind::Config, "config", 3),
            (CommonKind::Network, "network", 6),
            (CommonKind::NotFound, "not_found", 7),
            (CommonKind::Conflict, "conflict", 8),
        ];
        for (kind, name, code) in table {
            assert_eq!(kind.name(), name, "{kind:?}");
            assert_eq!(kind.exit_code(), code, "{kind:?}");
        }
    }
}
