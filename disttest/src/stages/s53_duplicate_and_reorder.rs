//! Stage 53 — Duplicated and reordered peer messages are tolerated.
//!
//! The proxy is put into message mode (README §4.2): it parses the dialer's direction as
//! HTTP/1.1 requests and then sends a quarter of them twice and holds a quarter of them back
//! by one slot. Whole messages are duplicated and swapped, never bytes, because deleting or
//! transposing bytes of a TCP stream corrupts it rather than modelling a network.
//!
//! What this is really testing is that the log, not the wire, decides what happened. An
//! append carries a term and a previous index, so a retransmission is either a no-op or
//! rejected, and an append that overtakes its predecessor fails its previous-index check and
//! is asked for again. A design that instead treats "a message arrived" as "an entry
//! happened" applies the same write twice, and the damage shows up as a version that has
//! climbed further than the number of puts, or a counter that is too large, long after the
//! network is healthy again.
//!
//! Message mode is decided when a connection is accepted, so the stage cuts every link for
//! a fraction of a second before turning the fault on and again before healing it. That is
//! not decoration: without the cut the peers keep using the connections they already had,
//! those connections keep copying bytes straight through, and the whole stage runs against a
//! network nothing ever happened to.
//!
//! The stage reports what the injector managed rather than assuming it. A stream the proxy
//! cannot frame — a chunked one, which is how a raft *stream* endpoint usually looks — is
//! passed through untouched, so a run in which nothing was duplicated is possible and is
//! said out loud in a note. Progress and safety are asserted either way: a cluster that
//! stops making progress while its peers repeat themselves is broken whether or not this
//! particular proxy managed to repeat anything.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::proxy::LinkFault;
use crate::cluster::workload::{self, verify_convergence, FaultSchedule, WorkloadSpec};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::etcd::RangeRequest;
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{check_strictly_increasing, ok, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// How long a mangled cluster is given to name a leader.
const ELECT: Duration = Duration::from_millis(30_000);
/// How long one write is given to find a member willing to take it.
const PATIENCE: Duration = Duration::from_millis(25_000);

/// The fault this stage runs under: a quarter of framed messages sent twice, a quarter of
/// them delivered one slot late.
fn mangled() -> LinkFault {
    LinkFault {
        duplicate: 0.25,
        reorder: 0.25,
        ..Default::default()
    }
}

/// Stage 53.
pub fn stage() -> Stage {
    Stage {
        number: 53,
        slug: "duplicate_and_reorder",
        name: "Duplicated and reordered peer messages are tolerated",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A retransmitted append must be idempotent: applying it twice changes nothing",
            "An out-of-order append is rejected on its previous index, not applied at the wrong place",
            "Terms and indexes are what make this safe, not the order bytes happen to arrive in",
            "The cluster must keep making progress, not merely avoid corruption",
        ],
        examples,
        tests: vec![
            Test::new(
                "a run of writes all succeed while peer messages are mangled",
                progress_is_made,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new("every value written under mangling reads back", values_are_correct)
                .ext()
                .fresh()
                .min_timeout_ms(60_000),
            Test::new("the revision sequence is unbroken", revisions_are_unbroken)
                .ext()
                .fresh()
                .min_timeout_ms(60_000),
            Test::new("every member converges once the mangling stops", members_converge)
                .ext()
                .fresh()
                .min_timeout_ms(60_000),
            Test::new(
                "a retransmitted append is idempotent, so no value is applied twice",
                appends_are_idempotent,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new("one leader is still named under mangling", a_leader_is_named)
                .ext()
                .fresh()
                .min_timeout_ms(60_000),
            Test::new(
                "a read under mangling never invents a value",
                reads_invent_nothing,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "what the proxy managed to mangle is reported",
                the_injector_reports_itself,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        cluster_example("A write that is sent twice and arrives late", || {
            vec![
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": "czUzL2sw", "value": "b25jZQ=="}),
                    "write s53/k0 once, while a quarter of peer messages are duplicated",
                ),
                step(
                    1,
                    "/v3/kv/range",
                    json!({"key": "czUzL2sw"}),
                    "m2's copy: version tells whether the write was applied more than once",
                ),
                step(
                    2,
                    "/v3/kv/range",
                    json!({"key": "czUzL2sw"}),
                    "m3's copy, which must match m2's exactly",
                ),
            ]
        })
        .request("one put, with the peer traffic behind it duplicated and reordered")
        .response("version 1 on every member, and create_revision equal to mod_revision")
        .note(
            "`version` is the field to watch. One put means version 1, however many times the \
         append carrying it crossed the wire. A store that counts messages rather than log \
         indexes answers version 2 here and is wrong in a way no read of the value alone \
         would catch.",
        ),
    ]
}

/// One acknowledged write.
#[derive(Debug, Clone)]
struct Written {
    key: String,
    value: String,
    revision: i64,
}

/// Put one key, trying every running member in turn until one takes it.
async fn put_patiently(
    cluster: &mut Cluster,
    key: &str,
    value: &str,
    within: Duration,
) -> Result<i64, Failure> {
    let deadline = Instant::now() + within;
    let mut last = "no member was running".to_string();
    loop {
        for i in cluster.running() {
            let name = cluster.members[i].name.clone();
            match cluster.members[i]
                .client
                .put(key.as_bytes(), value.as_bytes())
                .await
            {
                Ok(r) => return Ok(r.header.revision),
                Err(e) => last = format!("{name}: {e}"),
            }
        }
        if Instant::now() >= deadline {
            return Err(Failure::new(
                FailureKind::Assertion,
                format!(
                    "no member accepted a write of {key} within {} ms",
                    within.as_millis()
                ),
            )
            .note(last)
            .note(cluster.faults.describe()));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Write `n` keys while the fault stays in place.
async fn write_keys(
    cluster: &mut Cluster,
    prefix: &str,
    n: usize,
) -> Result<Vec<Written>, Failure> {
    let mut out = Vec::new();
    for i in 0..n {
        let key = format!("{prefix}/k{i:02}");
        let value = format!("v{i}");
        let revision = put_patiently(cluster, &key, &value, PATIENCE).await?;
        out.push(Written {
            key,
            value,
            revision,
        });
    }
    Ok(out)
}

/// Demand that every running member holds every acknowledged write at the same revision.
async fn check_every_member_holds(cluster: &mut Cluster, written: &[Written], c: &mut Check) {
    for i in cluster.running() {
        let name = cluster.members[i].name.clone();
        for w in written {
            match cluster.members[i].client.get_key(w.key.as_bytes()).await {
                Ok(r) => match r.one() {
                    Some(kv) => {
                        c.eq(
                            &format!("{name}.range({}).kvs[0].value", w.key),
                            w.value.clone(),
                            kv.value_str(),
                        );
                        c.eq(
                            &format!("{name}.range({}).kvs[0].mod_revision", w.key),
                            w.revision,
                            kv.mod_revision,
                        );
                    }
                    None => {
                        c.that(
                            &format!("{name}.range({}).kvs", w.key),
                            "the acknowledged value, still there",
                            false,
                            "the key is absent",
                        );
                    }
                },
                Err(e) => {
                    c.that(
                        &format!("{name}.range({})", w.key),
                        "an answer",
                        false,
                        e.to_string(),
                    );
                }
            }
        }
    }
}

/// Attach the cluster's state to a failed check.
async fn attach_state(cluster: &mut Cluster, c: &mut Check) {
    if c.ok() {
        return;
    }
    let described = cluster.describe().await;
    c.note(described);
    c.note(cluster.faults.describe());
}

/// Put every link into message mode, making the peers dial again first.
///
/// A proxy decides whether to parse a connection into messages at the instant it accepts it,
/// so connections that were already open when the fault arrived keep copying bytes straight
/// through and nothing is ever duplicated. Cutting every link for a moment makes the peers
/// reconnect, and it is those new connections the injector can mangle.
async fn mangle_every_link(cluster: &mut Cluster) {
    cluster
        .every_link(LinkFault {
            cut: true,
            ..Default::default()
        })
        .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    cluster.every_link(mangled()).await;
}

/// Take every link out of message mode, making the peers dial again.
///
/// The mirror of [`mangle_every_link`], and it matters just as much. A connection that was
/// accepted in message mode keeps trying to parse messages for as long as it lives, even
/// once the fault is gone, so a stream the proxy cannot frame stays stuck after a plain
/// heal. Cutting the links for a moment retires those connections; what the peers open next
/// is a plain byte pipe again.
async fn heal_and_reconnect(cluster: &mut Cluster) {
    cluster
        .every_link(LinkFault {
            cut: true,
            ..Default::default()
        })
        .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    cluster.heal().await;
}

/// Push real concurrent traffic through the peers for a moment.
///
/// A quiet cluster exchanges almost nothing the proxy can frame — heartbeats ride a chunked
/// stream, which message mode leaves alone by design — so a handful of sequential writes can
/// go by with nothing duplicated at all. A few seconds of four clients is what puts enough
/// framed peer messages on the wire for the injector to have something to repeat.
async fn stir(cluster: &mut Cluster, seed: u64, prefix: &str, ms: u64) -> Result<String, Failure> {
    let mut spec = WorkloadSpec::small(seed, prefix);
    spec.duration = Duration::from_millis(ms);
    let result = workload::run(cluster, &spec, &FaultSchedule::none()).await?;
    Ok(result.summary())
}

/// A line saying what the injector managed, and whether it managed anything at all.
fn mangling_note(cluster: &Cluster) -> String {
    let duplicated = cluster.stats.duplicated();
    let reordered = cluster.stats.reordered();
    if duplicated == 0 && reordered == 0 {
        format!(
            "the proxy framed nothing it could mangle, so this run proved tolerance of a \
             healthy stream only — {}",
            cluster.stats.summary()
        )
    } else {
        format!(
            "{duplicated} peer messages were sent twice and {reordered} were delivered one \
             slot late — {}",
            cluster.stats.summary()
        )
    }
}

dist_test!(progress_is_made, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    mangle_every_link(cluster).await;
    let stirred = stir(cluster, seed, &format!("{prefix}/w"), 2_500).await?;
    let started = Instant::now();
    let written = write_keys(cluster, &format!("{prefix}/seq"), 12).await?;
    let took = started.elapsed();
    let note = mangling_note(cluster);
    heal_and_reconnect(cluster).await;
    let mut c =
        Check::new("twelve writes issued while peer messages were duplicated and reordered");
    c.eq("writes acknowledged", 12, written.len());
    attach_state(cluster, &mut c).await;
    ctx.note(stirred);
    ctx.note(format!(
        "twelve further writes took {} ms",
        took.as_millis()
    ));
    ctx.note(note);
    c.finish()
});

dist_test!(values_are_correct, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    mangle_every_link(cluster).await;
    let written = write_keys(cluster, &prefix, 8).await?;
    let note = mangling_note(cluster);
    heal_and_reconnect(cluster).await;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let mut c = Check::new("eight writes made while peer messages were mangled");
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    ctx.note(note);
    c.finish()
});

dist_test!(revisions_are_unbroken, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    mangle_every_link(cluster).await;
    let written = write_keys(cluster, &prefix, 10).await?;
    let note = mangling_note(cluster);
    heal_and_reconnect(cluster).await;
    let revisions: Vec<i64> = written.iter().map(|w| w.revision).collect();
    let mut c = Check::new("the revisions ten mangled writes were acknowledged with");
    check_strictly_increasing(&mut c, "revisions", &revisions);
    // Ten distinct writes to ten fresh keys move the revision on by exactly ten. A store
    // that applied a duplicated append would have skipped further than that.
    if let (Some(first), Some(last)) = (revisions.first(), revisions.last()) {
        c.eq("revisions.last - revisions.first", 9, last - first);
    }
    c.observe("revisions", &revisions);
    attach_state(cluster, &mut c).await;
    ctx.note(note);
    c.finish()
});

dist_test!(members_converge, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(4_000);
    let keys = spec.key_names();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    mangle_every_link(cluster).await;
    let result = workload::run(cluster, &spec, &FaultSchedule::none()).await?;
    let note = mangling_note(cluster);
    heal_and_reconnect(cluster).await;
    let seen = verify_convergence(cluster, &keys, Duration::from_millis(30_000)).await?;
    let summary = result.summary();
    let mut c = Check::new("what every member sees after a mangled workload");
    c.at_least("operations acknowledged", 20, result.acknowledged);
    c.eq("keys agreed on by every member", keys.len(), seen.len());
    attach_state(cluster, &mut c).await;
    ctx.note(summary);
    ctx.note(format!("the members settled on {seen:?}"));
    ctx.note(note);
    c.finish()
});

dist_test!(appends_are_idempotent, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let idem = format!("{prefix}/idem");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    mangle_every_link(cluster).await;
    let stirred = stir(cluster, seed, &format!("{prefix}/w"), 2_500).await?;
    let written = write_keys(cluster, &idem, 10).await?;
    let note = mangling_note(cluster);
    heal_and_reconnect(cluster).await;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let mut c = Check::new("ten keys, each written exactly once, under duplication");
    for i in cluster.running() {
        let name = cluster.members[i].name.clone();
        let req = RangeRequest::prefix(idem.as_bytes());
        let range = ok(
            cluster.members[i].client.range_req(&req).await,
            "a range over the keys this test wrote",
        )?;
        // One put per key, so one key per put and version 1 everywhere. A duplicated
        // append that was applied twice shows up here as version 2, or as an eleventh key.
        c.eq(&format!("{name}.range(prefix).count"), 10, range.count);
        c.eq(
            &format!("{name}.range(prefix).kvs.len()"),
            10,
            range.kvs.len(),
        );
        for kv in &range.kvs {
            c.eq(&format!("{name}.{}.version", kv.key_str()), 1, kv.version);
            c.eq(
                &format!("{name}.{}.create_revision == mod_revision", kv.key_str()),
                kv.create_revision,
                kv.mod_revision,
            );
        }
    }
    attach_state(cluster, &mut c).await;
    ctx.note(stirred);
    ctx.note(note);
    c.finish()
});

dist_test!(a_leader_is_named, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    mangle_every_link(cluster).await;
    let written = write_keys(cluster, &prefix, 4).await?;
    let under_mangling = cluster.wait_for_leader(ELECT).await?;
    let note = mangling_note(cluster);
    heal_and_reconnect(cluster).await;
    let after = cluster.wait_for_leader(ELECT).await?;
    let described = cluster.describe().await;
    let mut c = Check::new("the leader of a cluster whose peers repeat themselves");
    c.that(
        "leader index under mangling",
        "one of the three members",
        under_mangling < 3,
        under_mangling,
    );
    c.eq("writes acknowledged", 4, written.len());
    attach_state(cluster, &mut c).await;
    ctx.note(format!(
        "leader m{} under mangling, m{} after the heal — {described}",
        under_mangling + 1,
        after + 1
    ));
    ctx.note(note);
    c.finish()
});

dist_test!(reads_invent_nothing, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    mangle_every_link(cluster).await;
    let written = write_keys(cluster, &prefix, 6).await?;
    // A key nobody ever wrote, inside the same prefix: a store that replays a duplicated
    // message into the wrong slot is one of the ways this key could come into existence.
    let absent = format!("{prefix}/never-written");
    let note = mangling_note(cluster);
    let mut c = Check::new("what a read sees while peer messages are duplicated and reordered");
    for i in cluster.running() {
        let name = cluster.members[i].name.clone();
        match cluster.members[i].client.get_key(absent.as_bytes()).await {
            Ok(r) => {
                c.eq(&format!("{name}.range({absent}).count"), 0, r.count);
                c.eq(&format!("{name}.range({absent}).kvs.len()"), 0, r.kvs.len());
            }
            Err(e) => {
                // A refused read is not a wrong answer; an invented value would be.
                c.observe(&format!("{name}.range({absent})"), e.to_string());
            }
        }
    }
    heal_and_reconnect(cluster).await;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    ctx.note(note);
    c.finish()
});

dist_test!(the_injector_reports_itself, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    mangle_every_link(cluster).await;
    let stirred = stir(cluster, seed, &format!("{prefix}/w"), 3_000).await?;
    let written = write_keys(cluster, &format!("{prefix}/seq"), 10).await?;
    let duplicated = cluster.stats.duplicated();
    let reordered = cluster.stats.reordered();
    let note = mangling_note(cluster);
    heal_and_reconnect(cluster).await;
    let mut c = Check::new("what the message-mode injector managed");
    // Deliberately not an assertion. A raft stream the proxy cannot frame is passed
    // through untouched by design (README §4.2), so demanding a duplicate here would make
    // the stage fail on a transport that is perfectly correct. Progress and safety are
    // asserted; what was mangled is reported.
    c.observe("cluster.stats.duplicated()", duplicated);
    c.observe("cluster.stats.reordered()", reordered);
    c.eq(
        "writes acknowledged while in message mode",
        10,
        written.len(),
    );
    attach_state(cluster, &mut c).await;
    ctx.note(stirred);
    ctx.note(note);
    c.finish()
});
