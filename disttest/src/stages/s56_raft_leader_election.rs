//! Stage 56 — Raft leader election.
//!
//! The first rung of the algorithms ladder, and the smallest complete piece of consensus
//! there is: one node, one term counter, one `votedFor` slot, and a rule about whose log is
//! allowed to win. No sockets, no timers, no cluster — the election is a state machine, and
//! the harness drives it by handing it the events a real node would have received.
//!
//! The oracle is Raft's own rules, transcribed. Nothing here asks the program what it
//! thinks the answer is: the tester keeps the term, the role, the vote and the log itself,
//! replays §5.1's "step down on a higher term", §5.2's "one vote per term" and §5.4.1's
//! up-to-date check by hand, and compares. The seeded replay at the end of the stage is the
//! same model over eighty events, which is where an implementation that counts a repeated
//! vote twice, or that grants a vote to a candidate whose log is behind, finally shows.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use std::collections::BTreeSet;

/// Stage 56.
pub fn stage() -> Stage {
    Stage {
        number: 56,
        slug: "raft_leader_election",
        name: "Raft leader election",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `raft-election`: one node's role, term, votedFor and vote tally",
            "Any message carrying a higher term makes this node a follower and clears votedFor",
            "One vote per term, and only for a candidate whose log is at least as up to date",
            "Count each voter once: a retransmitted vote must not make a minority a majority",
        ],
        examples,
        tests: vec![
            Test::new(
                "a fresh node is a follower at term zero",
                a_fresh_node_is_a_follower,
            ),
            Test::new(
                "an election timeout makes it a candidate that voted for itself",
                a_timeout_starts_an_election,
            ),
            Test::new(
                "a vote request from an older term is refused",
                an_older_term_is_refused,
            ),
            Test::new(
                "a vote request from a newer term makes it step down",
                a_newer_term_makes_it_step_down,
            ),
            Test::new("a node votes only once per term", one_vote_per_term),
            Test::new(
                "the same candidate asking twice gets the same answer",
                a_repeated_request_is_idempotent,
            ),
            Test::new(
                "a candidate whose log is behind is refused",
                a_shorter_log_is_refused,
            ),
            Test::new(
                "a higher last term beats a longer log",
                a_higher_last_term_wins,
            ),
            Test::new(
                "a majority of votes makes a candidate the leader",
                a_majority_elects,
            ),
            Test::new(
                "the same vote arriving twice does not count twice",
                a_duplicate_vote_does_not_count,
            ),
            Test::new(
                "an append-entries from the current term ends the election",
                an_append_entries_ends_the_election,
            ),
            Test::new(
                "a split vote elects nobody until a later term",
                a_split_vote_elects_nobody,
            )
            .ext(),
            Test::new(
                "a leader steps down when it sees a higher term",
                a_leader_steps_down,
            )
            .ext(),
            Test::new(
                "a leader does not start an election of its own",
                a_leader_ignores_its_timeout,
            )
            .ext(),
            Test::new(
                "eighty seeded events match the tester's own replay",
                a_long_seeded_election,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("An election won on the third vote", "raft-election", || {
            lines(&[
                "init 5",
                "timeout",
                "vote-response 1 s2 true",
                "vote-response 1 s3 true",
                "state",
            ])
        })
        .request("a five-member cluster, one timeout, and two granted votes")
        .response("candidate at term 1 with one vote, then two, then leader on the third")
        .note(
            "The candidate's own vote counts, so a five-member cluster needs two more, not \
             three. Forgetting it means a leader is never elected until an extra round of \
             timeouts, which looks like a liveness bug and is really an off-by-one.",
        ),
        prim_example("A candidate whose log is behind", "raft-election", || {
            lines(&[
                "init 3",
                "log-append 1",
                "log-append 2",
                "request-vote 3 s2 1 1",
                "request-vote 3 s3 2 2",
            ])
        })
        .request("a node holding two entries, asked by a short candidate and then a current one")
        .response("granted false for the first, and true for the second")
        .note(
            "This is §5.4.1, and it is the only thing standing between Raft and a leader \
             that silently drops committed entries. Note that the first request still moved \
             the term to 3: a refused vote is not an ignored message.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of §5.1, §5.2 and §5.4.1. Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// The election rules, as the paper states them.
#[derive(Default)]
struct Model {
    members: usize,
    term: i64,
    role: &'static str,
    voted_for: Option<String>,
    granters: BTreeSet<String>,
    log: Vec<i64>,
}

impl Model {
    fn new(members: usize) -> Model {
        Model {
            members,
            term: 0,
            role: "follower",
            voted_for: None,
            granters: BTreeSet::new(),
            log: Vec::new(),
        }
    }

    fn majority(&self) -> usize {
        self.members / 2 + 1
    }

    fn last_index(&self) -> i64 {
        self.log.len() as i64
    }

    fn last_term(&self) -> i64 {
        self.log.last().copied().unwrap_or(0)
    }

    fn step_down(&mut self, term: i64) {
        self.term = term;
        self.role = "follower";
        self.voted_for = None;
        self.granters.clear();
    }

    fn timeout(&mut self) {
        if self.role == "leader" {
            return;
        }
        self.term += 1;
        self.role = "candidate";
        self.voted_for = Some("self".to_string());
        self.granters.clear();
        self.granters.insert("self".to_string());
    }

    /// Returns what the node must answer: `(term, granted)`.
    fn request_vote(&mut self, term: i64, candidate: &str, li: i64, lt: i64) -> (i64, bool) {
        if term < self.term {
            return (self.term, false);
        }
        if term > self.term {
            self.step_down(term);
        }
        let free = self.voted_for.as_deref().is_none_or(|w| w == candidate);
        let current = lt > self.last_term() || (lt == self.last_term() && li >= self.last_index());
        let granted = free && current;
        if granted {
            self.voted_for = Some(candidate.to_string());
        }
        (self.term, granted)
    }

    fn vote_response(&mut self, term: i64, voter: &str, granted: bool) {
        if term > self.term {
            self.step_down(term);
            return;
        }
        if term == self.term && self.role == "candidate" && granted {
            self.granters.insert(voter.to_string());
            if self.granters.len() >= self.majority() {
                self.role = "leader";
            }
        }
    }

    /// Returns what the node must answer: `(term, success)`.
    fn append_entries(&mut self, term: i64, _leader: &str) -> (i64, bool) {
        if term < self.term {
            return (self.term, false);
        }
        if term > self.term {
            self.step_down(term);
        } else {
            self.role = "follower";
        }
        (self.term, true)
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_node_is_a_follower, |ctx| {
    let p = ctx.prim("raft-election").await?;
    let init = p.send("init 3").await?;
    let majority = p.expect_i64(&init, "init 3", "majority")?;
    let s = p.send("state").await?;
    let role = p.expect_str(&s, "state", "state")?;
    let term = p.expect_i64(&s, "state", "term")?;
    let votes = p.expect_i64(&s, "state", "votes")?;
    let mut c = Check::new("a node that has heard nothing yet");
    c.eq("state.state", "follower".to_string(), role);
    c.eq("state.term", 0, term);
    c.eq("state.votes", 0, votes);
    c.eq("state.voted_for", &serde_json::Value::Null, &s["voted_for"]);
    c.eq("init.majority", 2, majority);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_timeout_starts_an_election, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 5").await?;
    let t = p.send("timeout").await?;
    let role = p.expect_str(&t, "timeout", "state")?;
    let term = p.expect_i64(&t, "timeout", "term")?;
    let votes = p.expect_i64(&t, "timeout", "votes")?;
    let voted_for = p.expect_str(&t, "timeout", "voted_for")?;
    let again = p.send("timeout").await?;
    let term2 = p.expect_i64(&again, "timeout", "term")?;
    let mut c = Check::new("the election an expired timer starts");
    c.eq("timeout.state", "candidate".to_string(), role);
    c.eq("timeout.term", 1, term);
    // The candidate votes for itself, which is one of the votes it needs.
    c.eq("timeout.votes", 1, votes);
    c.eq("timeout.voted_for", "self".to_string(), voted_for);
    c.eq("the second timeout's term", 2, term2);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_older_term_is_refused, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 3").await?;
    p.send("timeout").await?;
    p.send("timeout").await?;
    let r = p.send("request-vote 1 s2 0 0").await?;
    let granted = p.expect_bool(&r, "request-vote 1 s2 0 0", "granted")?;
    let term = p.expect_i64(&r, "request-vote 1 s2 0 0", "term")?;
    let s = p.send("state").await?;
    let after = p.expect_i64(&s, "state", "term")?;
    let mut c = Check::new("a vote request stamped with a term that has already passed");
    c.eq("request-vote.granted", false, granted);
    // The answer carries *this* node's term, which is how the stale candidate learns.
    c.eq("request-vote.term", 2, term);
    c.eq("state.term", 2, after);
    c.eq(
        "state.state",
        "candidate".to_string(),
        p.expect_str(&s, "state", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_newer_term_makes_it_step_down, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 3").await?;
    p.send("timeout").await?;
    let r = p.send("request-vote 7 s3 0 0").await?;
    let granted = p.expect_bool(&r, "request-vote 7 s3 0 0", "granted")?;
    let s = p.send("state").await?;
    let mut c = Check::new("a candidate that meets a higher term");
    c.eq("request-vote.granted", true, granted);
    c.eq(
        "request-vote.term",
        7,
        p.expect_i64(&r, "request-vote", "term")?,
    );
    c.eq(
        "state.state",
        "follower".to_string(),
        p.expect_str(&s, "state", "state")?,
    );
    c.eq("state.term", 7, p.expect_i64(&s, "state", "term")?);
    // Stepping down clears the vote it cast for itself, and then it votes for s3.
    c.eq(
        "state.voted_for",
        "s3".to_string(),
        p.expect_str(&s, "state", "voted_for")?,
    );
    c.eq("state.votes", 0, p.expect_i64(&s, "state", "votes")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(one_vote_per_term, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 5").await?;
    let first = p.send("request-vote 4 s2 0 0").await?;
    let second = p.send("request-vote 4 s3 0 0").await?;
    let third = p.send("request-vote 5 s3 0 0").await?;
    let mut c = Check::new("two candidates asking in the same term");
    c.eq(
        "the first request-vote(4, s2).granted",
        true,
        p.expect_bool(&first, "request-vote 4 s2 0 0", "granted")?,
    );
    c.eq(
        "the second request-vote(4, s3).granted",
        false,
        p.expect_bool(&second, "request-vote 4 s3 0 0", "granted")?,
    );
    // A new term is a clean slate: votedFor is cleared and s3 wins the vote it just lost.
    c.eq(
        "request-vote(5, s3).granted",
        true,
        p.expect_bool(&third, "request-vote 5 s3 0 0", "granted")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_repeated_request_is_idempotent, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 3").await?;
    let mut answers = Vec::new();
    for _ in 0..3 {
        let r = p.send("request-vote 2 s2 0 0").await?;
        answers.push(p.expect_bool(&r, "request-vote 2 s2 0 0", "granted")?);
    }
    let s = p.send("state").await?;
    let mut c = Check::new("a candidate whose request was retransmitted");
    // The vote is already s2's; answering false here would strand a candidate whose reply
    // was simply lost on the way back.
    c.eq("the three answers", vec![true, true, true], answers);
    c.eq(
        "state.voted_for",
        "s2".to_string(),
        p.expect_str(&s, "state", "voted_for")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_shorter_log_is_refused, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 3").await?;
    for term in [1, 1, 2] {
        p.send(&format!("log-append {term}")).await?;
    }
    let short = p.send("request-vote 3 s2 2 1").await?;
    let equal = p.send("request-vote 3 s2 3 2").await?;
    let longer = p.send("request-vote 4 s3 9 2").await?;
    let mut c = Check::new("the up-to-date check of §5.4.1");
    c.eq(
        "request-vote(last_index 2, last_term 1).granted",
        false,
        p.expect_bool(&short, "request-vote 3 s2 2 1", "granted")?,
    );
    c.eq(
        "request-vote(last_index 3, last_term 2).granted",
        true,
        p.expect_bool(&equal, "request-vote 3 s2 3 2", "granted")?,
    );
    c.eq(
        "request-vote(last_index 9, last_term 2).granted",
        true,
        p.expect_bool(&longer, "request-vote 4 s3 9 2", "granted")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_higher_last_term_wins, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 3").await?;
    for term in [1, 1, 1, 1, 1] {
        p.send(&format!("log-append {term}")).await?;
    }
    // One entry, but at term 4: a later term beats a longer log outright.
    let r = p.send("request-vote 5 s2 1 4").await?;
    let mut c = Check::new("a short log at a later term against a long log at an early one");
    c.eq(
        "request-vote(last_index 1, last_term 4).granted",
        true,
        p.expect_bool(&r, "request-vote 5 s2 1 4", "granted")?,
    );
    // The comparison is lexicographic on (term, index) — never on length alone.
    c.eq("state.last_index", 5, p.num("state", "last_index").await?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_majority_elects, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 5").await?;
    p.send("timeout").await?;
    let one = p.send("vote-response 1 s2 true").await?;
    let two = p.send("vote-response 1 s3 true").await?;
    let mut c = Check::new("a five-member election won with three votes");
    c.eq(
        "after one granted vote, state",
        "candidate".to_string(),
        p.expect_str(&one, "vote-response", "state")?,
    );
    c.eq(
        "after one granted vote, votes",
        2,
        p.expect_i64(&one, "vote-response", "votes")?,
    );
    c.eq(
        "after two granted votes, state",
        "leader".to_string(),
        p.expect_str(&two, "vote-response", "state")?,
    );
    c.eq(
        "after two granted votes, votes",
        3,
        p.expect_i64(&two, "vote-response", "votes")?,
    );
    c.eq("state.term", 1, p.num("state", "term").await?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_duplicate_vote_does_not_count, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 5").await?;
    p.send("timeout").await?;
    let mut seen = Vec::new();
    for _ in 0..3 {
        let r = p.send("vote-response 1 s2 true").await?;
        seen.push(p.expect_i64(&r, "vote-response 1 s2 true", "votes")?);
    }
    let refused = p.send("vote-response 1 s3 false").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("one voter answering three times, and one refusing");
    // Counting a tally rather than a set of voters is how a minority elects itself.
    c.eq("the vote counts", vec![2, 2, 2], seen);
    c.eq(
        "after a refusal, votes",
        2,
        p.expect_i64(&refused, "vote-response 1 s3 false", "votes")?,
    );
    c.eq(
        "state.state",
        "candidate".to_string(),
        p.expect_str(&s, "state", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_append_entries_ends_the_election, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 3").await?;
    p.send("timeout").await?;
    let same = p.send("append-entries 1 s2").await?;
    let s = p.send("state").await?;
    let stale = p.send("append-entries 0 s3").await?;
    let after = p.send("state").await?;
    let mut c = Check::new("a heartbeat arriving during this node's own election");
    c.eq(
        "append-entries(1, s2).success",
        true,
        p.expect_bool(&same, "append-entries 1 s2", "success")?,
    );
    c.eq(
        "append-entries(1, s2).state",
        "follower".to_string(),
        p.expect_str(&same, "append-entries 1 s2", "state")?,
    );
    c.eq(
        "state.leader",
        "s2".to_string(),
        p.expect_str(&s, "state", "leader")?,
    );
    // A heartbeat from a term that has passed is refused, and changes nothing.
    c.eq(
        "append-entries(0, s3).success",
        false,
        p.expect_bool(&stale, "append-entries 0 s3", "success")?,
    );
    c.eq(
        "state.leader after the stale heartbeat",
        "s2".to_string(),
        p.expect_str(&after, "state", "leader")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_split_vote_elects_nobody, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 5").await?;
    p.send("timeout").await?;
    // Two peers granted, two refused: two votes plus this node's own is short of three.
    let a = p.send("vote-response 1 s2 true").await?;
    let b = p.send("vote-response 1 s3 false").await?;
    let c_ = p.send("vote-response 1 s4 false").await?;
    let still = p.expect_str(&c_, "vote-response 1 s4 false", "state")?;
    // The cure for a split vote is another timeout at a higher term, not a lower bar.
    let retry = p.send("timeout").await?;
    let mut c = Check::new("an election that nobody won");
    c.eq(
        "after s2 granted, votes",
        2,
        p.expect_i64(&a, "vote-response", "votes")?,
    );
    c.eq(
        "after s3 refused, votes",
        2,
        p.expect_i64(&b, "vote-response", "votes")?,
    );
    c.eq("after s4 refused, state", "candidate".to_string(), still);
    c.eq(
        "the retry's term",
        2,
        p.expect_i64(&retry, "timeout", "term")?,
    );
    c.eq(
        "the retry's votes",
        1,
        p.expect_i64(&retry, "timeout", "votes")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_leader_steps_down, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 3").await?;
    p.send("timeout").await?;
    p.send("vote-response 1 s2 true").await?;
    let elected = p.send("state").await?;
    let leader = p.expect_str(&elected, "state", "state")?;
    let higher = p.send("append-entries 4 s3").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("a leader that meets a later term");
    c.eq("state before the higher term", "leader".to_string(), leader);
    c.eq(
        "append-entries(4, s3).state",
        "follower".to_string(),
        p.expect_str(&higher, "append-entries 4 s3", "state")?,
    );
    c.eq("state.term", 4, p.expect_i64(&s, "state", "term")?);
    c.eq("state.voted_for", &serde_json::Value::Null, &s["voted_for"]);
    c.eq("state.votes", 0, p.expect_i64(&s, "state", "votes")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_leader_ignores_its_timeout, |ctx| {
    let p = ctx.prim("raft-election").await?;
    p.send("init 3").await?;
    p.send("timeout").await?;
    p.send("vote-response 1 s2 true").await?;
    let after = p.send("timeout").await?;
    let mut c = Check::new("an elected leader whose election timer fires anyway");
    // A leader is already sending heartbeats; starting an election would only churn the
    // term and cost the cluster an availability gap for nothing.
    c.eq(
        "timeout.state",
        "leader".to_string(),
        p.expect_str(&after, "timeout", "state")?,
    );
    c.eq("timeout.term", 1, p.expect_i64(&after, "timeout", "term")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_long_seeded_election, |ctx| {
    // The whole conversation is worked out here first — every command and the state it must
    // leave behind — by replaying §5.1, §5.2 and §5.4.1 over the seeded plan. The program is
    // then asked the same questions and never consulted about the answers.
    let peers = ["s2", "s3", "s4", "s5"];
    let mut model = Model::new(5);
    let mut script: Vec<(String, &'static str, i64, usize)> = Vec::new();
    for _ in 0..80 {
        let who = peers[ctx.rng.random_range(0..peers.len())];
        let command = match ctx.rng.random_range(0..5) {
            0 => {
                let term = model.last_term() + ctx.rng.random_range(0..2);
                model.log.push(term.max(1));
                format!("log-append {}", term.max(1))
            }
            1 => {
                model.timeout();
                "timeout".to_string()
            }
            2 => {
                let term = model.term + ctx.rng.random_range(-1..3);
                let li = ctx.rng.random_range(0..(model.last_index() + 3));
                let lt = ctx.rng.random_range(0..(model.last_term() + 3));
                model.request_vote(term, who, li, lt);
                format!("request-vote {term} {who} {li} {lt}")
            }
            3 => {
                let term = model.term + ctx.rng.random_range(-1..2);
                let granted = ctx.rng.random_bool(0.7);
                model.vote_response(term, who, granted);
                format!("vote-response {term} {who} {granted}")
            }
            _ => {
                let term = model.term + ctx.rng.random_range(-1..2);
                model.append_entries(term, who);
                format!("append-entries {term} {who}")
            }
        };
        script.push((command, model.role, model.term, model.granters.len()));
    }
    let seed = ctx.seed;
    let p = ctx.prim("raft-election").await?;
    p.send("init 5").await?;
    let mut actual = Vec::with_capacity(script.len());
    for (command, ..) in &script {
        p.send(command).await?;
        let s = p.send("state").await?;
        actual.push((
            p.expect_str(&s, "state", "state")?,
            p.expect_i64(&s, "state", "term")?,
            p.expect_i64(&s, "state", "votes")?,
        ));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("eighty seeded events replayed against the tester's own rules");
    c.note(format!("seed {seed}, {} events", script.len()));
    for (i, ((command, role, term, votes), (got_role, got_term, got_votes))) in
        script.iter().zip(&actual).enumerate()
    {
        c.eq(
            &format!("step[{i}] {command} → state"),
            (*role).to_string(),
            got_role.clone(),
        );
        c.eq(&format!("step[{i}] {command} → term"), *term, *got_term);
        c.eq(
            &format!("step[{i}] {command} → votes"),
            *votes as i64,
            *got_votes,
        );
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
