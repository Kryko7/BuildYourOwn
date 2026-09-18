//! Stage 34 — Error responses.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 34.
pub fn stage() -> Stage {
    Stage {
        number: 34,
        slug: "error_responses",
        name: "Error responses",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "Errors are a 4xx status and a body of {\"code\": <grpc code>, \"message\": \"...\"}",
            "Bad base64 is code 3, a lease that does not exist is code 5, too large is code 8",
            "An unknown path is 404 and never a 200 with an empty body",
            "A bad request must not change the store, and must not close the connection",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
