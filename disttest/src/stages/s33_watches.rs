//! Stage 33 — Watches.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 33.
pub fn stage() -> Stage {
    Stage {
        number: 33,
        slug: "watches",
        name: "Watches",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/watch is a stream: every message is a line of JSON wrapped in {\"result\": ...}",
            "The first message acknowledges the create request and carries created: true",
            "start_revision replays everything from that revision onwards, in revision order",
            "A delete event carries type DELETE and a kv with only the key and mod_revision",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
