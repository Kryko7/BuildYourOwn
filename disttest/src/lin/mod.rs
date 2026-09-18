//! The linearizability checker.
//!
//! A history is a list of operations, each with the wall-clock instant the client *called*
//! it and the instant the answer came *back*. The history is linearizable when every
//! operation can be placed at a single instant inside its own call/return window, in some
//! total order, such that a sequential key/value store would have produced exactly the
//! answers the clients saw.
//!
//! The search is Wing and Gong's, in the shape Lowe and Porcupine describe: repeatedly pick
//! an operation that is allowed to go next, apply it to the model, recurse, and undo the
//! choice when the branch dies. Two things keep it from exploding:
//!
//! * **The candidate rule.** An operation may only be linearized next when no other
//!   remaining operation has already returned before it was called. That is the whole of
//!   the real-time constraint, and it prunes most of the search tree.
//! * **Memoisation.** A branch is identified by (the set of operations already linearized,
//!   the model state). Reaching the same pair twice can never end differently, so the
//!   second visit is abandoned immediately.
//!
//! Operations whose answer never arrived — a timeout, a killed node, a partitioned client —
//! are *optional*: they may be linearized anywhere after they were called, or left out
//! entirely, because the harness genuinely does not know whether they took effect. That is
//! the only sound reading of a lost response, and it is what makes fault injection testable
//! at all.
//!
//! Keys are checked independently. The workload never spans two keys in one operation, so a
//! history is linearizable exactly when each key's sub-history is, and splitting turns one
//! intractable search into a handful of small ones.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// What a client asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Read the key.
    Read,
    /// Write a value.
    Write(Vec<u8>),
    /// Delete the key.
    Delete,
    /// Compare-and-swap: set the key to `new` only if it currently holds `expect`
    /// (`None` meaning "the key does not exist").
    Cas {
        /// The value the operation required the key to hold.
        expect: Option<Vec<u8>>,
        /// The value it wrote when the comparison held.
        new: Vec<u8>,
    },
}

/// What the client saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A read that saw this value (`None` when the key was absent).
    Value(Option<Vec<u8>>),
    /// A write or delete the server acknowledged.
    Ok,
    /// A compare-and-swap the server acknowledged, and whether the comparison held.
    Swapped(bool),
    /// No answer ever came back: the operation may or may not have taken effect.
    Unknown,
}

/// One operation of a recorded history.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Index in the history, used by the report.
    pub id: usize,
    /// Which client thread issued it.
    pub client: usize,
    /// Which key it touched.
    pub key: String,
    /// What was asked.
    pub op: Op,
    /// What came back.
    pub outcome: Outcome,
    /// Nanoseconds since the start of the run, when the client made the call.
    pub call_ns: u64,
    /// Nanoseconds since the start of the run, when the answer arrived. For an operation
    /// with no answer this is [`u64::MAX`]: it may be linearized arbitrarily late.
    pub ret_ns: u64,
    /// A note the workload attached, e.g. which member the request went to.
    pub note: String,
}

impl Entry {
    /// True when the answer never arrived.
    pub fn is_unknown(&self) -> bool {
        self.outcome == Outcome::Unknown
    }

    /// One line, the way the failure block prints it.
    pub fn render(&self, base_ns: u64) -> String {
        let call = (self.call_ns.saturating_sub(base_ns)) as f64 / 1e6;
        let ret = if self.ret_ns == u64::MAX {
            "   never".to_string()
        } else {
            format!("{:8.3}", (self.ret_ns.saturating_sub(base_ns)) as f64 / 1e6)
        };
        let op = match &self.op {
            Op::Read => "read".to_string(),
            Op::Write(v) => format!("write {}", text(v)),
            Op::Delete => "delete".to_string(),
            Op::Cas { expect, new } => {
                format!("cas {} -> {}", opt_text(expect.as_deref()), text(new))
            }
        };
        let out = match &self.outcome {
            Outcome::Value(v) => format!("= {}", opt_text(v.as_deref())),
            Outcome::Ok => "ok".to_string(),
            Outcome::Swapped(true) => "swapped".to_string(),
            Outcome::Swapped(false) => "not swapped".to_string(),
            Outcome::Unknown => "NO ANSWER".to_string(),
        };
        let note = if self.note.is_empty() {
            String::new()
        } else {
            format!("   [{}]", self.note)
        };
        format!(
            "#{:<4} c{:<2} {:>9.3}..{ret} ms  {:<6} {:<22} {:<14}{note}",
            self.id, self.client, call, self.key, op, out
        )
    }
}

fn text(v: &[u8]) -> String {
    String::from_utf8_lossy(v).to_string()
}

fn opt_text(v: Option<&[u8]>) -> String {
    match v {
        Some(v) => text(v),
        None => "<absent>".to_string(),
    }
}

/// A recorded history.
#[derive(Debug, Clone, Default)]
pub struct History {
    /// The operations, in the order the clients called them.
    pub entries: Vec<Entry>,
}

impl History {
    /// An empty history.
    pub fn new() -> History {
        History::default()
    }

    /// Add an operation, numbering it.
    pub fn push(&mut self, mut e: Entry) {
        e.id = self.entries.len();
        self.entries.push(e);
    }

    /// How many operations it holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many operations never got an answer.
    pub fn unknowns(&self) -> usize {
        self.entries.iter().filter(|e| e.is_unknown()).count()
    }

    /// The sub-history of one key, renumbered from zero but keeping the original ids.
    pub fn for_key(&self, key: &str) -> Vec<Entry> {
        self.entries
            .iter()
            .filter(|e| e.key == key)
            .cloned()
            .collect()
    }

    /// Every key the history mentions, in first-seen order.
    pub fn keys(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for e in &self.entries {
            if seen.insert(e.key.clone()) {
                out.push(e.key.clone());
            }
        }
        out
    }

    /// The whole history, rendered.
    pub fn render(&self) -> String {
        render_entries(&self.entries)
    }
}

/// Render a list of operations as a readable timeline.
pub fn render_entries(entries: &[Entry]) -> String {
    let base = entries.iter().map(|e| e.call_ns).min().unwrap_or(0);
    let mut s = String::new();
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by_key(|e| (e.call_ns, e.id));
    for e in sorted {
        let _ = writeln!(s, "{}", e.render(base));
    }
    s.trim_end().to_string()
}

/// The model state of one key: the value it holds, or nothing.
type State = Option<Vec<u8>>;

/// Apply one operation to the model, or refuse it.
///
/// Deterministic on purpose: a compare-and-swap whose *answer* was lost is still decided by
/// the state it is applied to, so an unknown outcome only relaxes the guard, it never forks
/// the search.
fn step(state: &State, e: &Entry) -> Option<State> {
    match (&e.op, &e.outcome) {
        (Op::Read, Outcome::Value(v)) => (state == v).then(|| state.clone()),
        (Op::Read, Outcome::Unknown) => Some(state.clone()),
        (Op::Write(v), Outcome::Ok | Outcome::Unknown) => Some(Some(v.clone())),
        (Op::Delete, Outcome::Ok | Outcome::Unknown) => Some(None),
        (Op::Cas { expect, new }, Outcome::Swapped(true)) => {
            (state == expect).then(|| Some(new.clone()))
        }
        (Op::Cas { expect, .. }, Outcome::Swapped(false)) => {
            (state != expect).then(|| state.clone())
        }
        (Op::Cas { expect, new }, Outcome::Unknown) => Some(if state == expect {
            Some(new.clone())
        } else {
            state.clone()
        }),
        // A shape the workload never produces; refusing it is safer than guessing.
        _ => None,
    }
}

/// What the checker concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every operation can be placed inside its own window.
    Linearizable,
    /// No placement exists. The report carries the smallest prefix that already fails.
    NotLinearizable(Box<Violation>),
    /// The search ran out of budget before it could decide.
    Inconclusive {
        /// The key whose sub-history was too large.
        key: String,
        /// How many states were visited before giving up.
        states: u64,
    },
}

impl Verdict {
    /// True when the history is linearizable (an inconclusive search is not a failure).
    pub fn ok(&self) -> bool {
        !matches!(self, Verdict::NotLinearizable(_))
    }
}

/// The evidence behind a [`Verdict::NotLinearizable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The key whose sub-history cannot be linearized.
    pub key: String,
    /// The smallest prefix of that sub-history that already admits no linearization.
    pub entries: Vec<Entry>,
    /// The operation whose return made the prefix impossible.
    pub culprit: Entry,
    /// The longest prefix of a linearization the search ever managed to build.
    pub best_attempt: Vec<usize>,
    /// The model state at the end of that attempt.
    pub best_state: Option<Vec<u8>>,
    /// How many distinct (linearized set, state) pairs were explored.
    pub states: u64,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.key == other.key
            && self.op == other.op
            && self.outcome == other.outcome
            && self.call_ns == other.call_ns
            && self.ret_ns == other.ret_ns
    }
}
impl Eq for Entry {}

impl Violation {
    /// The failure block a stage prints: the offending operations and what the search did.
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "key {:?}: no linearization exists for these {} operations",
            self.key,
            self.entries.len()
        );
        let _ = writeln!(s);
        let _ = writeln!(
            s,
            "  id    client  call..return (ms)     key    operation              result"
        );
        let base = self.entries.iter().map(|e| e.call_ns).min().unwrap_or(0);
        for e in &self.entries {
            let mark = if e.id == self.culprit.id { ">>" } else { "  " };
            let _ = writeln!(s, "{mark}{}", e.render(base));
        }
        let _ = writeln!(s);
        let _ = writeln!(
            s,
            "the operation marked >> cannot be placed anywhere inside its own window"
        );
        if !self.best_attempt.is_empty() {
            let order: Vec<String> = self
                .best_attempt
                .iter()
                .map(|id| format!("#{id}"))
                .collect();
            let _ = writeln!(
                s,
                "the furthest the search ever got was {} of {} operations: {}",
                self.best_attempt.len(),
                self.entries.len(),
                order.join(" → ")
            );
            let _ = writeln!(
                s,
                "leaving the key holding {}, which none of the remaining operations accept",
                opt_text(self.best_state.as_deref())
            );
        }
        let _ = writeln!(s, "({} linearization states explored)", self.states);
        s.trim_end().to_string()
    }
}

/// How many (set, state) pairs the search may visit per key before giving up.
pub const DEFAULT_BUDGET: u64 = 400_000;

/// Check a history for linearizability against a key/value register model.
pub fn check(history: &History) -> Verdict {
    check_with_budget(history, DEFAULT_BUDGET)
}

/// [`check`] with an explicit search budget, so a test can prove the budget is respected.
pub fn check_with_budget(history: &History, budget: u64) -> Verdict {
    for key in history.keys() {
        let entries = history.for_key(&key);
        match check_one_key(&key, &entries, budget) {
            Verdict::Linearizable => continue,
            other => return other,
        }
    }
    Verdict::Linearizable
}

fn check_one_key(key: &str, entries: &[Entry], budget: u64) -> Verdict {
    let mut search = Search::new(entries, budget);
    if search.run() {
        return Verdict::Linearizable;
    }
    if search.exhausted_budget {
        return Verdict::Inconclusive {
            key: key.to_string(),
            states: search.states,
        };
    }
    // Find the smallest prefix that already fails, so the report shows the few operations
    // that matter instead of the whole workload. Prefixes are taken in return order,
    // because it is a *return* that makes a history impossible.
    let mut by_return: Vec<Entry> = entries.to_vec();
    by_return.sort_by_key(|e| (e.ret_ns, e.call_ns, e.id));
    for n in 1..=by_return.len() {
        let prefix = &by_return[..n];
        let mut s = Search::new(prefix, budget);
        if !s.run() && !s.exhausted_budget {
            return Verdict::NotLinearizable(Box::new(Violation {
                key: key.to_string(),
                entries: prefix.to_vec(),
                culprit: prefix[n - 1].clone(),
                best_attempt: s.best_attempt.clone(),
                best_state: s.best_state.clone().flatten(),
                states: search.states,
            }));
        }
    }
    // Every prefix is linearizable but the whole is not: report the whole thing.
    Verdict::NotLinearizable(Box::new(Violation {
        key: key.to_string(),
        entries: by_return.clone(),
        culprit: by_return.last().cloned().unwrap_or_else(|| entries[0].clone()),
        best_attempt: search.best_attempt.clone(),
        best_state: search.best_state.clone().flatten(),
        states: search.states,
    }))
}

struct Search<'a> {
    entries: &'a [Entry],
    /// `true` while the operation is still waiting to be linearized.
    remaining: Vec<bool>,
    /// The same information as a bitmap, kept in step with `remaining` so the memo key
    /// costs one clone instead of a scan of the whole history at every node.
    mask: Vec<u64>,
    /// The operations linearized so far, in order.
    order: Vec<usize>,
    seen: HashSet<(Vec<u64>, State)>,
    states: u64,
    budget: u64,
    exhausted_budget: bool,
    best_attempt: Vec<usize>,
    best_state: Option<State>,
    known_left: usize,
}

impl<'a> Search<'a> {
    fn new(entries: &'a [Entry], budget: u64) -> Search<'a> {
        Search {
            entries,
            remaining: vec![true; entries.len()],
            mask: vec![0u64; entries.len().div_ceil(64).max(1)],
            order: Vec::new(),
            seen: HashSet::new(),
            states: 0,
            budget,
            exhausted_budget: false,
            best_attempt: Vec::new(),
            best_state: None,
            known_left: entries.iter().filter(|e| !e.is_unknown()).count(),
        }
    }

    fn run(&mut self) -> bool {
        if self.entries.is_empty() {
            return true;
        }
        let initial: State = None;
        self.recurse(&initial)
    }

    /// Mark operation `i` as linearized (or put it back), keeping the bitmap in step.
    fn set_linearized(&mut self, i: usize, linearized: bool) {
        self.remaining[i] = !linearized;
        let (word, bit) = (i / 64, 1u64 << (i % 64));
        if linearized {
            self.mask[word] |= bit;
        } else {
            self.mask[word] &= !bit;
        }
    }

    fn recurse(&mut self, state: &State) -> bool {
        if self.known_left == 0 {
            return true;
        }
        if self.states >= self.budget {
            self.exhausted_budget = true;
            return false;
        }
        self.states += 1;
        if !self.seen.insert((self.mask.clone(), state.clone())) {
            return false;
        }
        if self.order.len() > self.best_attempt.len() {
            self.best_attempt = self.order.clone();
            self.best_state = Some(state.clone());
        }
        // The candidate rule: nothing may be linearized after an operation that had already
        // returned before it was called.
        let entries = self.entries;
        let min_ret = entries
            .iter()
            .enumerate()
            .filter(|(i, _)| self.remaining[*i])
            .map(|(_, e)| e.ret_ns)
            .min()
            .unwrap_or(u64::MAX);
        for i in 0..entries.len() {
            if !self.remaining[i] {
                continue;
            }
            let e = &entries[i];
            if e.call_ns > min_ret {
                continue;
            }
            let Some(next) = step(state, e) else {
                continue;
            };
            let unknown = e.is_unknown();
            let id = e.id;
            self.set_linearized(i, true);
            self.order.push(id);
            if !unknown {
                self.known_left -= 1;
            }
            let ok = self.recurse(&next);
            if ok {
                return true;
            }
            if !unknown {
                self.known_left += 1;
            }
            self.order.pop();
            self.set_linearized(i, false);
            if self.exhausted_budget {
                return false;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------------------
// Building histories
// ---------------------------------------------------------------------------------------

/// Records operations with real timestamps, from several client tasks at once.
#[derive(Debug, Clone)]
pub struct Recorder {
    start: std::time::Instant,
}

impl Default for Recorder {
    fn default() -> Self {
        Recorder::new()
    }
}

impl Recorder {
    /// Start recording; every timestamp is relative to this instant.
    pub fn new() -> Recorder {
        Recorder {
            start: std::time::Instant::now(),
        }
    }

    /// Nanoseconds since the recorder was created.
    pub fn now(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }

    /// Build one entry; the id is assigned when it is pushed into a [`History`].
    #[allow(clippy::too_many_arguments)]
    pub fn entry(
        &self,
        client: usize,
        key: &str,
        op: Op,
        outcome: Outcome,
        call_ns: u64,
        ret_ns: u64,
        note: impl Into<String>,
    ) -> Entry {
        Entry {
            id: 0,
            client,
            key: key.to_string(),
            op,
            outcome: outcome.clone(),
            call_ns,
            ret_ns: if outcome == Outcome::Unknown {
                u64::MAX
            } else {
                ret_ns
            },
            note: note.into(),
        }
    }
}

/// How many operations each key carries, for the report.
pub fn per_key_counts(history: &History) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    for e in &history.entries {
        *m.entry(e.key.clone()).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: usize, client: usize, op: Op, outcome: Outcome, call: u64, ret: u64) -> Entry {
        Entry {
            id,
            client,
            key: "k".into(),
            op,
            outcome: outcome.clone(),
            call_ns: call,
            ret_ns: if outcome == Outcome::Unknown {
                u64::MAX
            } else {
                ret
            },
            note: String::new(),
        }
    }

    fn history(entries: Vec<Entry>) -> History {
        History { entries }
    }

    fn w(v: &str) -> Op {
        Op::Write(v.as_bytes().to_vec())
    }
    fn read(v: Option<&str>) -> Outcome {
        Outcome::Value(v.map(|s| s.as_bytes().to_vec()))
    }

    #[test]
    fn an_empty_history_is_linearizable() {
        assert_eq!(check(&History::new()), Verdict::Linearizable);
    }

    #[test]
    fn a_sequential_history_is_linearizable() {
        let h = history(vec![
            e(0, 0, w("a"), Outcome::Ok, 0, 10),
            e(1, 0, Op::Read, read(Some("a")), 20, 30),
            e(2, 0, w("b"), Outcome::Ok, 40, 50),
            e(3, 0, Op::Read, read(Some("b")), 60, 70),
            e(4, 0, Op::Delete, Outcome::Ok, 80, 90),
            e(5, 0, Op::Read, read(None), 100, 110),
        ]);
        assert_eq!(check(&h), Verdict::Linearizable);
    }

    #[test]
    fn a_stale_read_after_a_completed_write_is_caught() {
        // write "a" returns at t=10; the read starts at t=20 and still sees nothing.
        let h = history(vec![
            e(0, 0, w("a"), Outcome::Ok, 0, 10),
            e(1, 1, Op::Read, read(None), 20, 30),
        ]);
        let v = check(&h);
        let Verdict::NotLinearizable(viol) = v else {
            panic!("a stale read must be caught, got {v:?}");
        };
        assert_eq!(viol.key, "k");
        assert_eq!(viol.entries.len(), 2);
        assert!(viol.render().contains("no linearization exists"), "{}", viol.render());
    }

    #[test]
    fn a_read_concurrent_with_a_write_may_see_either_value() {
        for seen in [None, Some("a")] {
            let h = history(vec![
                e(0, 0, w("a"), Outcome::Ok, 0, 100),
                e(1, 1, Op::Read, read(seen), 10, 90),
            ]);
            assert_eq!(check(&h), Verdict::Linearizable, "seen = {seen:?}");
        }
    }

    #[test]
    fn two_reads_inside_one_write_must_agree_in_time_order() {
        // The second read finishes before the third begins: seeing "a" then nothing is
        // impossible, because the write cannot be un-applied.
        let h = history(vec![
            e(0, 0, w("a"), Outcome::Ok, 0, 100),
            e(1, 1, Op::Read, read(Some("a")), 10, 20),
            e(2, 2, Op::Read, read(None), 30, 40),
        ]);
        assert!(matches!(check(&h), Verdict::NotLinearizable(_)));
    }

    #[test]
    fn the_same_reads_in_the_other_order_are_fine() {
        let h = history(vec![
            e(0, 0, w("a"), Outcome::Ok, 0, 100),
            e(1, 1, Op::Read, read(None), 10, 20),
            e(2, 2, Op::Read, read(Some("a")), 30, 40),
        ]);
        assert_eq!(check(&h), Verdict::Linearizable);
    }

    #[test]
    fn a_value_nobody_ever_wrote_is_caught() {
        let h = history(vec![
            e(0, 0, w("a"), Outcome::Ok, 0, 10),
            e(1, 1, Op::Read, read(Some("ghost")), 20, 30),
        ]);
        assert!(matches!(check(&h), Verdict::NotLinearizable(_)));
    }

    #[test]
    fn an_operation_with_no_answer_may_have_happened_or_not() {
        // The write never answered, so a read seeing either value is fine.
        for seen in [None, Some("a")] {
            let h = history(vec![
                e(0, 0, w("a"), Outcome::Unknown, 0, 0),
                e(1, 1, Op::Read, read(seen), 100, 110),
            ]);
            assert_eq!(check(&h), Verdict::Linearizable, "seen = {seen:?}");
        }
    }

    #[test]
    fn a_lost_write_that_later_appears_is_still_linearizable() {
        // This is the real fault-injection case: the client timed out, the write went
        // through anyway, and a later read sees it.
        let h = history(vec![
            e(0, 0, w("a"), Outcome::Unknown, 0, 0),
            e(1, 1, Op::Read, read(None), 10, 20),
            e(2, 1, Op::Read, read(Some("a")), 30, 40),
        ]);
        assert_eq!(check(&h), Verdict::Linearizable);
    }

    #[test]
    fn an_acknowledged_write_that_vanishes_is_not_linearizable() {
        // The write was acknowledged, so no later read may miss it: this is exactly the
        // "lost acknowledged write" a leader kill must never produce.
        let h = history(vec![
            e(0, 0, w("a"), Outcome::Ok, 0, 10),
            e(1, 1, Op::Read, read(Some("a")), 20, 30),
            e(2, 1, Op::Read, read(None), 40, 50),
        ]);
        assert!(matches!(check(&h), Verdict::NotLinearizable(_)));
    }

    #[test]
    fn compare_and_swap_respects_the_value_it_compared() {
        let ok = history(vec![
            e(0, 0, w("a"), Outcome::Ok, 0, 10),
            e(
                1,
                1,
                Op::Cas {
                    expect: Some(b"a".to_vec()),
                    new: b"b".to_vec(),
                },
                Outcome::Swapped(true),
                20,
                30,
            ),
            e(2, 0, Op::Read, read(Some("b")), 40, 50),
        ]);
        assert_eq!(check(&ok), Verdict::Linearizable);

        let bad = history(vec![
            e(0, 0, w("a"), Outcome::Ok, 0, 10),
            e(
                1,
                1,
                Op::Cas {
                    expect: Some(b"zzz".to_vec()),
                    new: b"b".to_vec(),
                },
                Outcome::Swapped(true),
                20,
                30,
            ),
        ]);
        assert!(matches!(check(&bad), Verdict::NotLinearizable(_)));
    }

    #[test]
    fn two_compare_and_swaps_cannot_both_win() {
        // Classic lost update: both clients swapped away from "a", which only one can do.
        let h = history(vec![
            e(0, 0, w("a"), Outcome::Ok, 0, 10),
            e(
                1,
                1,
                Op::Cas {
                    expect: Some(b"a".to_vec()),
                    new: b"b".to_vec(),
                },
                Outcome::Swapped(true),
                20,
                60,
            ),
            e(
                2,
                2,
                Op::Cas {
                    expect: Some(b"a".to_vec()),
                    new: b"c".to_vec(),
                },
                Outcome::Swapped(true),
                20,
                60,
            ),
        ]);
        assert!(matches!(check(&h), Verdict::NotLinearizable(_)));
    }

    #[test]
    fn a_failed_compare_and_swap_pins_the_state_too() {
        let h = history(vec![
            e(0, 0, w("a"), Outcome::Ok, 0, 10),
            e(
                1,
                1,
                Op::Cas {
                    expect: Some(b"a".to_vec()),
                    new: b"b".to_vec(),
                },
                Outcome::Swapped(false),
                20,
                30,
            ),
        ]);
        assert!(matches!(check(&h), Verdict::NotLinearizable(_)));
    }

    #[test]
    fn keys_are_independent() {
        let mut h = History::new();
        h.push(Entry {
            key: "a".into(),
            ..e(0, 0, w("1"), Outcome::Ok, 0, 10)
        });
        h.push(Entry {
            key: "b".into(),
            ..e(0, 0, Op::Read, read(None), 20, 30)
        });
        h.push(Entry {
            key: "a".into(),
            ..e(0, 0, Op::Read, read(Some("1")), 40, 50)
        });
        assert_eq!(check(&h), Verdict::Linearizable);
        assert_eq!(h.keys(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn a_broken_key_is_found_among_healthy_ones() {
        let mut h = History::new();
        for k in ["a", "b"] {
            h.push(Entry {
                key: k.into(),
                ..e(0, 0, w("1"), Outcome::Ok, 0, 10)
            });
            h.push(Entry {
                key: k.into(),
                ..e(0, 0, Op::Read, read(Some("1")), 20, 30)
            });
        }
        h.push(Entry {
            key: "c".into(),
            ..e(0, 0, w("1"), Outcome::Ok, 0, 10)
        });
        h.push(Entry {
            key: "c".into(),
            ..e(0, 1, Op::Read, read(None), 20, 30)
        });
        let Verdict::NotLinearizable(v) = check(&h) else {
            panic!("the broken key must be reported");
        };
        assert_eq!(v.key, "c");
    }

    #[test]
    fn the_report_names_the_smallest_offending_prefix() {
        // Twelve healthy operations, then one impossible read.
        let mut entries = Vec::new();
        let mut t = 0;
        for i in 0..12 {
            entries.push(e(i, 0, w(&format!("v{i}")), Outcome::Ok, t, t + 5));
            t += 10;
        }
        entries.push(e(12, 1, Op::Read, read(Some("v0")), t, t + 5));
        let Verdict::NotLinearizable(v) = check(&history(entries)) else {
            panic!("must fail");
        };
        assert_eq!(v.culprit.id, 12);
        assert!(
            v.entries.len() <= 13,
            "the prefix must not grow past the history"
        );
        let text = v.render();
        assert!(text.contains(">>"), "the culprit must be marked:\n{text}");
        assert!(text.contains("states explored"), "{text}");
    }

    #[test]
    fn concurrent_writes_and_reads_stay_tractable() {
        // Four clients, ten rounds each: the search must finish quickly.
        let mut entries = Vec::new();
        let mut id = 0;
        for round in 0..10u64 {
            for client in 0..4usize {
                let call = round * 100 + client as u64;
                entries.push(e(
                    id,
                    client,
                    w(&format!("r{round}c{client}")),
                    Outcome::Ok,
                    call,
                    call + 50,
                ));
                id += 1;
            }
            let call = round * 100 + 60;
            entries.push(e(id, 0, Op::Read, Outcome::Unknown, call, call));
            id += 1;
        }
        let started = std::time::Instant::now();
        assert_eq!(check(&history(entries)), Verdict::Linearizable);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the checker took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_budget_is_respected_and_reported() {
        let mut entries = Vec::new();
        for i in 0..40 {
            entries.push(e(i, i % 8, w(&format!("v{i}")), Outcome::Ok, 0, 10_000));
        }
        entries.push(e(40, 0, Op::Read, read(Some("nothing-like-this")), 20_000, 20_010));
        match check_with_budget(&history(entries), 500) {
            Verdict::Inconclusive { key, states } => {
                assert_eq!(key, "k");
                assert!(states >= 500, "{states}");
            }
            other => panic!("a tiny budget must be inconclusive, got {other:?}"),
        }
    }

    #[test]
    fn an_inconclusive_verdict_is_not_a_failure() {
        assert!(Verdict::Inconclusive {
            key: "k".into(),
            states: 1
        }
        .ok());
        assert!(Verdict::Linearizable.ok());
    }

    #[test]
    fn entries_render_with_their_window_and_result() {
        let line = e(3, 2, w("hello"), Outcome::Ok, 1_000_000, 2_500_000).render(0);
        assert!(line.contains("#3"), "{line}");
        assert!(line.contains("c2"), "{line}");
        assert!(line.contains("write hello"), "{line}");
        assert!(line.contains("1.000"), "{line}");
        assert!(line.contains("2.500"), "{line}");
        let lost = e(4, 0, w("x"), Outcome::Unknown, 0, 0).render(0);
        assert!(lost.contains("never"), "{lost}");
        assert!(lost.contains("NO ANSWER"), "{lost}");
    }

    #[test]
    fn a_recorder_marks_unknown_operations_as_never_returning() {
        let r = Recorder::new();
        let entry = r.entry(0, "k", Op::Read, Outcome::Unknown, 5, 9, "m1");
        assert_eq!(entry.ret_ns, u64::MAX);
        assert_eq!(entry.note, "m1");
        let entry = r.entry(0, "k", Op::Read, read(None), 5, 9, "");
        assert_eq!(entry.ret_ns, 9);
    }
}
