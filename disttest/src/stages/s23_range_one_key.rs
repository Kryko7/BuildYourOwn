//! Stage 23 — Range: reading one key.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 23.
pub fn stage() -> Stage {
    Stage {
        number: 23,
        slug: "range_one_key",
        name: "Range: reading one key",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/kv/range with just a key is a point read",
            "A key that is not there answers no kvs and count 0 — not an error",
            "create_revision is set once, mod_revision moves with every put, version counts the puts",
            "count is the number of keys the range holds, whatever limit was asked for",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
