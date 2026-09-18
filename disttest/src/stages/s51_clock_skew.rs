//! Stage 51 — Clock skew changes nothing about safety.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 51.
pub fn stage() -> Stage {
    Stage {
        number: 51,
        slug: "clock_skew",
        name: "Clock skew changes nothing about safety",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "Raft safety rests on terms and indexes, never on wall-clock time",
            "A member whose clock is hours off must still agree on every value",
            "Lease expiry may be affected; linearizability may not",
            "Never compare timestamps from two different machines to order writes",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
