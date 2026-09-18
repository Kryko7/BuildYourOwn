//! Stage 01 — Lamport clocks.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 01.
pub fn stage() -> Stage {
    Stage {
        number: 1,
        slug: "lamport_clocks",
        name: "Lamport clocks",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `lamport`: keep one counter per process and hand it out with `send`",
            "A local event and a send both increment the counter before it is used",
            "On receive the counter becomes max(local, received) + 1 — the +1 is the whole point",
            "Counters never go backwards, even when a message arrives from the past",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
