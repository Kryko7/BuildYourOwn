//! Stage 12 — Merkle tree build.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 12.
pub fn stage() -> Stage {
    Stage {
        number: 12,
        slug: "merkle_tree",
        name: "Merkle tree build",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `merkle`: leaf = sha256(0x00 || leaf bytes), node = sha256(0x01 || left hex || right hex)",
            "An odd node at a level is carried up unchanged, not duplicated",
            "The root changes when any leaf changes, and only then",
            "The same leaves always produce the same root, whatever order they were added in",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
