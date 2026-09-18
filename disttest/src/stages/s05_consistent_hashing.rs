//! Stage 05 — Consistent hashing with virtual nodes.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 05.
pub fn stage() -> Stage {
    Stage {
        number: 5,
        slug: "consistent_hashing",
        name: "Consistent hashing with virtual nodes",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `consistent-hash`: place `vnodes` points per node on a ring and walk clockwise",
            "`locate` is a function: the same key must always answer the same node",
            "Virtual nodes are what makes the load even — one point per node is visibly lumpy",
            "`stats` reports how many of the sampled keys landed on each node",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
