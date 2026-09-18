//! Stage 17 — Chandy-Lamport snapshots.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 17.
pub fn stage() -> Stage {
    Stage {
        number: 17,
        slug: "chandy_lamport",
        name: "Chandy-Lamport snapshots",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `snapshot`: a process records its own state when it first sees a marker",
            "After that it records every message arriving on a channel until that channel's marker",
            "The snapshot's total must equal the system's total, whatever the interleaving",
            "A process sends a marker on every outgoing channel immediately after recording",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
