//! Stage 31 — Leases: grant, attach, expire, revoke.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 31.
pub fn stage() -> Stage {
    Stage {
        number: 31,
        slug: "leases",
        name: "Leases: grant, attach, expire, revoke",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/lease/grant answers an ID and the TTL it settled on",
            "A put naming a lease binds the key to it; the kv answers with that lease id",
            "When the lease expires every key attached to it disappears in one revision",
            "Revoking is the same thing on demand, and revoking twice is an error",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
