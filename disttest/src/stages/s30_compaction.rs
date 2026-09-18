//! Stage 30 — Compaction and ErrCompacted.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 30.
pub fn stage() -> Stage {
    Stage {
        number: 30,
        slug: "compaction",
        name: "Compaction and ErrCompacted",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/kv/compaction drops every revision below the one given",
            "A read below the compacted revision is code 11, not an empty answer",
            "Compaction never removes the newest version of a live key",
            "Compacting twice, or below what is already compacted, is an error, not a no-op",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
