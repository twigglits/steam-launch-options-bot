//! Closed vocabulary of machine-readable codes for the --json output mode.
//!
//! Clients switch on these constants. Every code may be accompanied by a
//! human-readable `message`, but the message is for display only: it is not
//! stable and must never be parsed. Adding a code here is additive; renaming
//! one is a wire-format break and needs a `jsonio::VERSION` bump.

/// Run-level guardrails: why a whole invocation declined to act. Each names the
/// condition ("steam-running"), never the consequence ("cannot-write").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guardrail {
    SteamRunning,
    NoSteamRoot,
    ConfigInvalid,
    NoSystemdSession,
    LegacyInstallShadowing,
}

impl Guardrail {
    pub fn as_str(&self) -> &'static str {
        match self {
            Guardrail::SteamRunning => "steam-running",
            Guardrail::NoSteamRoot => "no-steam-root",
            Guardrail::ConfigInvalid => "config-invalid",
            Guardrail::NoSystemdSession => "no-systemd-session",
            Guardrail::LegacyInstallShadowing => "legacy-install-shadowing",
        }
    }
}

/// Change-level actions: what happened to one (user, appid) pair. These mirror
/// `apply::Change.action` exactly, plus `Excluded`, which the planner never
/// produces because excluded games are dropped before planning - the JSON layer
/// re-introduces them so a client can show the exclusion is being honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Set,
    SkipUserSet,
    SkipUnchanged,
    Excluded,
}

impl Action {
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::Set => "set",
            Action::SkipUserSet => "skip-user-set",
            Action::SkipUnchanged => "skip-unchanged",
            Action::Excluded => "excluded",
        }
    }

    /// Every action, in the order the `counts` object is keyed. A client reads
    /// counts for actions that did not occur as zero rather than as absent, so
    /// the full set is always emitted.
    pub const ALL: [Action; 4] = [
        Action::Set,
        Action::SkipUserSet,
        Action::SkipUnchanged,
        Action::Excluded,
    ];
}

/// Run outcomes, carried on the final `result` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Ran to completion; see counts for what it did.
    Ok,
    /// A guardrail declined the write; exit status is still 0.
    Blocked,
    /// Something actually went wrong.
    Error,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Blocked => "blocked",
            Outcome::Error => "error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_strings_match_the_wire_contract() {
        let strings: Vec<&str> = Action::ALL.iter().map(|a| a.as_str()).collect();
        assert_eq!(
            strings,
            vec!["set", "skip-user-set", "skip-unchanged", "excluded"]
        );
    }

    #[test]
    fn guardrail_strings_match_the_wire_contract() {
        assert_eq!(Guardrail::SteamRunning.as_str(), "steam-running");
        assert_eq!(Guardrail::NoSteamRoot.as_str(), "no-steam-root");
        assert_eq!(Guardrail::ConfigInvalid.as_str(), "config-invalid");
        assert_eq!(Guardrail::NoSystemdSession.as_str(), "no-systemd-session");
        assert_eq!(
            Guardrail::LegacyInstallShadowing.as_str(),
            "legacy-install-shadowing"
        );
    }

    #[test]
    fn outcome_strings_match_the_wire_contract() {
        assert_eq!(Outcome::Ok.as_str(), "ok");
        assert_eq!(Outcome::Blocked.as_str(), "blocked");
        assert_eq!(Outcome::Error.as_str(), "error");
    }
}
