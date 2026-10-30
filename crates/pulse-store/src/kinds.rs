//! Job kinds: the words a job carries in its `kind` column, declared once.
//!
//! Every job-kind literal in the tree resolves through the table here: the
//! `const fn kind()` names a known kind and returns its [`KindSpec`], so the
//! compiler refuses an unknown word instead of it drifting through `&str`.
//! [`JOB_KINDS`] re-exports the names for the validators that enumerate them.
//!
//! Phase B (see the handoff on agents/agent-pack-followups) is two PRs. This
//! is the first: one table behind a `const fn` with the same literals, and a
//! test (`cargo test -p pulse-tasks kinds`) that every job-kind literal in
//! the tree is in the table. The second PR adds the loader that reads extra
//! or overridden kinds from `.pulse/kinds/*.yaml` (a path
//! [`crate::OWNER_PATHS`] already reserves for the owner) so a pack can name
//! a new kind without a Rust change.

/// What one job kind declares about the work it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindSpec {
    /// The word in the `jobs.kind` column and every brief, e.g. `"improve"`.
    pub name: &'static str,
    /// True when the job lands a change (task/improve/experiment): it gets
    /// the heavy worker model and the wider step budget. Read-only kinds
    /// (answer/ballot) answer or vote without touching the clone. The rest
    /// run the light model with the ordinary harness tools.
    pub implements: bool,
    /// True when the job must not change the clone: the harness runs it with
    /// `--read-only` (no edit or write tool, mutating commands refused).
    pub read_only: bool,
    /// One line of what the job is for, shown by `pulse agents`-style help.
    pub about: &'static str,
}

/// Every job kind the daemon runs, one row per `jobs.kind` value.
pub const KIND_SPECS: &[KindSpec] = &[
    KindSpec {
        name: "improve",
        implements: true,
        read_only: false,
        about: "make one measured change against the project check",
    },
    KindSpec {
        name: "task",
        implements: true,
        read_only: false,
        about: "work an owner-approved task",
    },
    KindSpec {
        name: "experiment",
        implements: true,
        read_only: false,
        about: "run one experiment card and measure its check",
    },
    KindSpec {
        name: "reflect",
        implements: false,
        read_only: false,
        about: "turn recent runs into lessons, skills and records",
    },
    KindSpec {
        name: "lead",
        implements: false,
        read_only: false,
        about: "coordinate the project: board, ballot, blockers",
    },
    KindSpec {
        name: "answer",
        implements: false,
        read_only: true,
        about: "answer an owner question without touching the clone",
    },
    KindSpec {
        name: "ballot",
        implements: false,
        read_only: true,
        about: "cast the project's votes on the open ballot",
    },
    KindSpec {
        name: "baseline",
        implements: false,
        read_only: false,
        about: "record the project's starting measurements",
    },
    KindSpec {
        name: "onboard",
        implements: false,
        read_only: false,
        about: "first run on a fresh project: map it and set up",
    },
    KindSpec {
        name: "clean",
        implements: false,
        read_only: false,
        about: "remove what the runs left behind",
    },
];

/// The job-kind names, kept in lockstep with [`KIND_SPECS`] by the unit test
/// below so the validators that enumerate names keep working unchanged.
pub const JOB_KINDS: &[&str] = &[
    "improve",
    "task",
    "experiment",
    "reflect",
    "lead",
    "answer",
    "ballot",
    "baseline",
    "onboard",
    "clean",
];

/// The spec for a known kind, or `None` for a word the table does not name.
/// `const` so a match on a literal kind compiles it away entirely.
pub const fn kind(name: &str) -> Option<KindSpec> {
    let mut i = 0;
    while i < KIND_SPECS.len() {
        let spec = &KIND_SPECS[i];
        if str_eq(spec.name, name) {
            return Some(*spec);
        }
        i += 1;
    }
    None
}

/// True when `name` is a job kind the table knows.
pub const fn is_kind(name: &str) -> bool {
    kind(name).is_some()
}

/// True when the kind lands a change (heavy model, wider step budget).
pub const fn kind_implements(name: &str) -> bool {
    match kind(name) {
        Some(spec) => spec.implements,
        None => false,
    }
}

/// True when the kind runs with the read-only harness.
pub const fn kind_is_read_only(name: &str) -> bool {
    match kind(name) {
        Some(spec) => spec.read_only,
        None => false,
    }
}

/// Byte-wise `str` equality usable in a `const fn`.
const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_kinds_mirrors_kind_specs() {
        let spec_names: Vec<&str> = KIND_SPECS.iter().map(|s| s.name).collect();
        assert_eq!(spec_names, JOB_KINDS, "JOB_KINDS must mirror KIND_SPECS");
    }

    #[test]
    fn every_name_resolves_through_kind() {
        for name in JOB_KINDS {
            let spec = kind(name).unwrap_or_else(|| panic!("unknown kind {name}"));
            assert_eq!(spec.name, *name);
        }
        assert!(kind("nope").is_none());
        assert!(!is_kind(""));
    }

    #[test]
    fn the_kind_classes_match_the_settings_lists() {
        for spec in KIND_SPECS {
            assert_eq!(
                spec.implements,
                crate::IMPLEMENTATION_JOB_KINDS.contains(&spec.name),
                "{}: implements",
                spec.name
            );
            assert_eq!(
                spec.read_only,
                crate::READ_ONLY_JOB_KINDS.contains(&spec.name),
                "{}: read_only",
                spec.name
            );
        }
        for name in crate::IMPLEMENTATION_JOB_KINDS {
            assert!(
                kind(name).is_some(),
                "implementation kind {name} has no spec"
            );
        }
        for name in crate::READ_ONLY_JOB_KINDS {
            assert!(kind(name).is_some(), "read-only kind {name} has no spec");
        }
        for spec in KIND_SPECS {
            assert!(
                !(spec.implements && spec.read_only),
                "{}: a kind cannot both implement and be read-only",
                spec.name
            );
        }
    }
}
