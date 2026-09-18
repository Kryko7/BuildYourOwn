//! Stage 49 — Removing a member.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 49.
pub fn stage() -> Stage {
    Stage {
        number: 49,
        slug: "member_remove",
        name: "Removing a member",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "POST /v3/cluster/member/remove takes the member id, not its name",
            "The quorum size changes with the membership, so removing a member can restore availability",
            "The removed member must stop being counted, and the rest must keep serving",
            "Removing a member that does not exist is an error, not a silent success",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
