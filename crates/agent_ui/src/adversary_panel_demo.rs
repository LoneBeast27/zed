//! Staged adversary result under the shared agentic-demo gate
//! ([`crate::bridge::is_agentic_demo`]). NOT a product surface: a believable
//! three-vendor answer set + a synthesis carrying the AGREEMENTS /
//! DISAGREEMENTS / SYNTHESIS sections, so the three-column panel + synthesis
//! card review without a live broadcast. Decoded once (a plain builder).

use std::collections::HashMap;

use crate::bridge::{AdversaryPhase, AdversaryResult};

/// A staged completed broadcast: one answer per vendor column + a synthesis
/// with all three parse sections populated.
pub fn demo_phase() -> AdversaryPhase {
    let mut answers = HashMap::new();
    answers.insert(
        "claude".to_string(),
        "A race condition is a bug where the outcome depends on the unpredictable \
         interleaving of concurrent operations touching shared state."
            .to_string(),
    );
    answers.insert(
        "codex".to_string(),
        "It's when two or more threads access shared data concurrently and at least \
         one writes, so the result hinges on timing rather than program logic."
            .to_string(),
    );
    answers.insert(
        "gemini".to_string(),
        "A race condition occurs when correctness depends on the relative timing of \
         events — typically unsynchronized reads/writes to shared memory."
            .to_string(),
    );
    let synthesis = "AGREEMENTS: All three define a race as timing-dependent behavior \
        over shared state with at least one writer.\n\n\
        DISAGREEMENTS: Codex scopes it to threads specifically; Claude and Gemini keep \
        it general to any concurrent operations.\n\n\
        SYNTHESIS: A race condition is a defect where a program's correctness depends on \
        the non-deterministic ordering of concurrent accesses to shared state — at least \
        one of which is a write — rather than on its logic."
        .to_string();
    AdversaryPhase::Done(AdversaryResult {
        answers,
        synthesis: Some(synthesis),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{VENDOR_COLUMNS, parse_synthesis_sections};

    #[test]
    fn demo_phase_fills_every_column_and_all_synthesis_sections() {
        let AdversaryPhase::Done(result) = demo_phase() else {
            panic!("demo phase must be Done");
        };
        for vendor in VENDOR_COLUMNS {
            assert!(
                result.answers.contains_key(vendor),
                "{vendor} column has a staged answer"
            );
        }
        let sections = parse_synthesis_sections(result.synthesis.as_deref().unwrap());
        assert!(!sections.agreements.is_empty());
        assert!(!sections.disagreements.is_empty());
        assert!(!sections.synthesis.is_empty());
    }
}
