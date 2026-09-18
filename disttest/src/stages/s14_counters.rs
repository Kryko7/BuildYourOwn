//! Stage 14 — G-Counter and PN-Counter.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 14.
pub fn stage() -> Stage {
    Stage {
        number: 14,
        slug: "counters",
        name: "G-Counter and PN-Counter",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topics `g-counter` and `pn-counter`: one entry per replica, merged by maximum",
            "A PN-Counter is two G-Counters: increments and decrements, never one signed number",
            "Merge must be commutative, associative and idempotent — every delivery order converges",
            "A replica only ever writes its own entry; it copies others by taking the maximum",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
