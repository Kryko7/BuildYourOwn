//! Stage 33 — placeholder.
use crate::examples::ExampleSpec;
use crate::stages::{Stage, Test};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage { number: 33, slug: "record_sizes", name: "placeholder", ext: false,
        hints: &["a", "b"], examples: examples,
        tests: vec![Test::new("placeholder", nothing)] }
}

tls_test!(nothing, |_ctx| { Ok(()) });

fn examples() -> Vec<ExampleSpec> {
    vec![ExampleSpec::text("x").request("x").response("x")]
}
