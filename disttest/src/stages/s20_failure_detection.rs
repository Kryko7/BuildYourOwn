//! Stage 20 — Backoff, phi-accrual and SWIM suspicion.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 20.
pub fn stage() -> Stage {
    Stage {
        number: 20,
        slug: "failure_detection",
        name: "Backoff, phi-accrual and SWIM suspicion",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `backoff`: full jitter is uniform(0, min(cap, base * 2^attempt))",
            "Topic `phi-accrual`: phi rises with the time since the last heartbeat, smoothly",
            "Topic `swim`: alive, then suspect, then dead — two deadlines, both sharp",
            "The sampled value arrives with the command, so a jittered delay is still reproducible",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
