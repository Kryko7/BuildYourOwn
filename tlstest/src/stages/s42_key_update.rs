//! Stage 42 — placeholder.
use crate::examples::ExampleSpec;
use crate::stages::{Stage, Test};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage { number: 42, slug: "key_update", name: "placeholder", ext: false,
        hints: &["a", "b"], examples: examples,
        tests: vec![Test::new("placeholder", nothing)] }
}

tls_test!(nothing, |_ctx| { Ok(()) });

fn examples() -> Vec<ExampleSpec> {
    vec![ExampleSpec::text("x").request("x").response("x")]
}
