//! Stage 33 — Watches.
//!
//! The first endpoint that pushes. Everything before this was a question and an answer;
//! a watch is a question that keeps answering, and it is what turns the store into
//! something a cluster of processes can coordinate through.
//!
//! Three details carry the stage. The stream acknowledges the watch before it says
//! anything about keys, so a client knows it is registered and no event can slip through
//! the gap. Events are ordered by revision, not by arrival, and several of them may share
//! one message. And a delete event is not a put with an empty value: it carries the key and
//! the revision it happened at, and nothing else, because there is no value left to send.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::etcd::{b64, prefix_end, Event};
use crate::examples::{node_example, node_example_after, ExampleSpec};
use crate::stages::{check_strictly_increasing, ok, text, Ladder, Stage, Test};
use serde_json::json;
use std::time::Duration;

/// How long a test waits for a message it is expecting.
const SOON: Duration = Duration::from_secs(5);

/// How long a test waits before concluding that nothing is coming.
const NOTHING_COMING: Duration = Duration::from_millis(900);

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
        examples,
        tests: vec![
            Test::new("the first message carries created", created_comes_first),
            Test::new("a put produces one event carrying the new kv", put_produces_an_event),
            Test::new(
                "a delete produces an event with the key and a revision and no value",
                delete_produces_an_event,
            ),
            Test::new("start_revision replays history in revision order", start_revision_replays),
            Test::new(
                "a prefix watch sees everything under the prefix and nothing else",
                prefix_watch,
            ),
            Test::new("a burst of writes arrives in revision order", burst_is_ordered),
            Test::new(
                "a create and a cancel in one body are both acknowledged",
                create_then_cancel,
            ),
            Test::new("a watch on a key nothing touches says nothing", quiet_watch),
            Test::new("an event carries prev_kv when the watch asks for it", prev_kv_on_events).ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example(
            "The write a watcher is waiting for",
            "/v3/kv/put",
            || json!({"key": b64(b"wk"), "value": b64(b"v1")}),
        )
        .request("an ordinary put, sent while a watch on `wk` is open")
        .response("the put's own header — and, on the open stream, one event at that revision")
        .note(
            "The watcher's stream carries `{\"result\":{\"header\":{...,\"revision\":\"N\"},\
             \"events\":[{\"kv\":{\"key\":\"d2s=\",\"create_revision\":\"N\",\
             \"mod_revision\":\"N\",\"version\":\"1\",\"value\":\"djE=\"}}]}}`, where N is \
             the revision in this response. There is no `type`: PUT is the zero value of \
             that enum, so it is left out.",
        ),
        node_example_after(
            "And the delete after it",
            || {
                vec![(
                    "/v3/kv/put".to_string(),
                    json!({"key": b64(b"wk"), "value": b64(b"v1")}),
                )]
            },
            "/v3/kv/deleterange",
            || json!({"key": b64(b"wk")}),
        )
        .request("the delete of the same key, again with a watch open")
        .response("`deleted: 1` here, and a second event on the stream")
        .note(
            "That event is `{\"type\":\"DELETE\",\"kv\":{\"key\":\"d2s=\",\
             \"mod_revision\":\"N\"}}`: the key, the revision it went at, and nothing else. \
             A watcher that expects a value here, or expects `version` to be there, breaks \
             on the first delete it sees.",
        ),
    ]
}

/// The one event a test was waiting for, or a failure listing what arrived instead.
fn only_event(events: &[Event], what: &str) -> Result<Event, Failure> {
    if let [one] = events {
        return Ok(one.clone());
    }
    let mut c = Check::new(what);
    c.eq("how many events arrived", 1, events.len());
    c.block(
        "the events that did arrive",
        events
            .iter()
            .map(|e| format!("{} {} = {}", e.kind, e.kv.key_str(), e.kv.value_str()))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    Err(c
        .finish()
        .err()
        .unwrap_or_else(|| Failure::harness("a check that had to fail did not")))
}

dist_test!(created_comes_first, |ctx| {
    let key = ctx.key("ack");
    let kv = ctx.kv()?;
    let mut watch = ok(
        kv.watch(&[json!({"create_request": {"key": b64(&key)}})])
            .await,
        "open a watch",
    )?;
    let first = ok(
        watch.next(SOON).await,
        "read the first message of the stream",
    )?;
    let mut c = Check::new("the message that acknowledges a watch");
    c.block("the response head", watch.head());
    let Some(msg) = first else {
        return c
            .that(
                "the first message",
                "an acknowledgement",
                false,
                "nothing arrived",
            )
            .finish();
    };
    c.eq("watch[0].created", true, msg.created);
    c.eq("watch[0].canceled", false, msg.canceled);
    c.eq("watch[0].events.len()", 0, msg.events.len());
    c.at_least("watch[0].header.revision", 1, msg.header.revision);
    c.ne("watch[0].header.cluster_id", 0, msg.header.cluster_id);
    c.finish()
});

dist_test!(put_produces_an_event, |ctx| {
    let key = ctx.key("one");
    let kv = ctx.kv()?;
    let mut watch = ok(
        kv.watch(&[json!({"create_request": {"key": b64(&key)}})])
            .await,
        "open a watch",
    )?;
    ok(watch.next(SOON).await, "read the acknowledgement")?;
    let put = ok(
        kv.put(&key, b"hello").await,
        "write the key the watch is on",
    )?;
    let events = ok(watch.collect_events(1, SOON).await, "collect the event")?;
    let event = only_event(&events, "one put, one event")?;
    let mut c = Check::new("the event a put produces");
    // A missing `type` is a put: PUT is the zero value of that enum.
    c.eq(
        "watch.events[0].type",
        "PUT".to_string(),
        event.kind.clone(),
    );
    c.eq("watch.events[0].kv.key", text(&key), event.kv.key_str());
    c.eq(
        "watch.events[0].kv.value",
        "hello".to_string(),
        event.kv.value_str(),
    );
    c.eq(
        "watch.events[0].kv.mod_revision",
        put.header.revision,
        event.kv.mod_revision,
    );
    c.eq(
        "watch.events[0].kv.create_revision",
        put.header.revision,
        event.kv.create_revision,
    );
    c.eq("watch.events[0].kv.version", 1, event.kv.version);
    c.finish()
});

dist_test!(delete_produces_an_event, |ctx| {
    let key = ctx.key("doomed");
    let kv = ctx.kv()?;
    ok(
        kv.put(&key, b"here").await,
        "write the key before the watch opens",
    )?;
    let mut watch = ok(
        kv.watch(&[json!({"create_request": {"key": b64(&key)}})])
            .await,
        "open a watch",
    )?;
    ok(watch.next(SOON).await, "read the acknowledgement")?;
    let deleted = ok(kv.delete(&key).await, "delete the key")?;
    let events = ok(watch.collect_events(1, SOON).await, "collect the event")?;
    let event = only_event(&events, "one delete, one event")?;
    let mut c = Check::new("the event a delete produces");
    c.eq("deleterange.deleted", 1, deleted.deleted);
    c.eq(
        "watch.events[0].type",
        "DELETE".to_string(),
        event.kind.clone(),
    );
    c.eq("watch.events[0].kv.key", text(&key), event.kv.key_str());
    // There is no value to send: the key is gone, and only the revision it went at is news.
    c.eq("watch.events[0].kv.value.len()", 0, event.kv.value.len());
    c.eq(
        "watch.events[0].kv.mod_revision",
        deleted.header.revision,
        event.kv.mod_revision,
    );
    c.finish()
});

dist_test!(start_revision_replays, |ctx| {
    let key = ctx.key("replay");
    let kv = ctx.kv()?;
    let mut revisions = Vec::new();
    for i in 0..3 {
        revisions.push(
            ok(
                kv.put(&key, format!("v{i}").as_bytes()).await,
                "write the key before anybody is watching",
            )?
            .header
            .revision,
        );
    }
    // Start from the second write: the watch must hand back history nobody was listening to.
    let mut watch = ok(
        kv.watch(&[json!({
            "create_request": {"key": b64(&key), "start_revision": revisions[1].to_string()}
        })])
        .await,
        "open a watch from a revision in the past",
    )?;
    let events = ok(
        watch.collect_events(2, SOON).await,
        "collect the replayed events",
    )?;
    let mut c = Check::new("history replayed from start_revision");
    c.eq("how many events were replayed", 2, events.len());
    c.eq(
        "the values, oldest first",
        vec!["v1".to_string(), "v2".to_string()],
        events.iter().map(|e| e.kv.value_str()).collect::<Vec<_>>(),
    );
    check_strictly_increasing(
        &mut c,
        "watch.events[].kv.mod_revision",
        &events.iter().map(|e| e.kv.mod_revision).collect::<Vec<_>>(),
    );
    c.eq(
        "watch.events[0].kv.mod_revision",
        revisions[1],
        events.first().map(|e| e.kv.mod_revision).unwrap_or(0),
    );
    c.finish()
});

dist_test!(prefix_watch, |ctx| {
    let prefix = ctx.key("under");
    let outside = ctx.key("elsewhere");
    let kv = ctx.kv()?;
    let mut watch = ok(
        kv.watch(&[json!({
            "create_request": {"key": b64(&prefix), "range_end": b64(&prefix_end(&prefix))}
        })])
        .await,
        "open a watch over a whole prefix",
    )?;
    ok(watch.next(SOON).await, "read the acknowledgement")?;
    let inside: Vec<Vec<u8>> = ["/a", "/b", "/c"]
        .iter()
        .map(|suffix| [prefix.as_slice(), suffix.as_bytes()].concat())
        .collect();
    for key in &inside {
        ok(kv.put(key, b"v").await, "write a key under the prefix")?;
    }
    ok(
        kv.put(&outside, b"v").await,
        "write a key outside the prefix",
    )?;
    // Ask for one more event than the prefix can produce, so anything leaking in shows up.
    let events = ok(
        watch.collect_events(4, Duration::from_secs(2)).await,
        "collect what the prefix saw",
    )?;
    let mut c = Check::new("a watch over a range of keys");
    c.eq("how many events arrived", 3, events.len());
    c.eq(
        "the keys the watch saw",
        inside.iter().map(|k| text(k)).collect::<Vec<_>>(),
        events.iter().map(|e| e.kv.key_str()).collect::<Vec<_>>(),
    );
    c.that(
        "the key outside the prefix",
        "no event at all",
        !events.iter().any(|e| e.kv.key == outside),
        text(&outside),
    );
    c.finish()
});

dist_test!(burst_is_ordered, |ctx| {
    let prefix = ctx.key("burst");
    let kv = ctx.kv()?;
    let mut watch = ok(
        kv.watch(&[json!({
            "create_request": {"key": b64(&prefix), "range_end": b64(&prefix_end(&prefix))}
        })])
        .await,
        "open a watch over the prefix",
    )?;
    ok(watch.next(SOON).await, "read the acknowledgement")?;
    let mut written = Vec::new();
    for i in 0..20 {
        let key = [prefix.as_slice(), format!("/{i:02}").as_bytes()].concat();
        let put = ok(
            kv.put(&key, format!("v{i}").as_bytes()).await,
            "write one of a burst",
        )?;
        written.push((text(&key), put.header.revision));
    }
    let events = ok(
        watch.collect_events(20, SOON).await,
        "collect the whole burst",
    )?;
    let mut c = Check::new("twenty writes down one watch");
    c.eq("how many events arrived", 20, events.len());
    // Events may be batched into as few messages as the server likes; what may not change
    // is the order, which is the order the store applied them in.
    check_strictly_increasing(
        &mut c,
        "watch.events[].kv.mod_revision",
        &events.iter().map(|e| e.kv.mod_revision).collect::<Vec<_>>(),
    );
    c.eq(
        "the keys, in the order they were written",
        written.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
        events.iter().map(|e| e.kv.key_str()).collect::<Vec<_>>(),
    );
    c.eq(
        "the revisions the events report",
        written.iter().map(|(_, r)| *r).collect::<Vec<_>>(),
        events.iter().map(|e| e.kv.mod_revision).collect::<Vec<_>>(),
    );
    c.finish()
});

dist_test!(create_then_cancel, |ctx| {
    let key = ctx.key("shortlived");
    let kv = ctx.kv()?;
    // Several request messages travel in one body, one per line: that is how a client
    // creates a watch and then takes it back down the same stream.
    let mut watch = ok(
        kv.watch(&[
            json!({"create_request": {"key": b64(&key)}}),
            json!({"cancel_request": {"watch_id": "0"}}),
        ])
        .await,
        "open a watch and cancel it in the same request",
    )?;
    let created = ok(watch.next(SOON).await, "read the first message")?;
    let canceled = ok(watch.next(SOON).await, "read the second message")?;
    ok(
        kv.put(&key, b"after the cancel").await,
        "write the key after the cancel",
    )?;
    let after = ok(
        watch.next(NOTHING_COMING).await,
        "look for an event after the cancel",
    )?;
    let mut c = Check::new("a watch created and cancelled in one body");
    c.eq(
        "watch[0].created",
        true,
        created.as_ref().map(|m| m.created).unwrap_or(false),
    );
    c.eq(
        "watch[1].canceled",
        true,
        canceled.as_ref().map(|m| m.canceled).unwrap_or(false),
    );
    c.eq(
        "watch[1].watch_id",
        created.as_ref().map(|m| m.watch_id).unwrap_or(-1),
        canceled.as_ref().map(|m| m.watch_id).unwrap_or(-2),
    );
    c.that(
        "anything after the cancel",
        "no events at all",
        after.as_ref().map(|m| m.events.is_empty()).unwrap_or(true),
        after.map(|m| m.events.len()),
    );
    c.finish()
});

dist_test!(quiet_watch, |ctx| {
    let watched = ctx.key("quiet");
    let busy = ctx.key("noisy");
    let kv = ctx.kv()?;
    let mut watch = ok(
        kv.watch(&[json!({"create_request": {"key": b64(&watched)}})])
            .await,
        "open a watch on a key nothing will touch",
    )?;
    let created = ok(watch.next(SOON).await, "read the acknowledgement")?;
    for i in 0..5 {
        ok(
            kv.put(&busy, format!("v{i}").as_bytes()).await,
            "write a different key over and over",
        )?;
    }
    ok(kv.delete(&busy).await, "and delete it again")?;
    let nothing = ok(
        watch.next(NOTHING_COMING).await,
        "wait for a message that must not come",
    )?;
    let mut c = Check::new("a watch on a key nobody writes");
    c.eq(
        "watch[0].created",
        true,
        created.map(|m| m.created).unwrap_or(false),
    );
    // A watch is a filter, not a firehose: traffic on other keys is not this watch's news.
    c.that(
        "the next message",
        "nothing at all",
        nothing.is_none(),
        nothing.map(|m| (m.events.len(), m.created, m.canceled)),
    );
    c.finish()
});

dist_test!(prev_kv_on_events, |ctx| {
    let key = ctx.key("before-and-after");
    let kv = ctx.kv()?;
    ok(
        kv.put(&key, b"old").await,
        "write the key before the watch opens",
    )?;
    let mut watch = ok(
        kv.watch(&[json!({"create_request": {"key": b64(&key), "prev_kv": true}})])
            .await,
        "open a watch asking for the previous value",
    )?;
    ok(watch.next(SOON).await, "read the acknowledgement")?;
    ok(kv.put(&key, b"new").await, "overwrite the key")?;
    let events = ok(watch.collect_events(1, SOON).await, "collect the event")?;
    let event = only_event(&events, "one overwrite, one event")?;
    let mut c = Check::new("prev_kv on a watch");
    c.eq(
        "watch.events[0].kv.value",
        "new".to_string(),
        event.kv.value_str(),
    );
    c.eq("watch.events[0].kv.version", 2, event.kv.version);
    c.eq(
        "watch.events[0].prev_kv.value",
        Some("old".to_string()),
        event.prev_kv.as_ref().map(|k| k.value_str()),
    );
    c.eq(
        "watch.events[0].prev_kv.version",
        Some(1),
        event.prev_kv.as_ref().map(|k| k.version),
    );
    c.finish()
});
