//! Stage 18 — Causal broadcast delivery order.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 18.
pub fn stage() -> Stage {
    Stage {
        number: 18,
        slug: "causal_broadcast",
        name: "Causal broadcast delivery order",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `causal-broadcast`: a message carries the sender's vector clock",
            "Deliver only when the message is the sender's next, and its other entries are already seen",
            "Anything else waits in a buffer and is re-checked after every delivery",
            "Delivering a message out of causal order is the failure this stage looks for",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
