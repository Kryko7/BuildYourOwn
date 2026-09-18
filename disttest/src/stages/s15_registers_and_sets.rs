//! Stage 15 — LWW-Register and OR-Set.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 15.
pub fn stage() -> Stage {
    Stage {
        number: 15,
        slug: "registers_and_sets",
        name: "LWW-Register and OR-Set",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `lww-register`: value plus timestamp, and a deterministic tie-break on equal timestamps",
            "Topic `or-set`: an add carries a unique tag; a remove takes away the tags it saw",
            "An add concurrent with a remove survives — that is the whole difference from a 2P-Set",
            "Re-adding after a remove must work, which is why tags cannot be reused",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
