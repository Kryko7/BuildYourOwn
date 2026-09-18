//! Stage 32 — Lease keepalive and TTL.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 32.
pub fn stage() -> Stage {
    Stage {
        number: 32,
        slug: "lease_keepalive",
        name: "Lease keepalive and TTL",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/lease/keepalive is a stream: the answer is wrapped in {\"result\": ...}",
            "A keepalive resets the lease's remaining time to its TTL",
            "A keepalive for a lease that is gone answers TTL 0, not an error",
            "Keys survive exactly as long as something keeps the lease alive",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
