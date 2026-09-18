//! Stage 10 — Bloom filter.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 10.
pub fn stage() -> Stage {
    Stage {
        number: 10,
        slug: "bloom_filter",
        name: "Bloom filter",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `bloom`: k hash positions per item over m bits, all set on add",
            "`contains` is only ever 'maybe' or 'no': a false negative is a bug, never a tuning issue",
            "The measured false-positive rate must sit inside the theoretical bound for m, k and n",
            "Derive the k positions from one or two hashes; k independent hash functions are not needed",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
