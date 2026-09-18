//! Stage 27 — prev_kv and deleting ranges.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 27.
pub fn stage() -> Stage {
    Stage {
        number: 27,
        slug: "prev_kv_and_delete",
        name: "prev_kv and deleting ranges",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "prev_kv on a put answers the pair as it was before the write, or nothing when it is new",
            "POST /v3/kv/deleterange answers how many keys it removed",
            "A delete over a range removes every key in it and counts them all",
            "Deleting a key that is not there is a successful request that deleted zero keys",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
