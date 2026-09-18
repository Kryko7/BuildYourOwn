//! Stage 21 — Bind, /version and /health.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 21.
pub fn stage() -> Stage {
    Stage {
        number: 21,
        slug: "bind_version_health",
        name: "Bind, /version and /health",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "Listen on the --listen-client-urls address and answer HTTP/1.1 on it",
            "GET /version answers {\"etcdserver\":..,\"etcdcluster\":..} and GET /health {\"health\":\"true\"}",
            "Keep the connection alive: the suite reuses one connection for thousands of requests",
            "An unknown path is 404, and a GET on a POST-only endpoint is not a 200",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
