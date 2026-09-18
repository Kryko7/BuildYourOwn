//! Stage 04 — Hybrid logical clocks.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 04.
pub fn stage() -> Stage {
    Stage {
        number: 4,
        slug: "hybrid_logical_clocks",
        name: "Hybrid logical clocks",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `hlc`: a timestamp is (l, c) — a physical part and a counter that breaks ties",
            "`now` takes l = max(l, wall); when l did not move, c increments, otherwise c resets to 0",
            "`recv` takes l = max(l, l_local, l_msg) and picks c from whichever source it matched",
            "The result must be strictly greater than both the local clock and the message's",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
