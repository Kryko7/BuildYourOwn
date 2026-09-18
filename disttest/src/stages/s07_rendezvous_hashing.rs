//! Stage 07 — Rendezvous (HRW) hashing.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 07.
pub fn stage() -> Stage {
    Stage {
        number: 7,
        slug: "rendezvous_hashing",
        name: "Rendezvous (HRW) hashing",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `rendezvous`: score every node for the key and take the highest",
            "No ring and no virtual nodes: the score function does all the work",
            "`locate-k` returns the top k in score order, and its head is what `locate` answers",
            "Removing a node only moves the keys it owned, exactly as a ring does",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
