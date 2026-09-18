//! Stage 38 — Linearizable reads after a write elsewhere.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 38.
pub fn stage() -> Stage {
    Stage {
        number: 38,
        slug: "linearizable_reads",
        name: "Linearizable reads after a write elsewhere",
        ext: false,
        ladder: Ladder::Cluster,
        hints: &[
            "A read that is not serializable must not answer from a stale local state",
            "Confirm leadership (a read index, or a round of heartbeats) before answering",
            "A write acknowledged by one member must be visible to a read on any other, immediately",
            "serializable: true is the opt-out, and it is the only way to answer locally",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
