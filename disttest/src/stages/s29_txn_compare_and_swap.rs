//! Stage 29 — Compare-and-swap semantics.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 29.
pub fn stage() -> Stage {
    Stage {
        number: 29,
        slug: "txn_compare_and_swap",
        name: "Compare-and-swap semantics",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "VERSION = 0 is how you say 'this key does not exist'",
            "CREATE, MOD, VALUE and VERSION are four different comparisons with four different fields",
            "Two concurrent swaps from the same value: exactly one may succeed",
            "A comparison naming a key that does not exist compares against zeros, it does not fail the request",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
