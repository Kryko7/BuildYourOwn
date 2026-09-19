//! Stage 58 — Raft commitment and the Figure 8 trap.
//!
//! The leader's half of replication: one `matchIndex` and one `nextIndex` per follower, and
//! the rule that turns a count of replicas into a commitment. Counting is the easy part.
//! The hard part is §5.4.2, which says that a majority holding an entry is *not* enough
//! when that entry came from an earlier term — the situation the paper draws as Figure 8,
//! where a leader that commits on replica count alone can watch the entry be overwritten.
//!
//! The oracle is the paper's own commitment rule, transcribed: the tester keeps the log,
//! the match indexes and the current term, works out the highest index a majority holds
//! from the leader's own term, and compares. Figure 8 has a test of its own, written out
//! step by step, because an implementation that stops at "a majority has it" passes every
//! other test in this stage and violates State Machine Safety the first time a leader
//! inherits entries from a predecessor.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use std::collections::BTreeMap;

/// Stage 58.
pub fn stage() -> Stage {
    Stage {
        number: 58,
        slug: "raft_commit_safety",
        name: "Raft commitment and the Figure 8 trap",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `raft-commit`: matchIndex and nextIndex per follower, and one commit index",
            "The leader counts itself, so a five-member cluster commits on two acknowledgements",
            "Advance the commit index only to an entry of the leader's *own* term — §5.4.2, the \
             Figure 8 trap — and let it carry the earlier entries with it",
            "An acknowledgement that arrived late must never lower a match index",
        ],
        examples,
        tests: vec![
            Test::new(
                "a fresh leader has replicated nothing",
                a_fresh_leader_has_replicated_nothing,
            ),
            Test::new(
                "a majority of acknowledgements commits an entry of the current term",
                a_majority_commits_a_current_term_entry,
            ),
            Test::new(
                "the leader counts itself towards the majority",
                the_leader_counts_itself,
            ),
            Test::new(
                "a rejection backs the next index up to the conflict index",
                a_rejection_backs_up,
            ),
            Test::new(
                "a rejection never backs the next index below one",
                a_rejection_stops_at_one,
            ),
            Test::new(
                "a majority alone does not commit an entry from an earlier term",
                figure_eight_refuses_to_commit,
            ),
            Test::new(
                "committing a current-term entry commits the earlier one behind it",
                figure_eight_commits_indirectly,
            ),
            Test::new(
                "commit-safe names the reason it refuses",
                commit_safe_names_its_reason,
            ),
            Test::new(
                "a late acknowledgement never lowers a match index",
                a_late_ack_never_lowers_a_match_index,
            )
            .ext(),
            Test::new(
                "a committed index keeps its term whatever arrives afterwards",
                a_committed_entry_is_fixed,
            )
            .ext(),
            Test::new(
                "eighty seeded acknowledgements match the tester's own replay",
                a_long_seeded_commitment,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Figure 8, in five commands", "raft-commit", || {
            lines(&[
                "init 5 4 [1,2]",
                "ack s2 2",
                "ack s3 2",
                "commit-safe 2",
                "append",
                "ack s2 3",
                "ack s3 3",
                "state",
            ])
        })
        .request("a leader at term 4 whose log ends with an entry from term 2, then a majority")
        .response("commit_index stays 0 while three of five hold index 2; it jumps to 3 at the end")
        .note(
            "Three of five members hold index 2, and it is still not committed. If the \
             leader committed it here and then crashed, a candidate that never saw index 2 \
             could still be elected — its log is as up to date as anyone's at term 2 — and \
             would overwrite a committed entry. Appending an entry of the leader's own term \
             and committing *that* carries index 2 along with it, safely.",
        ),
        prim_example("A follower that has to be backed up", "raft-commit", || {
            lines(&[
                "init 3 5 [1,1,2,2,5]",
                "state",
                "reject s2 3",
                "reject s2 0",
                "ack s2 5",
                "ack s2 2",
                "state",
            ])
        })
        .request("rejections that back nextIndex up, then acknowledgements that move it on")
        .response("nextIndex falls to 3, then 1, and matchIndex rises to 5 and stays there")
        .note(
            "The last acknowledgement carries an older index because it overtook the newer \
             one on the way back. Taking it at face value would un-replicate two entries the \
             follower demonstrably holds, and could pull the commit index backwards with it.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of §5.3 and §5.4.2. Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// A leader's view of its followers, and the rule that advances the commit index.
struct Model {
    members: usize,
    term: i64,
    log: Vec<i64>,
    commit_index: i64,
    match_index: BTreeMap<String, i64>,
    next_index: BTreeMap<String, i64>,
}

impl Model {
    fn new(members: usize, term: i64, log: &[i64]) -> Model {
        let mut match_index = BTreeMap::new();
        let mut next_index = BTreeMap::new();
        for i in 2..=members {
            match_index.insert(format!("s{i}"), 0);
            // A new leader guesses optimistically that every follower is already caught up.
            next_index.insert(format!("s{i}"), log.len() as i64 + 1);
        }
        Model {
            members,
            term,
            log: log.to_vec(),
            commit_index: 0,
            match_index,
            next_index,
        }
    }

    fn majority(&self) -> usize {
        self.members / 2 + 1
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

    /// How many members hold the entry at `index`; the leader is one of them.
    fn replicas(&self, index: i64) -> usize {
        1 + self.match_index.values().filter(|m| **m >= index).count()
    }

    fn advance(&mut self) {
        let mut n = self.last_index();
        while n > self.commit_index {
            if self.replicas(n) >= self.majority() && self.term_at(n) == Some(self.term) {
                self.commit_index = n;
                return;
            }
            n -= 1;
        }
    }

    fn append(&mut self) {
        self.log.push(self.term);
    }

    /// Returns `(match_index, next_index)` for the peer.
    fn ack(&mut self, peer: &str, index: i64) -> (i64, i64) {
        let matched = {
            let slot = self.match_index.entry(peer.to_string()).or_insert(0);
            *slot = (*slot).max(index);
            *slot
        };
        let next = {
            let slot = self.next_index.entry(peer.to_string()).or_insert(1);
            *slot = (*slot).max(matched + 1);
            *slot
        };
        self.advance();
        (matched, next)
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

dist_test!(a_fresh_leader_has_replicated_nothing, |ctx| {
    let p = ctx.prim("raft-commit").await?;
    let init = p.send("init 5 4 [1,2]").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("a leader that has just taken office");
    c.eq(
        "init.term",
        4,
        p.expect_i64(&init, "init 5 4 [1,2]", "term")?,
    );
    c.eq(
        "init.last_index",
        2,
        p.expect_i64(&init, "init", "last_index")?,
    );
    c.eq(
        "init.commit_index",
        0,
        p.expect_i64(&init, "init", "commit_index")?,
    );
    c.eq("init.majority", 3, p.expect_i64(&init, "init", "majority")?);
    // Raft's own rule: nextIndex starts optimistically at the end of the leader's log, and
    // matchIndex pessimistically at nothing, because the leader knows neither yet.
    c.json_eq(
        "state.match",
        &serde_json::json!({"s2": 0, "s3": 0, "s4": 0, "s5": 0}),
        &state["match"],
    );
    c.json_eq(
        "state.next",
        &serde_json::json!({"s2": 3, "s3": 3, "s4": 3, "s5": 3}),
        &state["next"],
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_majority_commits_a_current_term_entry, |ctx| {
    let p = ctx.prim("raft-commit").await?;
    p.send("init 3 2 [1,2]").await?;
    let first = p.send("ack s2 2").await?;
    let safe = p.send("commit-safe 2").await?;
    let mut c = Check::new("an entry of the leader's own term held by a majority");
    c.eq(
        "ack.match_index",
        2,
        p.expect_i64(&first, "ack s2 2", "match_index")?,
    );
    c.eq(
        "ack.next_index",
        3,
        p.expect_i64(&first, "ack s2 2", "next_index")?,
    );
    // Two of three: the leader and s2. Index 2 carries term 2, which is the leader's own.
    c.eq(
        "ack.commit_index",
        2,
        p.expect_i64(&first, "ack s2 2", "commit_index")?,
    );
    c.eq(
        "commit-safe(2).safe",
        true,
        p.expect_bool(&safe, "commit-safe 2", "safe")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_leader_counts_itself, |ctx| {
    let p = ctx.prim("raft-commit").await?;
    p.send("init 5 1 [1]").await?;
    let one = p.send("ack s2 1").await?;
    let two = p.send("ack s3 1").await?;
    let mut c = Check::new("how many acknowledgements a five-member cluster needs");
    // One acknowledgement makes two replicas, which is short of three.
    c.eq(
        "after one ack, commit_index",
        0,
        p.expect_i64(&one, "ack s2 1", "commit_index")?,
    );
    c.eq(
        "after two acks, commit_index",
        1,
        p.expect_i64(&two, "ack s3 1", "commit_index")?,
    );
    // The same entry in a three-member cluster needs only one acknowledgement.
    p.send("init 3 1 [1]").await?;
    let small = p.send("ack s2 1").await?;
    c.eq(
        "in a three-member cluster, one ack commits",
        1,
        p.expect_i64(&small, "ack s2 1", "commit_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_rejection_backs_up, |ctx| {
    let p = ctx.prim("raft-commit").await?;
    p.send("init 3 1 [1,1,1]").await?;
    let before = p.send("state").await?;
    let rejected = p.send("reject s2 2").await?;
    let after = p.send("state").await?;
    let mut c = Check::new("a follower that refused the leader's guess");
    c.json_eq(
        "state.next before the rejection",
        &serde_json::json!({"s2": 4, "s3": 4}),
        &before["next"],
    );
    c.eq(
        "reject.next_index",
        2,
        p.expect_i64(&rejected, "reject s2 2", "next_index")?,
    );
    // Only the rejecting follower moves; the others are still where the leader left them.
    c.json_eq(
        "state.next after the rejection",
        &serde_json::json!({"s2": 2, "s3": 4}),
        &after["next"],
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_rejection_stops_at_one, |ctx| {
    let p = ctx.prim("raft-commit").await?;
    p.send("init 3 1 [1,1]").await?;
    let zero = p.send("reject s2 0").await?;
    let negative = p.send("reject s2 -4").await?;
    let mut c = Check::new("a rejection that would take the next index off the front of the log");
    // Index 0 is the imaginary entry before the log starts; a nextIndex of 0 would make the
    // leader send a previous index of -1, which nothing can check.
    c.eq(
        "reject(0).next_index",
        1,
        p.expect_i64(&zero, "reject s2 0", "next_index")?,
    );
    c.eq(
        "reject(-4).next_index",
        1,
        p.expect_i64(&negative, "reject s2 -4", "next_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(figure_eight_refuses_to_commit, |ctx| {
    let p = ctx.prim("raft-commit").await?;
    // A leader elected at term 4 whose log ends with index 2, an entry it inherited from
    // term 2. This is the moment Figure 8 of the Raft paper is drawn at.
    p.send("init 5 4 [1,2]").await?;
    let one = p.send("ack s2 2").await?;
    let two = p.send("ack s3 2").await?;
    let safe = p.send("commit-safe 2").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("a majority holding an entry from a term before the leader's");
    c.eq(
        "after one ack, commit_index",
        0,
        p.expect_i64(&one, "ack s2 2", "commit_index")?,
    );
    // Three of five hold index 2 and it is still not committed. Committing it here and
    // crashing would let a candidate that never saw index 2 win the next election — its log
    // is as up to date as anybody's at term 2 — and overwrite an entry already applied.
    c.eq(
        "after two acks, commit_index",
        0,
        p.expect_i64(&two, "ack s3 2", "commit_index")?,
    );
    c.eq(
        "commit-safe(2).replicas",
        3,
        p.expect_i64(&safe, "commit-safe 2", "replicas")?,
    );
    c.eq(
        "commit-safe(2).safe",
        false,
        p.expect_bool(&safe, "commit-safe 2", "safe")?,
    );
    c.eq(
        "commit-safe(2).reason",
        "earlier term".to_string(),
        p.expect_str(&safe, "commit-safe 2", "reason")?,
    );
    c.eq(
        "state.commit_index",
        0,
        p.expect_i64(&state, "state", "commit_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(figure_eight_commits_indirectly, |ctx| {
    let p = ctx.prim("raft-commit").await?;
    p.send("init 5 4 [1,2]").await?;
    p.send("ack s2 2").await?;
    p.send("ack s3 2").await?;
    // The way out of Figure 8: append an entry of the leader's own term and replicate that.
    let appended = p.send("append").await?;
    let one = p.send("ack s2 3").await?;
    let two = p.send("ack s3 3").await?;
    let old = p.send("commit-safe 2").await?;
    let entry = p.send("entry 3").await?;
    let mut c = Check::new("the entry that rescues the one behind it");
    c.eq(
        "append.last_index",
        3,
        p.expect_i64(&appended, "append", "last_index")?,
    );
    c.eq("append.term", 4, p.expect_i64(&appended, "append", "term")?);
    c.eq("entry(3).term", 4, p.expect_i64(&entry, "entry 3", "term")?);
    c.eq(
        "after one ack, commit_index",
        0,
        p.expect_i64(&one, "ack s2 3", "commit_index")?,
    );
    // Index 3 belongs to term 4, so a majority holding it commits it — and everything before
    // it, index 2 included, is committed along with it by the Log Matching Property.
    c.eq(
        "after two acks, commit_index",
        3,
        p.expect_i64(&two, "ack s3 3", "commit_index")?,
    );
    c.eq(
        "commit-safe(2).safe",
        true,
        p.expect_bool(&old, "commit-safe 2", "safe")?,
    );
    c.eq(
        "commit-safe(2).reason",
        "already committed".to_string(),
        p.expect_str(&old, "commit-safe 2", "reason")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(commit_safe_names_its_reason, |ctx| {
    let p = ctx.prim("raft-commit").await?;
    p.send("init 5 4 [1,2,4]").await?;
    let past = p.send("commit-safe 9").await?;
    let empty = p.send("commit-safe 0").await?;
    let lonely = p.send("commit-safe 3").await?;
    p.send("ack s2 3").await?;
    p.send("ack s3 3").await?;
    let old = p.send("commit-safe 2").await?;
    let current = p.send("commit-safe 3").await?;
    let mut c = Check::new("every answer commit-safe can give");
    c.eq(
        "commit-safe(9).reason",
        "past the end of the log".to_string(),
        p.expect_str(&past, "commit-safe 9", "reason")?,
    );
    c.eq(
        "commit-safe(0).reason",
        "past the end of the log".to_string(),
        p.expect_str(&empty, "commit-safe 0", "reason")?,
    );
    c.eq(
        "commit-safe(3).reason before any ack",
        "no majority".to_string(),
        p.expect_str(&lonely, "commit-safe 3", "reason")?,
    );
    // Index 2 is held by a majority and has been committed indirectly by index 3, so the
    // answer is no longer about its term.
    c.eq(
        "commit-safe(2).reason",
        "already committed".to_string(),
        p.expect_str(&old, "commit-safe 2", "reason")?,
    );
    c.eq(
        "commit-safe(3).reason",
        "already committed".to_string(),
        p.expect_str(&current, "commit-safe 3", "reason")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_late_ack_never_lowers_a_match_index, |ctx| {
    let p = ctx.prim("raft-commit").await?;
    p.send("init 3 1 [1,1,1]").await?;
    let high = p.send("ack s2 3").await?;
    // The same follower's older reply, which took the scenic route back.
    let late = p.send("ack s2 1").await?;
    let zero = p.send("ack s2 0").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("acknowledgements that arrived out of order");
    c.eq(
        "the first ack's match_index",
        3,
        p.expect_i64(&high, "ack s2 3", "match_index")?,
    );
    c.eq(
        "the first ack's commit_index",
        3,
        p.expect_i64(&high, "ack s2 3", "commit_index")?,
    );
    // Taking the late reply at face value would say the follower has lost two entries it
    // demonstrably holds, and would drag the commit index back with it.
    c.eq(
        "the late ack's match_index",
        3,
        p.expect_i64(&late, "ack s2 1", "match_index")?,
    );
    c.eq(
        "the late ack's next_index",
        4,
        p.expect_i64(&late, "ack s2 1", "next_index")?,
    );
    c.eq(
        "a zero ack's match_index",
        3,
        p.expect_i64(&zero, "ack s2 0", "match_index")?,
    );
    c.eq(
        "state.commit_index",
        3,
        p.expect_i64(&state, "state", "commit_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_committed_entry_is_fixed, |ctx| {
    let p = ctx.prim("raft-commit").await?;
    p.send("init 3 4 [1,2,4]").await?;
    p.send("ack s2 3").await?;
    let committed = p.send("state").await?;
    let before = p.send("entry 2").await?;
    // Everything a hostile schedule could throw at the leader afterwards: late acks, zero
    // acks, rejections that back a follower all the way up.
    for command in ["ack s2 0", "ack s3 1", "reject s2 1", "ack s3 0", "append"] {
        p.send(command).await?;
    }
    let after_state = p.send("state").await?;
    let after = p.send("entry 2").await?;
    let third = p.send("entry 3").await?;
    let mut c = Check::new("State Machine Safety: a committed index never changes its mind");
    c.eq(
        "commit_index once committed",
        3,
        p.expect_i64(&committed, "state", "commit_index")?,
    );
    c.eq(
        "entry(2).term before",
        2,
        p.expect_i64(&before, "entry 2", "term")?,
    );
    c.eq(
        "entry(2).term after",
        2,
        p.expect_i64(&after, "entry 2", "term")?,
    );
    c.eq(
        "entry(3).term after",
        4,
        p.expect_i64(&third, "entry 3", "term")?,
    );
    c.at_least(
        "state.commit_index after",
        3,
        p.expect_i64(&after_state, "state", "commit_index")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_long_seeded_commitment, |ctx| {
    // The whole conversation is worked out here first — every command and the commit index
    // it must leave behind — by replaying §5.4.2 over the seeded plan. The program is then
    // asked the same questions and never consulted about the answers.
    let seed_log = [1i64, 1, 2, 3];
    let members = 5usize;
    let term = 3i64;
    let peers = ["s2", "s3", "s4", "s5"];
    let mut model = Model::new(members, term, &seed_log);
    let mut script: Vec<(String, i64, i64, i64)> = Vec::new();
    for _ in 0..80 {
        // An `append` answers with no indexes of its own, so the replay marks those steps
        // with -1 and reads the commit index back from `state` instead.
        if ctx.rng.random_range(0..6) == 0 {
            model.append();
            script.push(("append".to_string(), -1, -1, model.commit_index));
            continue;
        }
        let peer = peers[ctx.rng.random_range(0..peers.len())];
        let index = ctx.rng.random_range(0..(model.last_index() + 2));
        let (matched, next) = model.ack(peer, index);
        script.push((
            format!("ack {peer} {index}"),
            matched,
            next,
            model.commit_index,
        ));
    }
    let seed = ctx.seed;
    let p = ctx.prim("raft-commit").await?;
    p.send(&format!("init {members} {term} {}", terms_arg(&seed_log)))
        .await?;
    let mut actual = Vec::with_capacity(script.len());
    for (command, ..) in &script {
        let r = p.send(command).await?;
        if command == "append" {
            let state = p.send("state").await?;
            actual.push((-1, -1, p.expect_i64(&state, "state", "commit_index")?));
        } else {
            actual.push((
                p.expect_i64(&r, command, "match_index")?,
                p.expect_i64(&r, command, "next_index")?,
                p.expect_i64(&r, command, "commit_index")?,
            ));
        }
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("eighty seeded acknowledgements replayed against §5.4.2");
    c.note(format!("seed {seed}, {} events", script.len()));
    let mut highest = 0i64;
    for (i, ((command, matched, next, commit), (got_match, got_next, got_commit))) in
        script.iter().zip(&actual).enumerate()
    {
        c.eq(
            &format!("step[{i}] {command} → match_index"),
            *matched,
            *got_match,
        );
        c.eq(
            &format!("step[{i}] {command} → next_index"),
            *next,
            *got_next,
        );
        c.eq(
            &format!("step[{i}] {command} → commit_index"),
            *commit,
            *got_commit,
        );
        // Whatever else happens, the commit index is a ratchet.
        c.at_least(
            &format!("step[{i}] {command} → commit_index is monotonic"),
            highest,
            *got_commit,
        );
        highest = highest.max(*got_commit);
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
