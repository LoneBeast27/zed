//! The standardized agentic-demo gate (PARITY_SPEC §4.9 constellation-demo
//! precedent, generalized). ONE env var — `ZED_AGENTIC_DEMO=1` — stages
//! believable demo data across ALL mode surfaces (usage, symphony, board,
//! adversary, constellation) so the shell can be reviewed without a live
//! bridge. The pre-existing `ZED_CONSTELLATION_DEMO=1` is honored as an ALIAS
//! (the constellation's original var), so old invocations keep working.
//!
//! Each surface's staged data lives in its own `demo.rs`-style module
//! (`constellation::demo`, `usage_panel::demo`, `symphony_panel::demo`, …)
//! and is decoded once per frame pump — this module is only the shared ON/OFF
//! decision, no timers of its own.

/// The canonical demo env var (2026-07-04).
const AGENTIC_DEMO_VAR: &str = "ZED_AGENTIC_DEMO";
/// The original constellation-only var, honored as an alias so existing
/// `ZED_CONSTELLATION_DEMO=1` invocations still stage the whole shell.
const CONSTELLATION_DEMO_VAR: &str = "ZED_CONSTELLATION_DEMO";

/// Whether an env var is set to a truthy value (present, non-empty, not "0").
fn env_truthy(var: &str) -> bool {
    std::env::var(var).is_ok_and(|value| !value.is_empty() && value != "0")
}

/// Whether agentic demo mode is on — the shared gate every surface's demo
/// path checks. True when EITHER the canonical `ZED_AGENTIC_DEMO` or the
/// legacy `ZED_CONSTELLATION_DEMO` alias is truthy.
pub fn is_agentic_demo() -> bool {
    env_truthy(AGENTIC_DEMO_VAR) || env_truthy(CONSTELLATION_DEMO_VAR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_truthy_matches_the_1_not_0_convention() {
        // The gate reads env at call time; assert the truthiness predicate
        // directly (env mutation in tests is racy across threads).
        for value in ["1", "true", "yes", "on"] {
            // SAFETY: single-threaded test-scoped mutation, cleared below.
            unsafe { std::env::set_var("ZED_DEMO_PREDICATE_PROBE", value) };
            assert!(env_truthy("ZED_DEMO_PREDICATE_PROBE"), "{value} is truthy");
        }
        for value in ["", "0"] {
            unsafe { std::env::set_var("ZED_DEMO_PREDICATE_PROBE", value) };
            assert!(!env_truthy("ZED_DEMO_PREDICATE_PROBE"), "{value:?} is falsy");
        }
        unsafe { std::env::remove_var("ZED_DEMO_PREDICATE_PROBE") };
        assert!(!env_truthy("ZED_DEMO_PREDICATE_PROBE"), "unset is falsy");
    }
}
