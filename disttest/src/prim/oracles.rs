//! The oracles: what the answer *should* be, worked out by the tester itself.
//!
//! Nothing here is a second copy of the learner's program. Each oracle is either
//!
//! * a brute-force computation where brute force is the specification (an exact distinct
//!   count against a sketch, every permutation of a delivery order against a CRDT, an
//!   O(n²) componentwise comparison against a vector clock), or
//! * closed-form arithmetic the specification pins down (quorum overlap, full jitter, the
//!   token and leaky bucket, the Merkle hash), or
//! * a statistical envelope, when the answer is a distribution rather than a value — in
//!   which case the measured value always goes into the failure block next to the bound.

use sha2::Digest;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------------------
// Causality
// ---------------------------------------------------------------------------------------

/// How two vector clocks relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// `a` happened before `b`.
    Before,
    /// `a` happened after `b`.
    After,
    /// The two are identical.
    Equal,
    /// Neither precedes the other.
    Concurrent,
}

impl Relation {
    /// The word the protocol uses on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Relation::Before => "before",
            Relation::After => "after",
            Relation::Equal => "equal",
            Relation::Concurrent => "concurrent",
        }
    }
}

/// A vector clock: process name → counter. A missing process counts as zero.
pub type Clock = BTreeMap<String, i64>;

/// Compare two vector clocks the slow, obvious way: every component of the union.
pub fn compare_clocks(a: &Clock, b: &Clock) -> Relation {
    let mut le = true;
    let mut ge = true;
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        let (x, y) = (*a.get(k).unwrap_or(&0), *b.get(k).unwrap_or(&0));
        if x > y {
            le = false;
        }
        if x < y {
            ge = false;
        }
    }
    match (le, ge) {
        (true, true) => Relation::Equal,
        (true, false) => Relation::Before,
        (false, true) => Relation::After,
        (false, false) => Relation::Concurrent,
    }
}

/// Build a clock from pairs.
pub fn clock(pairs: &[(&str, i64)]) -> Clock {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), *v))
        .collect()
}

/// Is a message with clock `msg` from `sender` deliverable at a process whose clock is
/// `local`? The causal-broadcast rule: exactly one new message from the sender, and nothing
/// the receiver has not already seen.
pub fn causally_deliverable(local: &Clock, sender: &str, msg: &Clock) -> bool {
    let want = local.get(sender).unwrap_or(&0) + 1;
    if *msg.get(sender).unwrap_or(&0) != want {
        return false;
    }
    msg.iter()
        .filter(|(k, _)| k.as_str() != sender)
        .all(|(k, v)| *v <= *local.get(k).unwrap_or(&0))
}

// ---------------------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------------------

/// What changed in a key→node mapping.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Movement {
    /// Keys that stayed where they were.
    pub stayed: usize,
    /// Keys that moved to a node that is new in the second mapping.
    pub moved_to_new: usize,
    /// Keys that moved away from a node that disappeared.
    pub moved_off_gone: usize,
    /// Keys that moved between two nodes present in both mappings — never acceptable for a
    /// consistent hash, and the whole point of the scheme.
    pub moved_between_survivors: Vec<String>,
}

/// Compare two placements of the same keys.
pub fn movement(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
    nodes_before: &[String],
    nodes_after: &[String],
) -> Movement {
    let mut m = Movement::default();
    for (key, was) in before {
        let Some(now) = after.get(key) else { continue };
        if was == now {
            m.stayed += 1;
        } else if !nodes_before.contains(now) {
            m.moved_to_new += 1;
        } else if !nodes_after.contains(was) {
            m.moved_off_gone += 1;
        } else {
            m.moved_between_survivors.push(key.clone());
        }
    }
    m
}

/// The largest deviation from a perfectly even split, as a ratio of the mean.
pub fn imbalance(counts: &[usize]) -> f64 {
    if counts.is_empty() {
        return 0.0;
    }
    let total: usize = counts.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let mean = total as f64 / counts.len() as f64;
    counts
        .iter()
        .map(|c| ((*c as f64 - mean) / mean).abs())
        .fold(0.0, f64::max)
}

// ---------------------------------------------------------------------------------------
// Quorums
// ---------------------------------------------------------------------------------------

/// Does a read of `r` and a write of `w` out of `n` always overlap?
pub fn quorum_overlaps(n: i64, r: i64, w: i64) -> bool {
    r + w > n
}

/// Can two writes of `w` out of `n` run without overlapping?
pub fn write_conflict_possible(n: i64, w: i64) -> bool {
    2 * w <= n
}

/// The smallest `r` that overlaps every write of `w` out of `n`.
pub fn smallest_read_quorum(n: i64, w: i64) -> i64 {
    (n - w + 1).max(1)
}

// ---------------------------------------------------------------------------------------
// Sketches
// ---------------------------------------------------------------------------------------

/// The theoretical false-positive rate of a Bloom filter: `(1 - e^(-kn/m))^k`.
pub fn bloom_fpr(bits: f64, hashes: f64, inserted: f64) -> f64 {
    if bits <= 0.0 || hashes <= 0.0 {
        return 1.0;
    }
    (1.0 - (-hashes * inserted / bits).exp()).powf(hashes)
}

/// The number of hash functions that minimises the false-positive rate.
pub fn bloom_optimal_k(bits: f64, inserted: f64) -> f64 {
    if inserted <= 0.0 {
        return 1.0;
    }
    (bits / inserted) * std::f64::consts::LN_2
}

/// HyperLogLog's standard error for a precision of `p` registers bits: `1.04 / sqrt(2^p)`.
pub fn hll_standard_error(p: u32) -> f64 {
    1.04 / (2f64.powi(p as i32)).sqrt()
}

// ---------------------------------------------------------------------------------------
// Merkle trees
// ---------------------------------------------------------------------------------------

/// The hash of a leaf: `sha256(0x00 || leaf)`.
pub fn merkle_leaf(leaf: &[u8]) -> String {
    let mut h = sha2::Sha256::new();
    h.update([0u8]);
    h.update(leaf);
    hex(&h.finalize())
}

/// The hash of an internal node: `sha256(0x01 || left || right)` over the hex children.
pub fn merkle_node(left: &str, right: &str) -> String {
    let mut h = sha2::Sha256::new();
    h.update([1u8]);
    h.update(left.as_bytes());
    h.update(right.as_bytes());
    hex(&h.finalize())
}

/// Every level of the tree, leaves first. An odd node is carried up unchanged.
pub fn merkle_levels(leaves: &[Vec<u8>]) -> Vec<Vec<String>> {
    if leaves.is_empty() {
        return vec![vec![hex(&sha2::Sha256::digest([]))]];
    }
    let mut levels = vec![leaves.iter().map(|l| merkle_leaf(l)).collect::<Vec<_>>()];
    while levels.last().map(Vec::len).unwrap_or(0) > 1 {
        let last = levels.last().cloned().unwrap_or_default();
        let mut next = Vec::new();
        let mut i = 0;
        while i < last.len() {
            if i + 1 < last.len() {
                next.push(merkle_node(&last[i], &last[i + 1]));
                i += 2;
            } else {
                next.push(last[i].clone());
                i += 1;
            }
        }
        levels.push(next);
    }
    levels
}

/// The root hash of a list of leaves.
pub fn merkle_root(leaves: &[Vec<u8>]) -> String {
    merkle_levels(leaves)
        .last()
        .and_then(|l| l.first())
        .cloned()
        .unwrap_or_default()
}

/// The indices at which two leaf lists differ, as inclusive ranges.
pub fn differing_ranges(a: &[Vec<u8>], b: &[Vec<u8>]) -> Vec<(usize, usize)> {
    let n = a.len().max(b.len());
    let mut out: Vec<(usize, usize)> = Vec::new();
    for i in 0..n {
        if a.get(i) != b.get(i) {
            match out.last_mut() {
                Some(last) if last.1 + 1 == i => last.1 = i,
                _ => out.push((i, i)),
            }
        }
    }
    out
}

/// How many node comparisons a perfect top-down walk needs to find those differences.
///
/// The bound a stage asserts, not an exact count: any sane descent visits at most the
/// nodes on the paths to the differing leaves, and there are at most `log2(n) + 1` of them
/// per leaf.
pub fn descent_bound(leaves: usize, differing: usize) -> usize {
    if differing == 0 {
        return 1;
    }
    let depth = (usize::BITS - leaves.next_power_of_two().leading_zeros()) as usize + 1;
    2 * depth * differing + 2
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------------------

/// Exponential backoff with full jitter: `u * min(cap, base * 2^attempt)`.
pub fn full_jitter(base_ms: f64, cap_ms: f64, attempt: u32, u: f64) -> f64 {
    let ceiling = cap_ms.min(base_ms * 2f64.powi(attempt as i32));
    u * ceiling
}

/// One `take` against a token bucket: tokens refill at `rate` per second, capped at `burst`.
#[derive(Debug, Clone, Copy)]
pub struct TokenBucket {
    /// Tokens added per second.
    pub rate: f64,
    /// The most tokens the bucket may hold.
    pub burst: f64,
    /// Tokens currently held.
    pub tokens: f64,
    /// The time of the last operation, in milliseconds.
    pub last_ms: f64,
}

impl TokenBucket {
    /// A full bucket at time zero.
    pub fn new(rate: f64, burst: f64) -> TokenBucket {
        TokenBucket {
            rate,
            burst,
            tokens: burst,
            last_ms: 0.0,
        }
    }

    /// Try to take `n` tokens at `t_ms`; returns whether it was allowed.
    pub fn take(&mut self, t_ms: f64, n: f64) -> bool {
        let elapsed = (t_ms - self.last_ms).max(0.0);
        self.tokens = (self.tokens + self.rate * elapsed / 1000.0).min(self.burst);
        self.last_ms = t_ms;
        if self.tokens + 1e-9 >= n {
            self.tokens -= n;
            true
        } else {
            false
        }
    }
}

/// A leaky bucket as a queue: work drains at `rate` per second, and is refused when the
/// queue would pass `capacity`.
#[derive(Debug, Clone, Copy)]
pub struct LeakyBucket {
    /// Units drained per second.
    pub rate: f64,
    /// The most the queue may hold.
    pub capacity: f64,
    /// What is queued now.
    pub queued: f64,
    /// The time of the last operation, in milliseconds.
    pub last_ms: f64,
}

impl LeakyBucket {
    /// An empty bucket at time zero.
    pub fn new(rate: f64, capacity: f64) -> LeakyBucket {
        LeakyBucket {
            rate,
            capacity,
            queued: 0.0,
            last_ms: 0.0,
        }
    }

    /// Offer `n` units at `t_ms`; returns whether they were accepted.
    pub fn offer(&mut self, t_ms: f64, n: f64) -> bool {
        let elapsed = (t_ms - self.last_ms).max(0.0);
        self.queued = (self.queued - self.rate * elapsed / 1000.0).max(0.0);
        self.last_ms = t_ms;
        if self.queued + n <= self.capacity + 1e-9 {
            self.queued += n;
            true
        } else {
            false
        }
    }
}

/// The state a SWIM-style failure detector should be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suspicion {
    /// Heard from recently enough.
    Alive,
    /// Missed the suspicion deadline but not the death one.
    Suspect,
    /// Missed the death deadline.
    Dead,
}

impl Suspicion {
    /// The word the protocol uses on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Suspicion::Alive => "alive",
            Suspicion::Suspect => "suspect",
            Suspicion::Dead => "dead",
        }
    }
}

/// What a node's state must be, given when it was last heard from.
pub fn suspicion(since_last_ack_ms: i64, suspect_after_ms: i64, dead_after_ms: i64) -> Suspicion {
    if since_last_ack_ms < suspect_after_ms {
        Suspicion::Alive
    } else if since_last_ack_ms < dead_after_ms {
        Suspicion::Suspect
    } else {
        Suspicion::Dead
    }
}

// ---------------------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------------------

/// Every permutation of `0..n`, for replaying a delivery order.
///
/// Used by the CRDT stages: a state-based CRDT's merge must be commutative, associative and
/// idempotent, so *every* order of the same updates has to end at the same value. Six
/// elements is 720 orders, which is a second of work and plenty to catch a merge that
/// quietly depends on arrival order.
pub fn permutations(n: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut current: Vec<usize> = (0..n).collect();
    permute(&mut current, 0, &mut out);
    out
}

fn permute(items: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
    if k == items.len() {
        out.push(items.clone());
        return;
    }
    for i in k..items.len() {
        items.swap(k, i);
        permute(items, k + 1, out);
        items.swap(k, i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_comparison_covers_every_relation() {
        let a = clock(&[("a", 1), ("b", 2)]);
        let b = clock(&[("a", 2), ("b", 3)]);
        assert_eq!(compare_clocks(&a, &b), Relation::Before);
        assert_eq!(compare_clocks(&b, &a), Relation::After);
        assert_eq!(compare_clocks(&a, &a), Relation::Equal);
        let c = clock(&[("a", 3), ("b", 1)]);
        assert_eq!(compare_clocks(&a, &c), Relation::Concurrent);
        // A missing component is a zero, not an absence.
        assert_eq!(
            compare_clocks(&clock(&[("a", 1)]), &clock(&[("a", 1), ("b", 0)])),
            Relation::Equal
        );
        assert_eq!(
            compare_clocks(&clock(&[]), &clock(&[("a", 1)])),
            Relation::Before
        );
    }

    #[test]
    fn causal_delivery_waits_for_the_gap() {
        let local = clock(&[("a", 1), ("b", 0)]);
        assert!(causally_deliverable(&local, "a", &clock(&[("a", 2)])));
        assert!(
            !causally_deliverable(&local, "a", &clock(&[("a", 3)])),
            "a message from the future must wait"
        );
        assert!(
            !causally_deliverable(&local, "a", &clock(&[("a", 1)])),
            "a message already delivered is not deliverable again"
        );
        assert!(
            !causally_deliverable(&local, "b", &clock(&[("a", 2), ("b", 1)])),
            "a dependency the receiver has not seen must hold the message back"
        );
        assert!(causally_deliverable(&local, "b", &clock(&[("a", 1), ("b", 1)])));
    }

    #[test]
    fn movement_notices_a_key_shuffled_between_survivors() {
        let before: BTreeMap<String, String> =
            [("k1", "n1"), ("k2", "n2"), ("k3", "n1")]
                .into_iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect();
        let after: BTreeMap<String, String> =
            [("k1", "n1"), ("k2", "n3"), ("k3", "n2")]
                .into_iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect();
        let nodes_before = vec!["n1".to_string(), "n2".to_string()];
        let nodes_after = vec!["n1".to_string(), "n2".to_string(), "n3".to_string()];
        let m = movement(&before, &after, &nodes_before, &nodes_after);
        assert_eq!(m.stayed, 1);
        assert_eq!(m.moved_to_new, 1);
        assert_eq!(m.moved_between_survivors, vec!["k3".to_string()]);
    }

    #[test]
    fn imbalance_is_zero_for_an_even_split() {
        assert!(imbalance(&[10, 10, 10]).abs() < 1e-9);
        assert!((imbalance(&[20, 10, 0]) - 1.0).abs() < 1e-9);
        assert_eq!(imbalance(&[]), 0.0);
    }

    #[test]
    fn quorum_arithmetic_matches_the_textbook() {
        assert!(quorum_overlaps(3, 2, 2));
        assert!(!quorum_overlaps(3, 1, 2));
        assert!(quorum_overlaps(5, 3, 3));
        assert!(!write_conflict_possible(3, 2));
        assert!(write_conflict_possible(4, 2));
        assert_eq!(smallest_read_quorum(3, 2), 2);
        assert_eq!(smallest_read_quorum(5, 5), 1);
    }

    #[test]
    fn bloom_and_hll_bounds_are_the_published_formulas() {
        // 10 bits per item with 7 hashes is the classic ~0.8 % filter.
        let fpr = bloom_fpr(10_000.0, 7.0, 1_000.0);
        assert!((0.007..0.009).contains(&fpr), "{fpr}");
        assert!((bloom_optimal_k(10_000.0, 1_000.0) - 6.93).abs() < 0.01);
        assert!((hll_standard_error(14) - 0.008125).abs() < 1e-6);
    }

    #[test]
    fn merkle_hashes_are_stable_and_domain_separated() {
        assert_eq!(
            merkle_leaf(b"a"),
            "022a6979e6dab7aa5ae4c3e5e45f7e977112a7e63593820dbec1ec738a24f93c"
        );
        assert_ne!(merkle_leaf(b"ab"), merkle_node(&merkle_leaf(b"a"), &merkle_leaf(b"b")));
        let leaves: Vec<Vec<u8>> = ["a", "b", "c"].iter().map(|s| s.as_bytes().to_vec()).collect();
        let levels = merkle_levels(&leaves);
        assert_eq!(levels[0].len(), 3);
        assert_eq!(levels[1].len(), 2, "the odd leaf is carried up");
        assert_eq!(levels[2].len(), 1);
        assert_eq!(merkle_root(&leaves), levels[2][0]);
        assert_eq!(merkle_root(&[]).len(), 64);
    }

    #[test]
    fn differing_ranges_are_merged_when_adjacent() {
        let a: Vec<Vec<u8>> = (0..8).map(|i| vec![i as u8]).collect();
        let mut b = a.clone();
        b[2] = vec![99];
        b[3] = vec![98];
        b[6] = vec![97];
        assert_eq!(differing_ranges(&a, &b), vec![(2, 3), (6, 6)]);
        assert_eq!(differing_ranges(&a, &a), Vec::new());
        assert!(descent_bound(8, 3) > 3);
    }

    #[test]
    fn full_jitter_is_uniform_below_the_capped_ceiling() {
        assert!((full_jitter(100.0, 10_000.0, 0, 1.0) - 100.0).abs() < 1e-9);
        assert!((full_jitter(100.0, 10_000.0, 3, 1.0) - 800.0).abs() < 1e-9);
        assert!((full_jitter(100.0, 500.0, 5, 1.0) - 500.0).abs() < 1e-9, "the cap wins");
        assert!((full_jitter(100.0, 10_000.0, 3, 0.5) - 400.0).abs() < 1e-9);
        assert_eq!(full_jitter(100.0, 10_000.0, 3, 0.0), 0.0);
    }

    #[test]
    fn a_token_bucket_refills_at_its_rate() {
        let mut b = TokenBucket::new(10.0, 5.0);
        assert!(b.take(0.0, 5.0), "a full bucket allows the whole burst");
        assert!(!b.take(0.0, 1.0), "and nothing more until it refills");
        assert!(b.take(100.0, 1.0), "100 ms at 10/s is one token");
        assert!(!b.take(100.0, 1.0));
        // The cap holds however long the bucket idles.
        assert!(b.take(100_000.0, 5.0));
        assert!(!b.take(100_000.0, 1.0));
    }

    #[test]
    fn a_leaky_bucket_refuses_what_would_overflow() {
        let mut b = LeakyBucket::new(10.0, 3.0);
        assert!(b.offer(0.0, 3.0));
        assert!(!b.offer(0.0, 1.0));
        assert!(b.offer(200.0, 2.0), "200 ms at 10/s drains two units");
        assert!(!b.offer(200.0, 2.0));
    }

    #[test]
    fn suspicion_has_three_states_with_sharp_edges() {
        assert_eq!(suspicion(999, 1000, 3000), Suspicion::Alive);
        assert_eq!(suspicion(1000, 1000, 3000), Suspicion::Suspect);
        assert_eq!(suspicion(2999, 1000, 3000), Suspicion::Suspect);
        assert_eq!(suspicion(3000, 1000, 3000), Suspicion::Dead);
        assert_eq!(Suspicion::Suspect.as_str(), "suspect");
    }

    #[test]
    fn permutations_are_complete_and_distinct() {
        let p = permutations(4);
        assert_eq!(p.len(), 24);
        let mut sorted = p.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 24);
        assert_eq!(permutations(0), vec![Vec::<usize>::new()]);
        assert_eq!(permutations(6).len(), 720);
    }
}
