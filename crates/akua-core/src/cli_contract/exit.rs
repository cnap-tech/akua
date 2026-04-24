//! Typed exit codes per [cli-contract §2](../../../../docs/cli-contract.md#2-exit-codes).
//!
//! Seven stable codes. Verbs do not invent their own. Any other exit code
//! from an `akua` verb is a bug.

use std::fmt;

use serde::{Deserialize, Serialize};

crate::contract_type! {
/// The seven typed exit codes every akua verb may produce.
///
/// Agents branch on these. Humans read them. The stability contract is
/// that meanings never change; new codes require a major version bump.
///
/// Serialized as the stable kebab-case name (`"success"`, `"user-error"`,
/// `"policy-deny"`, etc.) — same string [`ExitCode::name`] returns. The
/// SDK maps from the numeric `child.exitCode` (0..=6) to this name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum ExitCode {
    /// `0` — operation completed as requested.
    Success = 0,

    /// `1` — invalid inputs, bad flags, missing required arguments.
    /// Something the caller got wrong.
    UserError = 1,

    /// `2` — unexpected failure (disk, network, bug). Something akua
    /// couldn't recover from that isn't the caller's fault.
    SystemError = 2,

    /// `3` — the policy engine rejected the operation.
    PolicyDeny = 3,

    /// `4` — registry / API rate limits. Retry with backoff.
    RateLimited = 4,

    /// `5` — the operation is allowed but requires human approval
    /// before it can proceed.
    NeedsApproval = 5,

    /// `6` — operation did not complete within `--timeout`.
    Timeout = 6,
}
}

impl ExitCode {
    /// Numeric code suitable for `std::process::exit`.
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// Stable, machine-readable name (lowercase with hyphens).
    pub const fn name(self) -> &'static str {
        match self {
            ExitCode::Success => "success",
            ExitCode::UserError => "user-error",
            ExitCode::SystemError => "system-error",
            ExitCode::PolicyDeny => "policy-deny",
            ExitCode::RateLimited => "rate-limited",
            ExitCode::NeedsApproval => "needs-approval",
            ExitCode::Timeout => "timeout",
        }
    }

    /// Try to construct from a raw `i32`. Rejects values outside 0..=6.
    pub const fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(ExitCode::Success),
            1 => Some(ExitCode::UserError),
            2 => Some(ExitCode::SystemError),
            3 => Some(ExitCode::PolicyDeny),
            4 => Some(ExitCode::RateLimited),
            5 => Some(ExitCode::NeedsApproval),
            6 => Some(ExitCode::Timeout),
            _ => None,
        }
    }

    /// `true` when this code means the operation did not complete
    /// successfully and should not be treated as a win.
    pub const fn is_failure(self) -> bool {
        !matches!(self, ExitCode::Success)
    }

    /// `true` when retrying with the same inputs might succeed without
    /// caller intervention — rate-limited and timeout. `needs-approval`
    /// is not retriable; it requires a human to approve before a retry
    /// will produce a different outcome.
    pub const fn is_retriable(self) -> bool {
        matches!(self, ExitCode::RateLimited | ExitCode::Timeout)
    }
}

impl fmt::Display for ExitCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl From<ExitCode> for i32 {
    fn from(code: ExitCode) -> i32 {
        code.code()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_contract() {
        assert_eq!(ExitCode::Success.code(), 0);
        assert_eq!(ExitCode::UserError.code(), 1);
        assert_eq!(ExitCode::SystemError.code(), 2);
        assert_eq!(ExitCode::PolicyDeny.code(), 3);
        assert_eq!(ExitCode::RateLimited.code(), 4);
        assert_eq!(ExitCode::NeedsApproval.code(), 5);
        assert_eq!(ExitCode::Timeout.code(), 6);
    }

    #[test]
    fn round_trips_from_code() {
        for code in 0..=6 {
            let ec = ExitCode::from_code(code).expect("valid code");
            assert_eq!(ec.code(), code);
        }
        assert!(ExitCode::from_code(7).is_none());
        assert!(ExitCode::from_code(-1).is_none());
        assert!(ExitCode::from_code(100).is_none());
    }

    #[test]
    fn names_are_stable_and_distinct() {
        use std::collections::HashSet;
        let codes = [
            ExitCode::Success,
            ExitCode::UserError,
            ExitCode::SystemError,
            ExitCode::PolicyDeny,
            ExitCode::RateLimited,
            ExitCode::NeedsApproval,
            ExitCode::Timeout,
        ];
        let names: HashSet<_> = codes.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), codes.len(), "names must be distinct");
        // Spot-check two to anchor the stability contract.
        assert_eq!(ExitCode::PolicyDeny.name(), "policy-deny");
        assert_eq!(ExitCode::NeedsApproval.name(), "needs-approval");
    }

    #[test]
    fn is_failure_flags_non_success_codes() {
        assert!(!ExitCode::Success.is_failure());
        assert!(ExitCode::UserError.is_failure());
        assert!(ExitCode::SystemError.is_failure());
        assert!(ExitCode::PolicyDeny.is_failure());
        assert!(ExitCode::RateLimited.is_failure());
        assert!(ExitCode::NeedsApproval.is_failure());
        assert!(ExitCode::Timeout.is_failure());
    }

    #[test]
    fn is_retriable_flags_transient_failures_only() {
        assert!(!ExitCode::Success.is_retriable());
        assert!(!ExitCode::UserError.is_retriable());
        assert!(!ExitCode::SystemError.is_retriable());
        assert!(!ExitCode::PolicyDeny.is_retriable());
        assert!(ExitCode::RateLimited.is_retriable());
        assert!(!ExitCode::NeedsApproval.is_retriable());
        assert!(ExitCode::Timeout.is_retriable());
    }

    #[test]
    fn display_uses_stable_name() {
        assert_eq!(ExitCode::PolicyDeny.to_string(), "policy-deny");
    }

    #[test]
    fn into_i32_works() {
        let n: i32 = ExitCode::NeedsApproval.into();
        assert_eq!(n, 5);
    }
}
