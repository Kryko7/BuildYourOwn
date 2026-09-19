//! Stage 57 — Raft log replication.
//!
//! The follower's half of `AppendEntries`, which is where Raft keeps its promise that two
//! logs agreeing at one index agree on everything before it. Four numbers arrive with every
//! message — a term, a previous index, that index's term and the leader's commit point —
//! and each of them refuses the message for a different reason. The log here is nothing but
//! an array of entry terms, because the terms are the whole of the consistency check.
//!
//! The oracle is §5.3 transcribed. The tester keeps the log itself, replays the consistency
//! check, the truncate-on-conflict rule and `min(leaderCommit, index of last new entry)` by
//! hand, and compares. The trap this stage exists for is the retransmit: an implementation
//! that truncates at `prevLogIndex` before appending passes every ordered test and then
//! deletes committed entries the first time a leader resends a prefix it has already sent.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 57.
pub fn stage() -> Stage {
    Stage {
        number: 57,
        slug: "raft_log_replication",
        name: "Raft log replication",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `raft-log`: a log is an array of entry terms, index 1 upwards",
            "Refuse the message when prev_index is past the end, or its term disagrees — and \
             leave the log exactly as it was",
            "Truncate only at the first entry that really conflicts; entries that already \
             match must stay, tail and all",
            "commit_index is min(leader_commit, the index of the last new entry), and it \
             never goes backwards",
        ],
        examples,
        tests: vec![
            Test::new(
                "a fresh log answers its last index and term",
                a_fresh_log_reports_itself,
            ),
            Test::new(
                "an append from an older term is refused and changes nothing",
                an_older_term_is_refused,
            ),
            Test::new(
                "an append past the end of the log names the first missing index",
                a_gap_names_the_missing_index,
            ),
            Test::new(
                "an append whose previous term disagrees is refused without truncating",
                a_failed_check_does_not_truncate,
            ),
            Test::new(
                "an append at index zero always passes the consistency check",
                index_zero_always_matches,
            ),
            Test::new(
                "a retransmitted prefix leaves the tail alone",
                a_retransmit_keeps_the_tail,
            ),
            Test::new(
                "a conflicting entry truncates exactly the suffix that disagrees",
                a_conflict_truncates_the_suffix,
            ),
            Test::new(
                "the commit index is capped by the last new entry",
                commit_is_capped_by_the_last_new_entry,
            ),
            Test::new(
                "the conflict index skips a whole term at once",
                the_conflict_index_skips_a_term,
            )
            .ext(),
            Test::new(
                "the commit index never goes backwards",
                commit_never_goes_backwards,
            )
            .ext(),
            Test::new(
                "sixty seeded appends match the tester's own replay",
                a_long_seeded_replication,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A prefix the leader has already sent", "raft-log", || {
            lines(&[
                "init [1,1,2,2]",
                "append 2 1 1 0 [1,2]",
                "log",
                "append 2 2 1 0 [3]",
                "log",
            ])
        })
        .request("a four-entry log, a retransmitted prefix, then a genuinely conflicting entry")
        .response("the first append leaves all four entries; the second leaves three")
        .note(
            "Both appends pass the consistency check at index 1 and 2, and both overlap \
             entries the follower already has. Only the second one disagrees about a term, \
             and only it is allowed to truncate. Deleting from prev_index + 1 unconditionally \
             would throw away indexes 3 and 4 on the first message, and they may already be \
             committed.",
        ),
        prim_example("A follower that is missing entries", "raft-log", || {
            lines(&["init [1,1]", "append 3 5 2 0 [3]", "append 3 2 9 0 [3]"])
        })
        .request("two appends the follower cannot accept: one past its end, one disagreeing")
        .response("success false both times, with conflict_index 3 and then 1")
        .note(
            "The conflict index is what saves the leader from backing up one entry per round \
             trip. Past the end it is the first index the follower lacks; on a term mismatch \
             it is the first index of the run holding the wrong term, so the leader skips the \
             whole term in one go.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of §5.3. Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// A follower's log and the rules that govern what may be written to it.
struct Model {
    term: i64,
    log: Vec<i64>,
    commit_index: i64,
}

/// What one `append` must answer.
struct Applied {
    success: bool,
    conflict_index: i64,
    last_index: i64,
    commit_index: i64,
}

impl Model {
    fn new(terms: &[i64]) -> Model {
        Model {
            term: terms.last().copied().unwrap_or(0),
            log: terms.to_vec(),
            commit_index: 0,
        }
    }

    fn last_index(&self) -> i64 {
        self.log.len() as i64
    }

    fn term_at(&self, index: i64) -> Option<i64> {
        if index >= 1 && index <= self.last_index() {
            self.log.get((index - 1) as usize).copied()
        } else {
            None
        }
    }

    fn run_start(&self, index: i64) -> i64 {
        let Some(t) = self.term_at(index) else {
            return index;
        };
        let mut first = index;
        while first > 1 && self.term_at(first - 1) == Some(t) {
            first -= 1;
        }
        first
    }

    fn answer(&self, success: bool, conflict_index: i64) -> Applied {
        Applied {
            success,
            conflict_index,
            last_index: self.last_index(),
            commit_index: self.commit_index,
        }
    }

    fn append(
        &mut self,
        term: i64,
        prev_index: i64,
        prev_term: i64,
        leader_commit: i64,
        entries: &[i64],
    ) -> Applied {
        if term < self.term {
            return self.answer(false, 0);
        }
        self.term = term;
        if prev_index > self.last_index() {
            return self.answer(false, self.last_index() + 1);
        }
        if prev_index >= 1 && self.term_at(prev_index) != Some(prev_term) {
            let conflict = self.run_start(prev_index);
            return self.answer(false, conflict);
        }
        for (k, entry) in entries.iter().enumerate() {
            let idx = prev_index + 1 + k as i64;
            match self.term_at(idx) {
                Some(existing) if existing == *entry => {}
                Some(_) => {
                    self.log.truncate((idx - 1) as usize);
                    self.log.push(*entry);
                }
                None => self.log.push(*entry),
            }
        }
        let last_new = prev_index + entries.len() as i64;
        if leader_commit > self.commit_index {
            self.commit_index = self.commit_index.max(leader_commit.min(last_new));
        }
        self.answer(true, 0)
    }
}

/// The `entries` argument as the command line wants it: compact JSON, no spaces.
fn entries_arg(entries: &[i64]) -> String {
    let body: Vec<String> = entries.iter().map(i64::to_string).collect();
    format!("[{}]", body.join(","))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_log_reports_itself, |ctx| {
    let p = ctx.prim("raft-log").await?;
    let empty = p.send("init []").await?;
    let seeded = p.send("init [1,1,2]").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("a log the harness handed to the program");
    c.eq(
        "init([]).last_index",
        0,
        p.expect_i64(&empty, "init []", "last_index")?,
    );
    c.eq(
        "init([]).last_term",
        0,
        p.expect_i64(&empty, "init []", "last_term")?,
    );
    c.eq(
        "init([1,1,2]).last_index",
        3,
        p.expect_i64(&seeded, "init", "last_index")?,
    );
    c.eq(
        "init([1,1,2]).last_term",
        2,
        p.expect_i64(&seeded, "init", "last_term")?,
    );
    c.eq(
        "init([1,1,2]).commit_index",
        0,
        p.expect_i64(&seeded, "init", "commit_index")?,
    );
    c.json_eq("log.terms", &serde_json::json!([1, 1, 2]), &log["terms"]);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_older_term_is_refused, |ctx| {
    let p = ctx.prim("raft-log").await?;
    p.send("init [1,1,4]").await?;
    // The log's last term is 4, so a leader still at term 2 has long been deposed.
    let stale = p.send("append 2 0 0 3 [2,2,2]").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("an append from a leader whose term has passed");
    c.eq(
        "append.success",
        false,
        p.expect_bool(&stale, "append 2 ...", "success")?,
    );
    c.eq(
        "append.term",
        4,
        p.expect_i64(&stale, "append 2 ...", "term")?,
    );
    // Nothing may move: not the entries, not the commit index, not the current term.
    c.json_eq("log.terms", &serde_json::json!([1, 1, 4]), &log["terms"]);
    c.eq(
        "log.commit_index",
        0,
        p.expect_i64(&log, "log", "commit_index")?,
    );
    c.eq("log.term", 4, p.expect_i64(&log, "log", "term")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_gap_names_the_missing_index, |ctx| {
    let p = ctx.prim("raft-log").await?;
    p.send("init [1,1]").await?;
    let gap = p.send("append 3 5 3 0 [3]").await?;
    let just_past = p.send("append 3 3 1 0 [3]").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("an append whose previous index is past the end of the log");
    c.eq(
        "append(prev 5).success",
        false,
        p.expect_bool(&gap, "append 3 5 3 0 [3]", "success")?,
    );
    c.eq(
        "append(prev 5).conflict_index",
        3,
        p.expect_i64(&gap, "append 3 5 3 0 [3]", "conflict_index")?,
    );
    c.eq(
        "append(prev 3).conflict_index",
        3,
        p.expect_i64(&just_past, "append 3 3 1 0 [3]", "conflict_index")?,
    );
    // The follower has two entries, so the first one it is missing is index 3, whether the
    // leader guessed five entries ahead or only one.
    c.json_eq("log.terms", &serde_json::json!([1, 1]), &log["terms"]);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_failed_check_does_not_truncate, |ctx| {
    let p = ctx.prim("raft-log").await?;
    p.send("init [1,1,2,2,3]").await?;
    let refused = p.send("append 3 3 9 0 [3,3]").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("an append whose previous term disagrees");
    c.eq(
        "append.success",
        false,
        p.expect_bool(&refused, "append 3 3 9 0 [3,3]", "success")?,
    );
    // A follower that truncates before checking would lose indexes 4 and 5 to a message it
    // was about to reject anyway, and those entries may already be committed elsewhere.
    c.json_eq(
        "log.terms",
        &serde_json::json!([1, 1, 2, 2, 3]),
        &log["terms"],
    );
    c.eq(
        "log.last_index",
        5,
        p.expect_i64(&log, "log", "last_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(index_zero_always_matches, |ctx| {
    let p = ctx.prim("raft-log").await?;
    p.send("init [7,7]").await?;
    // There is no entry at index 0, so its term cannot be compared with anything: the check
    // is vacuously true, and a leader starting from scratch always gets in.
    let from_scratch = p.send("append 7 0 0 0 [7,7,8]").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("an append rooted at index zero");
    c.eq(
        "append.success",
        true,
        p.expect_bool(&from_scratch, "append 7 0 0 0 [7,7,8]", "success")?,
    );
    c.json_eq("log.terms", &serde_json::json!([7, 7, 8]), &log["terms"]);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_retransmit_keeps_the_tail, |ctx| {
    let p = ctx.prim("raft-log").await?;
    p.send("init [1,1,2,2,3]").await?;
    p.send("append 3 5 3 5 []").await?;
    // Exactly the entries the follower already holds at indexes 2 and 3, resent.
    let again = p.send("append 3 1 1 0 [1,2]").await?;
    let log = p.send("log").await?;
    // And now the same prefix a second time, from a different starting point.
    p.send("append 3 0 0 0 [1,1,2]").await?;
    let after = p.send("log").await?;
    let mut c = Check::new("a leader resending a prefix the follower already has");
    c.eq(
        "append.success",
        true,
        p.expect_bool(&again, "append 3 1 1 0 [1,2]", "success")?,
    );
    // Truncating at prev_index + 1 would leave [1,1,2] and silently drop two committed
    // entries; the rule is to truncate only where a term genuinely differs.
    c.json_eq(
        "log.terms",
        &serde_json::json!([1, 1, 2, 2, 3]),
        &log["terms"],
    );
    c.json_eq(
        "log.terms after the second retransmit",
        &serde_json::json!([1, 1, 2, 2, 3]),
        &after["terms"],
    );
    c.eq(
        "log.commit_index",
        5,
        p.expect_i64(&after, "log", "commit_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_conflict_truncates_the_suffix, |ctx| {
    let p = ctx.prim("raft-log").await?;
    p.send("init [1,1,2,2,3]").await?;
    // Indexes 1 and 2 match, index 3 does not: everything from index 3 on must go.
    let conflict = p.send("append 4 0 0 0 [1,1,4]").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("an append that disagrees three entries in");
    c.eq(
        "append.success",
        true,
        p.expect_bool(&conflict, "append 4 0 0 0 [1,1,4]", "success")?,
    );
    c.json_eq("log.terms", &serde_json::json!([1, 1, 4]), &log["terms"]);
    c.eq(
        "log.last_index",
        3,
        p.expect_i64(&log, "log", "last_index")?,
    );
    c.eq("log.last_term", 4, p.expect_i64(&log, "log", "last_term")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(commit_is_capped_by_the_last_new_entry, |ctx| {
    let p = ctx.prim("raft-log").await?;
    p.send("init [1,1]").await?;
    // The leader has committed up to index 9, but this message only carries index 3. The
    // follower may not claim to have committed entries it has never seen.
    let short = p.send("append 2 2 1 9 [2]").await?;
    let heartbeat = p.send("append 2 3 2 9 []").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("a leader commit index that runs ahead of the entries sent");
    c.eq(
        "append([2]).commit_index",
        3,
        p.expect_i64(&short, "append 2 2 1 9 [2]", "commit_index")?,
    );
    // An empty append is a heartbeat: its last new entry is prev_index itself, so it still
    // cannot carry the commit point past what the follower holds.
    c.eq(
        "append([]).commit_index",
        3,
        p.expect_i64(&heartbeat, "append 2 3 2 9 []", "commit_index")?,
    );
    c.eq(
        "log.commit_index",
        3,
        p.expect_i64(&log, "log", "commit_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_conflict_index_skips_a_term, |ctx| {
    let p = ctx.prim("raft-log").await?;
    // A long run of term 2 sits between two entries of other terms.
    p.send("init [1,2,2,2,2,3]").await?;
    let middle = p.send("append 4 4 9 0 []").await?;
    let last = p.send("append 4 5 9 0 []").await?;
    let head = p.send("append 4 1 9 0 []").await?;
    let mut c = Check::new("the conflict index of a mismatch inside a run of one term");
    // Whichever index of the run the leader guessed, the follower points at the first one,
    // so the leader skips the whole term in a single round trip.
    c.eq(
        "append(prev 4).conflict_index",
        2,
        p.expect_i64(&middle, "append 4 4 9 0 []", "conflict_index")?,
    );
    c.eq(
        "append(prev 5).conflict_index",
        2,
        p.expect_i64(&last, "append 4 5 9 0 []", "conflict_index")?,
    );
    // Index 1 is a run of its own, so there is nothing to skip.
    c.eq(
        "append(prev 1).conflict_index",
        1,
        p.expect_i64(&head, "append 4 1 9 0 []", "conflict_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(commit_never_goes_backwards, |ctx| {
    let p = ctx.prim("raft-log").await?;
    p.send("init [1,1,1,1]").await?;
    let high = p.send("append 1 4 1 4 []").await?;
    // A message that overtook an earlier one carries a commit point from the past. Adopting
    // it would un-apply entries the state machine has already executed.
    let late = p.send("append 1 4 1 1 []").await?;
    let zero = p.send("append 1 2 1 0 []").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("a stale commit index arriving after a newer one");
    c.eq(
        "the first append's commit_index",
        4,
        p.expect_i64(&high, "append", "commit_index")?,
    );
    c.eq(
        "the late append's commit_index",
        4,
        p.expect_i64(&late, "append", "commit_index")?,
    );
    c.eq(
        "a zero commit index changes nothing",
        4,
        p.expect_i64(&zero, "append", "commit_index")?,
    );
    c.eq(
        "log.commit_index",
        4,
        p.expect_i64(&log, "log", "commit_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_long_seeded_replication, |ctx| {
    // The whole conversation is worked out here first — every command and the log it must
    // leave behind — by replaying §5.3 over the seeded plan. The program is then asked the
    // same questions and never consulted about the answers.
    let seed_terms = [1i64, 1, 2, 2, 3];
    let mut model = Model::new(&seed_terms);
    let mut script: Vec<(String, Applied, Vec<i64>)> = Vec::new();
    for _ in 0..60 {
        let term = (model.term + ctx.rng.random_range(-1..3)).max(0);
        let prev_index = ctx.rng.random_range(0..(model.last_index() + 3));
        let prev_term = if ctx.rng.random_bool(0.7) {
            model.term_at(prev_index).unwrap_or(0)
        } else {
            ctx.rng.random_range(0..5)
        };
        let leader_commit = ctx.rng.random_range(0..(model.last_index() + 3));
        let count = ctx.rng.random_range(0..3);
        let entries: Vec<i64> = (0..count).map(|_| ctx.rng.random_range(1..5)).collect();
        let applied = model.append(term, prev_index, prev_term, leader_commit, &entries);
        script.push((
            format!(
                "append {term} {prev_index} {prev_term} {leader_commit} {}",
                entries_arg(&entries)
            ),
            applied,
            model.log.clone(),
        ));
    }
    let seed = ctx.seed;
    let p = ctx.prim("raft-log").await?;
    p.send(&format!("init {}", entries_arg(&seed_terms)))
        .await?;
    let mut actual = Vec::with_capacity(script.len());
    for (command, ..) in &script {
        let r = p.send(command).await?;
        let log = p.send("log").await?;
        actual.push((
            Applied {
                success: p.expect_bool(&r, command, "success")?,
                conflict_index: p.expect_i64(&r, command, "conflict_index")?,
                last_index: p.expect_i64(&r, command, "last_index")?,
                commit_index: p.expect_i64(&r, command, "commit_index")?,
            },
            log["terms"].clone(),
        ));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("sixty seeded appends replayed against the tester's own log");
    c.note(format!("seed {seed}, {} appends", script.len()));
    for (i, ((command, want, log), (got, got_log))) in script.iter().zip(&actual).enumerate() {
        c.eq(
            &format!("step[{i}] {command} → success"),
            want.success,
            got.success,
        );
        c.eq(
            &format!("step[{i}] {command} → conflict_index"),
            want.conflict_index,
            got.conflict_index,
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
        c.json_eq(
            &format!("step[{i}] {command} → log.terms"),
            &serde_json::json!(log),
            got_log,
        );
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
