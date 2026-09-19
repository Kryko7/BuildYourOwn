//! Stage 59 — Raft snapshots and log compaction.
//!
//! A log that only grows is a log that eventually fills the disk, so Raft lets a server
//! throw away a committed prefix and remember it as one snapshot instead. The cost is that
//! the log no longer starts at index 1, and every question that used to be answered by
//! looking an entry up — the `AppendEntries` consistency check above all — now has a
//! boundary it can fall off. This stage is about that boundary, and about `InstallSnapshot`,
//! the message a leader sends when a follower has fallen behind the entries it still holds.
//!
//! The oracle is §7 transcribed. The tester keeps the surviving entries, the last included
//! index and its term, and works out for itself which of the three things an incoming
//! snapshot does: nothing, because it is older than what this server has already committed;
//! keep the tail, because the server already holds the entry at the boundary; or replace
//! everything, because it does not. Compacting past the commit index is the trap the stage
//! exists for: an entry that is not yet committed may still be truncated by a future
//! leader, and once it is inside a snapshot there is no way to take it back out.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 59.
pub fn stage() -> Stage {
    Stage {
        number: 59,
        slug: "raft_snapshots",
        name: "Raft snapshots and log compaction",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `raft-snapshot`: the log no longer starts at index 1, so keep first_index, \
             snapshot_index and snapshot_term",
            "Never compact past the commit index: an uncommitted entry may still be truncated",
            "The snapshot boundary still answers a consistency check, so keep its term; an \
             append that reaches below the boundary can only be refused",
            "An InstallSnapshot the follower already covers is ignored; one whose boundary it \
             holds keeps the tail; anything else replaces the whole log",
        ],
        examples,
        tests: vec![
            Test::new("a fresh log has no snapshot", a_fresh_log_has_no_snapshot),
            Test::new(
                "compacting past the commit index is refused",
                compaction_stops_at_the_commit_index,
            ),
            Test::new(
                "a snapshot discards the entries it covers and keeps their boundary term",
                a_snapshot_discards_its_prefix,
            ),
            Test::new(
                "the snapshot boundary still answers a consistency check",
                the_boundary_still_answers,
            ),
            Test::new(
                "an append that reaches below the boundary is refused as compacted",
                an_append_below_the_boundary_is_refused,
            ),
            Test::new(
                "taking the same snapshot twice changes nothing",
                a_snapshot_is_idempotent,
            ),
            Test::new(
                "a snapshot from behind the commit index is ignored",
                a_stale_snapshot_is_ignored,
            ),
            Test::new(
                "a snapshot whose boundary the follower holds keeps the tail",
                a_matching_snapshot_retains_the_tail,
            ),
            Test::new(
                "a snapshot from ahead replaces the whole log",
                a_snapshot_from_ahead_discards_everything,
            ),
            Test::new(
                "compaction never changes what any surviving index holds",
                compaction_preserves_every_surviving_entry,
            )
            .ext(),
            Test::new(
                "sixty seeded commits, snapshots and installs match the tester's own replay",
                a_long_seeded_compaction,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Compacting a committed prefix", "raft-snapshot", || {
            lines(&[
                "init [1,1,2,2,3]",
                "snapshot 3",
                "commit 3",
                "snapshot 3",
                "log",
                "append 3 3 2 []",
                "append 3 2 1 []",
            ])
        })
        .request("a five-entry log compacted before and after its commit index reaches index 3")
        .response("the first snapshot is refused; the second leaves two entries and a boundary")
        .note(
            "The refusal is the point. An entry that is only replicated, not committed, may \
             still be truncated by a future leader — and an entry inside a snapshot cannot \
             be truncated, because there is nothing left to truncate. Note that the boundary \
             at index 3 still answers an append, and that index 2 can no longer be checked.",
        ),
        prim_example("Three kinds of InstallSnapshot", "raft-snapshot", || {
            lines(&[
                "init [1,1,2,2,3]",
                "commit 4",
                "install 9 2 1",
                "install 9 4 2",
                "log",
                "install 9 20 9",
                "log",
            ])
        })
        .request("snapshots from behind, from a boundary the follower holds, and from far ahead")
        .response("stale, retained and discarded, in that order")
        .note(
            "A follower that throws its log away every time a snapshot arrives is correct but \
             wasteful: when it already holds the entry at the snapshot's last included index, \
             the Log Matching Property says everything after it is good too, and refetching \
             that tail can cost a great deal of bandwidth on a large state.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of §7. Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// The shape a compacted log reports: where it starts, where it ends, and its boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shape {
    first_index: i64,
    last_index: i64,
    commit_index: i64,
    snapshot_index: i64,
    snapshot_term: i64,
    terms: Vec<i64>,
}

/// A log with a compacted prefix: `log` holds only the entries that survived.
struct Model {
    log: Vec<i64>,
    snapshot_index: i64,
    snapshot_term: i64,
    commit_index: i64,
}

impl Model {
    fn new(terms: &[i64]) -> Model {
        Model {
            log: terms.to_vec(),
            snapshot_index: 0,
            snapshot_term: 0,
            commit_index: 0,
        }
    }

    fn first_index(&self) -> i64 {
        self.snapshot_index + 1
    }

    fn last_index(&self) -> i64 {
        self.snapshot_index + self.log.len() as i64
    }

    fn term_at(&self, index: i64) -> Option<i64> {
        if index == self.snapshot_index {
            return Some(self.snapshot_term);
        }
        if index >= self.first_index() && index <= self.last_index() {
            return self.log.get((index - self.first_index()) as usize).copied();
        }
        None
    }

    fn shape(&self) -> Shape {
        Shape {
            first_index: self.first_index(),
            last_index: self.last_index(),
            commit_index: self.commit_index,
            snapshot_index: self.snapshot_index,
            snapshot_term: self.snapshot_term,
            terms: self.log.clone(),
        }
    }

    fn commit(&mut self, index: i64) {
        let wanted = index.min(self.last_index());
        if wanted > self.commit_index {
            self.commit_index = wanted;
        }
    }

    /// Returns whether the snapshot was taken.
    fn snapshot(&mut self, index: i64) -> bool {
        if index > self.commit_index {
            return false;
        }
        if index > self.snapshot_index {
            let term = self.term_at(index).unwrap_or(self.snapshot_term);
            let drop = (index - self.snapshot_index) as usize;
            self.log.drain(..drop.min(self.log.len()));
            self.snapshot_index = index;
            self.snapshot_term = term;
        }
        true
    }

    /// Returns what the server must answer as `action`.
    fn install(&mut self, last_included_index: i64, last_included_term: i64) -> &'static str {
        if last_included_index <= self.commit_index {
            return "stale";
        }
        let matching = last_included_index >= self.first_index()
            && self.term_at(last_included_index) == Some(last_included_term);
        let action = if matching {
            let keep = (last_included_index - self.snapshot_index) as usize;
            self.log.drain(..keep.min(self.log.len()));
            "retained"
        } else {
            self.log.clear();
            "discarded"
        };
        self.snapshot_index = last_included_index;
        self.snapshot_term = last_included_term;
        self.commit_index = self.commit_index.max(last_included_index);
        action
    }
}

/// A list of terms as the command line wants it: compact JSON, no spaces.
fn terms_arg(terms: &[i64]) -> String {
    let body: Vec<String> = terms.iter().map(i64::to_string).collect();
    format!("[{}]", body.join(","))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_log_has_no_snapshot, |ctx| {
    let p = ctx.prim("raft-snapshot").await?;
    let init = p.send("init [1,1,2]").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("a log nothing has been compacted out of yet");
    // Index 0 is the imaginary entry before the log, and its term is 0: exactly the values
    // an uncompacted log reports, so no code path needs a special case for "no snapshot".
    c.eq(
        "init.first_index",
        1,
        p.expect_i64(&init, "init", "first_index")?,
    );
    c.eq(
        "init.last_index",
        3,
        p.expect_i64(&init, "init", "last_index")?,
    );
    c.eq(
        "init.snapshot_index",
        0,
        p.expect_i64(&init, "init", "snapshot_index")?,
    );
    c.eq(
        "init.snapshot_term",
        0,
        p.expect_i64(&init, "init", "snapshot_term")?,
    );
    c.eq(
        "init.commit_index",
        0,
        p.expect_i64(&init, "init", "commit_index")?,
    );
    c.json_eq("log.terms", &serde_json::json!([1, 1, 2]), &log["terms"]);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(compaction_stops_at_the_commit_index, |ctx| {
    let p = ctx.prim("raft-snapshot").await?;
    p.send("init [1,1,2,2,3]").await?;
    let too_early = p.send("snapshot 1").await?;
    p.send("commit 2").await?;
    let past_commit = p.send("snapshot 3").await?;
    let allowed = p.send("snapshot 2").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("compaction against the commit index");
    // Nothing is committed yet, so nothing may be compacted — not even index 1.
    c.eq(
        "snapshot(1).ok before any commit",
        false,
        p.expect_bool(&too_early, "snapshot 1", "ok")?,
    );
    c.eq(
        "snapshot(3).ok with commit_index 2",
        false,
        p.expect_bool(&past_commit, "snapshot 3", "ok")?,
    );
    c.eq(
        "snapshot(2).ok",
        true,
        p.expect_bool(&allowed, "snapshot 2", "ok")?,
    );
    c.eq(
        "log.first_index",
        3,
        p.expect_i64(&log, "log", "first_index")?,
    );
    c.json_eq("log.terms", &serde_json::json!([2, 2, 3]), &log["terms"]);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_snapshot_discards_its_prefix, |ctx| {
    let p = ctx.prim("raft-snapshot").await?;
    p.send("init [1,1,2,2,3]").await?;
    p.send("commit 5").await?;
    let taken = p.send("snapshot 3").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("what a snapshot leaves behind");
    c.eq(
        "snapshot.snapshot_index",
        3,
        p.expect_i64(&taken, "snapshot 3", "snapshot_index")?,
    );
    // The boundary's term has to outlive the entry itself, or the next consistency check
    // against index 3 has nothing to compare.
    c.eq(
        "snapshot.snapshot_term",
        2,
        p.expect_i64(&taken, "snapshot 3", "snapshot_term")?,
    );
    c.eq(
        "snapshot.first_index",
        4,
        p.expect_i64(&taken, "snapshot 3", "first_index")?,
    );
    c.eq(
        "snapshot.log_len",
        2,
        p.expect_i64(&taken, "snapshot 3", "log_len")?,
    );
    c.json_eq("log.terms", &serde_json::json!([2, 3]), &log["terms"]);
    c.eq(
        "log.last_index",
        5,
        p.expect_i64(&log, "log", "last_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_boundary_still_answers, |ctx| {
    let p = ctx.prim("raft-snapshot").await?;
    p.send("init [1,1,2,2,3]").await?;
    p.send("commit 5").await?;
    p.send("snapshot 3").await?;
    // The leader's next message is rooted exactly at the boundary, which is the common case
    // right after a compaction.
    let matched = p.send("append 3 3 2 [2,3,4]").await?;
    let log = p.send("log").await?;
    let wrong_term = p.send("append 4 3 9 []").await?;
    let mut c = Check::new("an append rooted at the last included index");
    c.eq(
        "append(prev 3, term 2).success",
        true,
        p.expect_bool(&matched, "append 3 3 2 ...", "success")?,
    );
    c.eq(
        "append.last_index",
        6,
        p.expect_i64(&matched, "append", "last_index")?,
    );
    c.json_eq("log.terms", &serde_json::json!([2, 3, 4]), &log["terms"]);
    // The boundary is checked like any other entry: a leader that disagrees about its term
    // is refused, not humoured.
    c.eq(
        "append(prev 3, term 9).success",
        false,
        p.expect_bool(&wrong_term, "append 4 3 9 []", "success")?,
    );
    c.eq(
        "append(prev 3, term 9).reason",
        "term mismatch".to_string(),
        p.expect_str(&wrong_term, "append 4 3 9 []", "reason")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_append_below_the_boundary_is_refused, |ctx| {
    let p = ctx.prim("raft-snapshot").await?;
    p.send("init [1,1,2,2,3]").await?;
    p.send("commit 5").await?;
    p.send("snapshot 3").await?;
    let below = p.send("append 3 2 1 [2]").await?;
    let far_below = p.send("append 3 0 0 [1,1,2]").await?;
    let past_the_end = p.send("append 3 9 3 []").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("appends the follower cannot check any more");
    // The entry at index 2 is inside the snapshot, so its term is gone and the check cannot
    // be performed at all. Guessing "yes" here would let a stale leader rewrite history.
    c.eq(
        "append(prev 2).success",
        false,
        p.expect_bool(&below, "append 3 2 1 [2]", "success")?,
    );
    c.eq(
        "append(prev 2).reason",
        "compacted".to_string(),
        p.expect_str(&below, "append 3 2 1 [2]", "reason")?,
    );
    c.eq(
        "append(prev 0).reason",
        "compacted".to_string(),
        p.expect_str(&far_below, "append 3 0 0 [1,1,2]", "reason")?,
    );
    c.eq(
        "append(prev 9).reason",
        "missing".to_string(),
        p.expect_str(&past_the_end, "append 3 9 3 []", "reason")?,
    );
    c.json_eq("log.terms", &serde_json::json!([2, 3]), &log["terms"]);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_snapshot_is_idempotent, |ctx| {
    let p = ctx.prim("raft-snapshot").await?;
    p.send("init [1,1,2,2,3]").await?;
    p.send("commit 5").await?;
    let first = p.send("snapshot 3").await?;
    let second = p.send("snapshot 3").await?;
    // An older snapshot request that arrived late must not undo the newer one.
    let older = p.send("snapshot 1").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("the same snapshot taken three times");
    c.eq(
        "the first snapshot's first_index",
        4,
        p.expect_i64(&first, "snapshot 3", "first_index")?,
    );
    c.eq(
        "the second snapshot's first_index",
        4,
        p.expect_i64(&second, "snapshot 3", "first_index")?,
    );
    c.eq(
        "snapshot(1).ok",
        true,
        p.expect_bool(&older, "snapshot 1", "ok")?,
    );
    c.eq(
        "snapshot(1).snapshot_index",
        3,
        p.expect_i64(&older, "snapshot 1", "snapshot_index")?,
    );
    c.json_eq("log.terms", &serde_json::json!([2, 3]), &log["terms"]);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_stale_snapshot_is_ignored, |ctx| {
    let p = ctx.prim("raft-snapshot").await?;
    p.send("init [1,1,2,2,3]").await?;
    p.send("commit 4").await?;
    let stale = p.send("install 9 2 1").await?;
    let at_commit = p.send("install 9 4 2").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("an InstallSnapshot from behind this server's commit point");
    c.eq(
        "install(2).action",
        "stale".to_string(),
        p.expect_str(&stale, "install 9 2 1", "action")?,
    );
    c.eq(
        "install(2).ok",
        false,
        p.expect_bool(&stale, "install 9 2 1", "ok")?,
    );
    // Applying it would throw away indexes 3 and 4, which this server has already applied
    // to its state machine and answered reads from.
    c.eq(
        "install(4).action",
        "stale".to_string(),
        p.expect_str(&at_commit, "install 9 4 2", "action")?,
    );
    c.json_eq(
        "log.terms",
        &serde_json::json!([1, 1, 2, 2, 3]),
        &log["terms"],
    );
    c.eq(
        "log.commit_index",
        4,
        p.expect_i64(&log, "log", "commit_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_matching_snapshot_retains_the_tail, |ctx| {
    let p = ctx.prim("raft-snapshot").await?;
    p.send("init [1,1,2,2,3]").await?;
    p.send("commit 2").await?;
    // The follower holds index 4 at term 2, which is exactly what the snapshot claims.
    let retained = p.send("install 9 4 2").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("an InstallSnapshot whose boundary the follower already holds");
    c.eq(
        "install.action",
        "retained".to_string(),
        p.expect_str(&retained, "install 9 4 2", "action")?,
    );
    c.eq(
        "install.ok",
        true,
        p.expect_bool(&retained, "install 9 4 2", "ok")?,
    );
    c.eq(
        "install.first_index",
        5,
        p.expect_i64(&retained, "install 9 4 2", "first_index")?,
    );
    // Index 5 survives: Log Matching says that if index 4 agrees, everything before it does
    // too, so the tail this server holds is as good as the leader's.
    c.eq(
        "install.last_index",
        5,
        p.expect_i64(&retained, "install 9 4 2", "last_index")?,
    );
    c.json_eq("log.terms", &serde_json::json!([3]), &log["terms"]);
    c.eq(
        "log.commit_index",
        4,
        p.expect_i64(&log, "log", "commit_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_snapshot_from_ahead_discards_everything, |ctx| {
    let p = ctx.prim("raft-snapshot").await?;
    p.send("init [1,1,2]").await?;
    let far_ahead = p.send("install 9 20 7").await?;
    let log = p.send("log").await?;
    // And the other reason to discard: the boundary is inside this server's log, but at a
    // term it disagrees about, so nothing after it can be trusted either.
    p.send("init [1,1,2,2,3]").await?;
    let wrong_term = p.send("install 9 4 9").await?;
    let after = p.send("log").await?;
    let mut c = Check::new("an InstallSnapshot this server cannot reconcile with its log");
    c.eq(
        "install(20).action",
        "discarded".to_string(),
        p.expect_str(&far_ahead, "install 9 20 7", "action")?,
    );
    c.eq(
        "install(20).first_index",
        21,
        p.expect_i64(&far_ahead, "install", "first_index")?,
    );
    c.eq(
        "install(20).last_index",
        20,
        p.expect_i64(&far_ahead, "install", "last_index")?,
    );
    c.eq(
        "install(20).commit_index",
        20,
        p.expect_i64(&far_ahead, "install", "commit_index")?,
    );
    c.json_eq("log.terms", &serde_json::json!([]), &log["terms"]);
    c.eq(
        "install(4, term 9).action",
        "discarded".to_string(),
        p.expect_str(&wrong_term, "install 9 4 9", "action")?,
    );
    c.json_eq(
        "log.terms after the mismatch",
        &serde_json::json!([]),
        &after["terms"],
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(compaction_preserves_every_surviving_entry, |ctx| {
    // Compaction is allowed to make an index unanswerable; it is never allowed to change
    // the answer. Compact the same log one index at a time and check every entry that is
    // still in reach against the log the harness started from.
    let terms = [1i64, 1, 2, 2, 3, 3, 4];
    let p = ctx.prim("raft-snapshot").await?;
    let mut seen: Vec<(i64, Vec<i64>, i64, i64)> = Vec::new();
    p.send(&format!("init {}", terms_arg(&terms))).await?;
    p.send(&format!("commit {}", terms.len())).await?;
    for boundary in 1..=terms.len() as i64 {
        p.send(&format!("snapshot {boundary}")).await?;
        let log = p.send("log").await?;
        let survivors: Vec<i64> = log["terms"]
            .as_array()
            .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect())
            .unwrap_or_default();
        seen.push((
            boundary,
            survivors,
            p.expect_i64(&log, "log", "first_index")?,
            p.expect_i64(&log, "log", "snapshot_term")?,
        ));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("every entry that survived each of seven compactions");
    for (boundary, survivors, first_index, snapshot_term) in &seen {
        let expected: Vec<i64> = terms[*boundary as usize..].to_vec();
        c.eq(
            &format!("after snapshot {boundary}, log.terms"),
            expected,
            survivors.clone(),
        );
        c.eq(
            &format!("after snapshot {boundary}, log.first_index"),
            boundary + 1,
            *first_index,
        );
        c.eq(
            &format!("after snapshot {boundary}, log.snapshot_term"),
            terms[(*boundary - 1) as usize],
            *snapshot_term,
        );
    }
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_long_seeded_compaction, |ctx| {
    // The whole conversation is worked out here first — every command and the shape it must
    // leave behind — by replaying §7 over the seeded plan. The program is then asked the
    // same questions and never consulted about the answers.
    let seed_terms = [1i64, 1, 2, 2, 3, 3, 4, 4];
    let mut model = Model::new(&seed_terms);
    let mut script: Vec<(String, Shape)> = Vec::new();
    for _ in 0..60 {
        let command = match ctx.rng.random_range(0..3) {
            0 => {
                let index = ctx.rng.random_range(0..(model.last_index() + 3));
                model.commit(index);
                format!("commit {index}")
            }
            1 => {
                let index = ctx.rng.random_range(0..(model.last_index() + 2));
                model.snapshot(index);
                format!("snapshot {index}")
            }
            _ => {
                let index = ctx.rng.random_range(0..(model.last_index() + 4));
                let term = ctx.rng.random_range(1..6);
                model.install(index, term);
                format!("install 9 {index} {term}")
            }
        };
        script.push((command, model.shape()));
    }
    let seed = ctx.seed;
    let p = ctx.prim("raft-snapshot").await?;
    p.send(&format!("init {}", terms_arg(&seed_terms))).await?;
    let mut actual = Vec::with_capacity(script.len());
    for (command, _) in &script {
        p.send(command).await?;
        let log = p.send("log").await?;
        let terms: Vec<i64> = log["terms"]
            .as_array()
            .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect())
            .unwrap_or_default();
        actual.push(Shape {
            first_index: p.expect_i64(&log, "log", "first_index")?,
            last_index: p.expect_i64(&log, "log", "last_index")?,
            commit_index: p.expect_i64(&log, "log", "commit_index")?,
            snapshot_index: p.expect_i64(&log, "log", "snapshot_index")?,
            snapshot_term: p.expect_i64(&log, "log", "snapshot_term")?,
            terms,
        });
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("sixty seeded commits, snapshots and installs replayed against §7");
    c.note(format!("seed {seed}, {} events", script.len()));
    for (i, ((command, want), got)) in script.iter().zip(&actual).enumerate() {
        c.eq(
            &format!("step[{i}] {command} → first_index"),
            want.first_index,
            got.first_index,
        );
        c.eq(
            &format!("step[{i}] {command} → last_index"),
            want.last_index,
            got.last_index,
        );
        c.eq(
            &format!("step[{i}] {command} → commit_index"),
            want.commit_index,
            got.commit_index,
        );
        c.eq(
            &format!("step[{i}] {command} → snapshot_index"),
            want.snapshot_index,
            got.snapshot_index,
        );
        c.eq(
            &format!("step[{i}] {command} → snapshot_term"),
            want.snapshot_term,
            got.snapshot_term,
        );
        c.eq(
            &format!("step[{i}] {command} → log.terms"),
            want.terms.clone(),
            got.terms.clone(),
        );
        // The invariant that makes a compacted log usable at all.
        c.at_least(
            &format!("step[{i}] {command} → first_index <= last_index + 1"),
            got.first_index,
            got.last_index + 1,
        );
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
