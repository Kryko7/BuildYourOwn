//! Stage 19 — Token bucket and leaky bucket.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 19.
pub fn stage() -> Stage {
    Stage {
        number: 19,
        slug: "rate_limiting",
        name: "Token bucket and leaky bucket",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `token-bucket`: refill rate * elapsed, capped at the burst, then spend",
            "Topic `leaky-bucket`: drain rate * elapsed, then refuse whatever would overflow",
            "Time arrives with every command: never read a clock of your own",
            "A long idle period fills the token bucket to the burst and no further",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
