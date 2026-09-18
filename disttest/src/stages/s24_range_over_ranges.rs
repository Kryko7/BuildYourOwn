//! Stage 24 — Ranges, prefixes and the whole store.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 24.
pub fn stage() -> Stage {
    Stage {
        number: 24,
        slug: "range_over_ranges",
        name: "Ranges, prefixes and the whole store",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "range_end makes the request a half-open range [key, range_end)",
            "A prefix range ends at the key with its last byte incremented",
            "key and range_end both \0 means every key in the store",
            "Results come back in key order, which is byte order, not string order",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
