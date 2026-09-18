//! Stage 35 — Durability across SIGKILL.
//!
//! The stage that hurts. Everything before this could be done in a hash map; this one
//! cannot be done at all unless the answer to a write is held back until the write is on
//! the disk. The harness kills the process with `SIGKILL` — no handler runs, no buffer is
//! flushed, nothing is closed — and starts it again from the same data directory, with
//! nothing but what was already written down.
//!
//! What is being tested is a promise, not a mechanism. A 200 on a put says "this survives a
//! power cut". Everything the store answered before the kill has to be there afterwards:
//! the values, the revisions each of them was written at, the store's clock, and the
//! deletes as much as the writes. Writes that were still in flight when the process died
//! may be there or may not — both are correct, because nobody was ever told they happened.
//!
//! `examples/broken_node.rs` acknowledges writes before they are durable, and exists to
//! show what this stage's red output looks like.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::etcd::{b64, RangeRequest};
use crate::examples::{node_example, node_example_after, ExampleSpec};
use crate::stages::{check_strictly_increasing, ok, text, Ctx, Ladder, Stage, Test};
use serde_json::json;

/// Every test here starts a node twice and waits for it to come back; the default per-test
/// timeout is not enough on a cold machine.
const RESTART_TIMEOUT_MS: u64 = 30_000;

/// Stage 35.
pub fn stage() -> Stage {
    Stage {
        number: 35,
        slug: "durability",
        name: "Durability across SIGKILL",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "An acknowledged write must survive kill -9 and a restart: fsync before you answer",
            "The data directory must be reopenable without being reformatted",
            "The revision sequence continues where it left off; it never restarts at 1",
            "A torn record at the tail of the log may be dropped, but never an acknowledged write",
        ],
        examples,
        tests: vec![
            Test::new("an acknowledged write survives a kill", one_write_survives)
                .min_timeout_ms(RESTART_TIMEOUT_MS),
            Test::new(
                "a hundred acknowledged writes all survive",
                a_hundred_writes_survive,
            )
            .min_timeout_ms(RESTART_TIMEOUT_MS),
            Test::new(
                "the revision sequence continues after a restart",
                the_clock_continues,
            )
            .min_timeout_ms(RESTART_TIMEOUT_MS),
            Test::new(
                "create_revision, mod_revision and version survive a restart",
                the_bookkeeping_survives,
            )
            .min_timeout_ms(RESTART_TIMEOUT_MS),
            Test::new(
                "a kill in the middle of a workload loses nothing that was acknowledged",
                a_kill_mid_workload,
            )
            .min_timeout_ms(RESTART_TIMEOUT_MS),
            Test::new("a delete is as durable as a write", deletes_are_durable)
                .min_timeout_ms(RESTART_TIMEOUT_MS),
            Test::new("two restarts in a row lose nothing", two_restarts)
                .min_timeout_ms(RESTART_TIMEOUT_MS),
            Test::new(
                "the data directory is reopened, not reformatted",
                the_data_directory_is_reopened,
            )
            .min_timeout_ms(RESTART_TIMEOUT_MS),
            Test::new(
                "a large value comes back byte for byte",
                a_large_value_survives,
            )
            .ext()
            .min_timeout_ms(RESTART_TIMEOUT_MS),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example("The put whose 200 is a promise", "/v3/kv/put", || {
            json!({"key": b64(b"durable"), "value": b64(b"survive me")})
        })
        .request("one ordinary write, answered with one ordinary header")
        .response("HTTP 200 — which is the store saying this write is on the disk")
        .note(
            "Nothing in this exchange says anything about fsync, and that is the point: the \
             only place durability is visible is in what the store is still able to answer \
             after it has been killed. A store that answers this a millisecond sooner by \
             writing later passes every earlier stage and fails this one.",
        ),
        node_example_after(
            "And the read that comes after the power cut",
            || {
                vec![(
                    "/v3/kv/put".to_string(),
                    json!({"key": b64(b"durable"), "value": b64(b"survive me")}),
                )]
            },
            "/v3/kv/range",
            || json!({"key": b64(b"durable")}),
        )
        .request("the same point read the suite sends once the process has been killed and started again")
        .response("the value, with the create_revision, mod_revision and version it had before")
        .note(
            "The harness sends this against a process that has been `SIGKILL`ed and \
             restarted from the same --data-dir. The revisions have to match the ones from \
             before the kill: a store that reloads its keys but renumbers them has lost the \
             history every watcher and every compare-and-swap depends on.",
        ),
    ]
}

/// Write `n` keys under `prefix`, in order, and hand back what was acknowledged.
///
/// The values are derived from the index, so a value that comes back wrong names the write
/// it came from.
async fn write_series(
    ctx: &Ctx,
    prefix: &[u8],
    n: usize,
) -> Result<Vec<(Vec<u8>, String, i64)>, Failure> {
    let kv = ctx.kv()?;
    let mut written = Vec::with_capacity(n);
    for i in 0..n {
        let key = [prefix, format!("/{i:04}").as_bytes()].concat();
        let value = format!("value-{i:04}");
        let put = ok(
            kv.put(&key, value.as_bytes()).await,
            "write one key of the series",
        )?;
        written.push((key, value, put.header.revision));
    }
    Ok(written)
}

/// Read every key back and report the first one that is missing or wrong.
async fn check_series(
    ctx: &Ctx,
    c: &mut Check,
    written: &[(Vec<u8>, String, i64)],
) -> Result<(), Failure> {
    let kv = ctx.kv()?;
    let mut missing = Vec::new();
    let mut wrong = Vec::new();
    for (key, value, revision) in written {
        let read = ok(kv.get_key(key).await, "read one key of the series back")?;
        match read.one() {
            None => missing.push(text(key)),
            Some(kv_pair) => {
                if &kv_pair.value_str() != value || kv_pair.mod_revision != *revision {
                    wrong.push(format!(
                        "{}: expected {value} at revision {revision}, got {} at revision {}",
                        text(key),
                        kv_pair.value_str(),
                        kv_pair.mod_revision
                    ));
                }
            }
        }
    }
    c.eq("acknowledged writes that are missing", 0, missing.len());
    c.eq("acknowledged writes that came back wrong", 0, wrong.len());
    if !missing.is_empty() {
        c.block("the keys that were lost", first_few(&missing));
    }
    if !wrong.is_empty() {
        c.block("the keys that came back wrong", first_few(&wrong));
    }
    Ok(())
}

/// The first few lines of a long list, because a hundred lost keys are one bug.
fn first_few(lines: &[String]) -> String {
    let mut out: Vec<String> = lines.iter().take(10).cloned().collect();
    if lines.len() > out.len() {
        out.push(format!("... and {} more", lines.len() - out.len()));
    }
    out.join("\n")
}

dist_test!(one_write_survives, |ctx| {
    let key = ctx.key("survivor");
    let put = ok(ctx.kv()?.put(&key, b"still here").await, "write one key")?;
    ctx.restart_node().await?;
    let read = ok(
        ctx.kv()?.get_key(&key).await,
        "read it back after the restart",
    )?;
    let mut c = Check::new("one acknowledged write across a power cut");
    c.eq("range.count", 1, read.count);
    c.eq(
        "range.kvs[0].value",
        "still here".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range.kvs[0].mod_revision",
        put.header.revision,
        read.one().map(|k| k.mod_revision).unwrap_or(0),
    );
    // The store cannot have gone backwards in time either.
    c.at_least(
        "range.header.revision",
        put.header.revision,
        read.header.revision,
    );
    c.finish()
});

dist_test!(a_hundred_writes_survive, |ctx| {
    let prefix = ctx.key("hundred");
    let written = write_series(ctx, &prefix, 100).await?;
    ctx.restart_node().await?;
    let all = ok(
        ctx.kv()?.range_req(&RangeRequest::prefix(&prefix)).await,
        "read the whole prefix back after the restart",
    )?;
    let mut c = Check::new("a hundred acknowledged writes across a power cut");
    c.eq("range.count", 100, all.count);
    check_series(ctx, &mut c, &written).await?;
    c.finish()
});

dist_test!(the_clock_continues, |ctx| {
    let prefix = ctx.key("clock");
    let written = write_series(ctx, &prefix, 5).await?;
    let last = written.last().map(|(_, _, r)| *r).unwrap_or(0);
    ctx.restart_node().await?;
    let status = ok(
        ctx.kv()?.status().await,
        "ask where the clock is after the restart",
    )?;
    let after = ok(
        ctx.kv()?
            .put(&ctx.key("fresh"), b"written after the restart")
            .await,
        "write a key after the restart",
    )?;
    let read = ok(
        ctx.kv()?.get_key(&ctx.key("fresh")).await,
        "read that key back",
    )?;
    let mut c = Check::new("the store's clock across a power cut");
    // Revisions are the store's history: restarting is not a new history.
    c.at_least("status.header.revision", last, status.header.revision);
    c.at_least(
        "put(after).header.revision",
        last + 1,
        after.header.revision,
    );
    c.eq(
        "range(after).kvs[0].mod_revision",
        after.header.revision,
        read.one().map(|k| k.mod_revision).unwrap_or(0),
    );
    // A key written after the restart must not be able to claim a revision an older key
    // already used, which is exactly what restarting the count at 1 would do.
    c.at_least(
        "the new key's revision compared with the old ones",
        last + 1,
        read.one().map(|k| k.mod_revision).unwrap_or(0),
    );
    c.finish()
});

dist_test!(the_bookkeeping_survives, |ctx| {
    let key = ctx.key("bookkeeping");
    let (born, latest) = {
        let kv = ctx.kv()?;
        let born = ok(kv.put(&key, b"one").await, "create the key")?;
        ok(kv.put(&key, b"two").await, "write it a second time")?;
        let latest = ok(kv.put(&key, b"three").await, "write it a third time")?;
        (born.header.revision, latest.header.revision)
    };
    let before = ok(ctx.kv()?.get_key(&key).await, "read it before the kill")?;
    ctx.restart_node().await?;
    let after = ok(ctx.kv()?.get_key(&key).await, "read it after the restart")?;
    let mut c = Check::new("the revision bookkeeping of a key that outlived a kill");
    c.eq("range.count", 1, after.count);
    c.eq(
        "range.kvs[0].value",
        "three".to_string(),
        after.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range.kvs[0].create_revision",
        born,
        after.one().map(|k| k.create_revision).unwrap_or(0),
    );
    c.eq(
        "range.kvs[0].mod_revision",
        latest,
        after.one().map(|k| k.mod_revision).unwrap_or(0),
    );
    // version counts the puts this key has seen, and a restart is not a put.
    c.eq(
        "range.kvs[0].version",
        3,
        after.one().map(|k| k.version).unwrap_or(0),
    );
    c.eq(
        "the kv before the kill and the kv after it",
        before.one().cloned(),
        after.one().cloned(),
    );
    c.finish()
});

dist_test!(a_kill_mid_workload, |ctx| {
    let prefix = ctx.key("workload");
    // Write in a loop and kill the moment the last acknowledgement lands: no pause, no
    // flush, nothing that would let a lazy writer catch up.
    let written = write_series(ctx, &prefix, 60).await?;
    ctx.kill_node().await?;
    ctx.start_node().await?;
    let all = ok(
        ctx.kv()?.range_req(&RangeRequest::prefix(&prefix)).await,
        "read the whole workload back",
    )?;
    let mut c = Check::new("sixty acknowledgements and then a power cut");
    c.note("every one of these writes was answered before the kill, so every one must be here");
    c.eq("range.count", 60, all.count);
    check_series(ctx, &mut c, &written).await?;
    // The revisions they were given must still be in order, with no gaps invented.
    check_strictly_increasing(
        &mut c,
        "range.kvs[].mod_revision",
        &all.kvs.iter().map(|k| k.mod_revision).collect::<Vec<_>>(),
    );
    c.finish()
});

dist_test!(deletes_are_durable, |ctx| {
    let kept = ctx.key("kept");
    let removed = ctx.key("removed");
    let deleted_at = {
        let kv = ctx.kv()?;
        ok(kv.put(&kept, b"keep me").await, "write the key that stays")?;
        ok(
            kv.put(&removed, b"delete me").await,
            "write the key that goes",
        )?;
        ok(kv.delete(&removed).await, "delete it again")?
            .header
            .revision
    };
    ctx.restart_node().await?;
    let survivor = ok(ctx.kv()?.get_key(&kept).await, "read the key that stayed")?;
    let ghost = ok(
        ctx.kv()?.get_key(&removed).await,
        "read the key that was deleted",
    )?;
    let mut c = Check::new("a delete across a power cut");
    c.eq("range(kept).count", 1, survivor.count);
    c.eq(
        "range(kept).kvs[0].value",
        "keep me".to_string(),
        survivor.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    // A store that replays its writes but not its deletes brings the dead back.
    c.eq("range(removed).count", 0, ghost.count);
    c.at_least(
        "range(removed).header.revision",
        deleted_at,
        ghost.header.revision,
    );
    c.finish()
});

dist_test!(two_restarts, |ctx| {
    let first = ctx.key("first");
    let second = ctx.key("second");
    let third = ctx.key("third");
    let one = ok(
        ctx.kv()?.put(&first, b"1").await,
        "write before the first kill",
    )?;
    ctx.restart_node().await?;
    let two = ok(
        ctx.kv()?.put(&second, b"2").await,
        "write between the two kills",
    )?;
    ctx.restart_node().await?;
    let three = ok(
        ctx.kv()?.put(&third, b"3").await,
        "write after the second kill",
    )?;
    let mut c = Check::new("two power cuts in a row");
    for (label, key, value, revision) in [
        ("first", &first, "1", one.header.revision),
        ("second", &second, "2", two.header.revision),
        ("third", &third, "3", three.header.revision),
    ] {
        let read = ok(ctx.kv()?.get_key(key).await, "read one of the three keys")?;
        c.eq(&format!("range({label}).count"), 1, read.count);
        c.eq(
            &format!("range({label}).kvs[0].value"),
            value.to_string(),
            read.one().map(|k| k.value_str()).unwrap_or_default(),
        );
        c.eq(
            &format!("range({label}).kvs[0].mod_revision"),
            revision,
            read.one().map(|k| k.mod_revision).unwrap_or(0),
        );
    }
    check_strictly_increasing(
        &mut c,
        "the three revisions",
        &[
            one.header.revision,
            two.header.revision,
            three.header.revision,
        ],
    );
    c.finish()
});

dist_test!(the_data_directory_is_reopened, |ctx| {
    let key = ctx.key("identity");
    let before = {
        let kv = ctx.kv()?;
        ok(kv.put(&key, b"v").await, "write a key")?;
        ok(kv.status().await, "ask who this node is")?
    };
    ctx.restart_node().await?;
    let after = ok(ctx.kv()?.status().await, "ask again after the restart")?;
    let read = ok(ctx.kv()?.get_key(&key).await, "read the key back")?;
    let mut c = Check::new("the same store, reopened");
    // A node that came back with a new identity threw its data directory away and made a
    // new one; the keys may look fine now, but nothing it wrote before belongs to it.
    c.eq(
        "status.header.cluster_id",
        before.header.cluster_id,
        after.header.cluster_id,
    );
    c.eq(
        "status.header.member_id",
        before.header.member_id,
        after.header.member_id,
    );
    c.at_least(
        "status.header.revision",
        before.header.revision,
        after.header.revision,
    );
    c.eq("range.count", 1, read.count);
    c.finish()
});

dist_test!(a_large_value_survives, |ctx| {
    let key = ctx.key("large");
    // A pattern rather than a constant byte, so a value that comes back truncated, padded
    // or shifted is caught rather than matching by luck.
    let value: Vec<u8> = (0..256 * 1024)
        .map(|i| ((i * 7 + i / 251) % 251) as u8)
        .collect();
    let put = ok(ctx.kv()?.put(&key, &value).await, "write 256 KiB")?;
    ctx.restart_node().await?;
    let read = ok(ctx.kv()?.get_key(&key).await, "read the 256 KiB back")?;
    let mut c = Check::new("a large value across a power cut");
    c.eq(
        "range.kvs[0].value.len()",
        value.len(),
        read.one().map(|k| k.value.len()).unwrap_or(0),
    );
    c.that(
        "range.kvs[0].value",
        "the same 256 KiB, byte for byte",
        read.one().map(|k| k.value.as_slice()) == Some(value.as_slice()),
        read.one().map(|k| k.value.len()),
    );
    c.eq(
        "range.kvs[0].mod_revision",
        put.header.revision,
        read.one().map(|k| k.mod_revision).unwrap_or(0),
    );
    c.finish()
});
