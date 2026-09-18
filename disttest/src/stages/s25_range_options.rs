//! Stage 25 — limit, sort order, count_only and keys_only.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 25.
pub fn stage() -> Stage {
    Stage {
        number: 25,
        slug: "range_options",
        name: "limit, sort order, count_only and keys_only",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "limit cuts the answer and sets more, but never changes count",
            "sort_order and sort_target together decide the order; the default is ascending by key",
            "count_only answers the count and no kvs at all",
            "keys_only answers the kvs with their values left out, not with empty values invented",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
