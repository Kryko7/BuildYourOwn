//! Reference topics: timing. See `examples/reference_primitives.rs`.

use crate::Topic;

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    let _ = topic;
    None
}
