//! Stage 54 — Linearizability under fault injection.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 54.
pub fn stage() -> Stage {
    Stage {
        number: 54,
        slug: "linearizability",
        name: "Linearizability under fault injection",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A randomized concurrent workload runs while the seeded schedule cuts and heals the network",
            "Every call is recorded with the instant it was made and the instant it answered",
            "An operation whose answer was lost may have happened or not; everything else is pinned",
            "The checker looks for one total order that a single key/value store could have produced",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
