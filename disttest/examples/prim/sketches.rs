//! Reference topics: sketches — `bloom`, `hll` and `merkle`. See
//! `examples/reference_primitives.rs`.
//!
//! Reading this file spoils stages 10 to 13. It exists so that
//! `disttest --target reference_primitives --validate --stage 10` can prove the suite's own
//! expectations, and for no other reason: nothing here is packed, tuned or clever.
//!
//! Two of the three topics are pinned by the specification and one is not. The Merkle
//! hashes are fixed to the byte (`leaf = sha256(0x00 || bytes)`,
//! `node = sha256(0x01 || lowercase-hex(left) || lowercase-hex(right))`), so this file must
//! agree with `disttest::prim::oracles` exactly, and the stage compares the two. The Bloom
//! filter and the HyperLogLog only have to be *a* good hash: they are checked against a
//! statistical envelope, never against a fixed digest, so the plain FNV-1a-plus-avalanche
//! below is written out here rather than pulled from a crate.

use crate::{err, int, word, Topic};
use serde_json::{json, Value};
use sha2::Digest;
use std::collections::BTreeMap;

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "bloom" => Some(Box::new(Bloom::new())),
        "hll" => Some(Box::new(Hll::new())),
        "merkle" => Some(Box::new(Merkle::new())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// A deterministic hash, written out rather than imported
// ---------------------------------------------------------------------------------------

/// FNV-1a over the bytes, then the splitmix64 finalizer.
///
/// FNV alone is fine for equality but its low bits move far too little for either sketch:
/// a Bloom filter derives `k` positions from it and HyperLogLog counts leading zeros in it,
/// and both want every output bit to flip independently. The three-line avalanche does that.
fn hash64(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ seed;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut z = h;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

// ---------------------------------------------------------------------------------------
// bloom
// ---------------------------------------------------------------------------------------

/// A Bloom filter over `m` bits with `k` positions per item.
struct Bloom {
    bits: Vec<bool>,
    hashes: u32,
    items: u64,
}

impl Bloom {
    fn new() -> Bloom {
        Bloom {
            bits: vec![false; 1024],
            hashes: 3,
            items: 0,
        }
    }

    /// The `k` positions of one item, by Kirsch and Mitzenmacher: two hashes are enough,
    /// because `h1 + i * h2` is as good as `k` independent functions for this purpose.
    fn positions(&self, item: &str) -> Vec<usize> {
        let m = self.bits.len() as u64;
        if m == 0 {
            return Vec::new();
        }
        let h1 = hash64(0, item.as_bytes());
        let h2 = hash64(0x9e37_79b9, item.as_bytes()) | 1;
        (0..u64::from(self.hashes))
            .map(|i| (h1.wrapping_add(i.wrapping_mul(h2)) % m) as usize)
            .collect()
    }
}

impl Topic for Bloom {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let (bits, hashes) = match (int(args, 0), int(args, 1)) {
                    (Ok(b), Ok(h)) => (b, h),
                    (Err(e), _) | (_, Err(e)) => return e,
                };
                if !(1..=1 << 24).contains(&bits) || !(1..=64).contains(&hashes) {
                    return err("bits must be 1..=16777216 and hashes 1..=64");
                }
                self.bits = vec![false; bits as usize];
                self.hashes = hashes as u32;
                self.items = 0;
                json!({ "bits": bits, "hashes": hashes })
            }
            "add" => {
                let item = match word(args, 0) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                for p in self.positions(&item) {
                    self.bits[p] = true;
                }
                self.items += 1;
                json!({ "ok": true })
            }
            "contains" => {
                let item = match word(args, 0) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let maybe = self.positions(&item).into_iter().all(|p| self.bits[p]);
                json!({ "maybe": maybe })
            }
            "stats" => json!({
                "bits": self.bits.len(),
                "hashes": self.hashes,
                "set_bits": self.bits.iter().filter(|b| **b).count(),
                "items": self.items,
            }),
            other => err(format!("bloom: unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// hll
// ---------------------------------------------------------------------------------------

/// A HyperLogLog over `2^p` registers.
struct Hll {
    p: u32,
    registers: Vec<u8>,
}

impl Hll {
    fn new() -> Hll {
        Hll {
            p: 14,
            registers: vec![0; 1 << 14],
        }
    }

    /// The bias constant for `m` registers.
    fn alpha(m: f64) -> f64 {
        match m as u64 {
            16 => 0.673,
            32 => 0.697,
            64 => 0.709,
            _ => 0.7213 / (1.0 + 1.079 / m),
        }
    }
}

impl Topic for Hll {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let p = match int(args, 0) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                if !(4..=18).contains(&p) {
                    return err("p must be between 4 and 18");
                }
                self.p = p as u32;
                self.registers = vec![0u8; 1usize << self.p];
                json!({ "registers": self.registers.len() })
            }
            "add" => {
                let item = match word(args, 0) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let h = hash64(0x5151, item.as_bytes());
                let index = (h >> (64 - self.p)) as usize;
                // The rank is the position of the first one bit in what is left after the
                // index has been taken off the top. A zero tail would give 65, which no
                // register may hold, so it is clamped to the longest run that can occur.
                let tail = h << self.p;
                let rank = ((tail.leading_zeros() + 1) as u8).min((64 - self.p + 1) as u8);
                if self.registers[index] < rank {
                    self.registers[index] = rank;
                }
                json!({ "ok": true })
            }
            "count" => {
                let m = self.registers.len() as f64;
                let sum: f64 = self
                    .registers
                    .iter()
                    .map(|r| 2f64.powi(-i32::from(*r)))
                    .sum();
                let zeros = self.registers.iter().filter(|r| **r == 0).count();
                let raw = Hll::alpha(m) * m * m / sum;
                // Below roughly 2.5 m the raw estimator is badly biased; linear counting
                // over the empty registers is exact enough there, and is the only thing
                // that makes a small set come back as itself.
                let estimate = if raw <= 2.5 * m && zeros > 0 {
                    m * (m / zeros as f64).ln()
                } else {
                    raw
                };
                json!({ "estimate": estimate.round() as u64 })
            }
            other => err(format!("hll: unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// merkle
// ---------------------------------------------------------------------------------------

/// Lower-case hex, the only form a hash is ever written or hashed in here.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `sha256(0x00 || leaf)`.
fn leaf_hash(leaf: &str) -> String {
    let mut h = sha2::Sha256::new();
    h.update([0u8]);
    h.update(leaf.as_bytes());
    hex(&h.finalize())
}

/// `sha256(0x01 || left || right)` over the two children written as lower-case hex.
fn node_hash(left: &str, right: &str) -> String {
    let mut h = sha2::Sha256::new();
    h.update([1u8]);
    h.update(left.as_bytes());
    h.update(right.as_bytes());
    hex(&h.finalize())
}

/// Every level, leaves first. An odd node is carried up unchanged, never paired with itself.
fn levels(leaves: &[String]) -> Vec<Vec<String>> {
    if leaves.is_empty() {
        return vec![vec![hex(&sha2::Sha256::digest([]))]];
    }
    let mut out = vec![leaves.iter().map(|l| leaf_hash(l)).collect::<Vec<String>>()];
    while out.last().map_or(0, Vec::len) > 1 {
        let last = out.last().cloned().unwrap_or_default();
        let mut next = Vec::new();
        let mut i = 0;
        while i < last.len() {
            if i + 1 < last.len() {
                next.push(node_hash(&last[i], &last[i + 1]));
                i += 2;
            } else {
                next.push(last[i].clone());
                i += 1;
            }
        }
        out.push(next);
    }
    out
}

/// A handful of named trees, each remembered as its leaves.
struct Merkle {
    trees: BTreeMap<String, Vec<String>>,
}

impl Merkle {
    fn new() -> Merkle {
        Merkle {
            trees: BTreeMap::new(),
        }
    }

    fn tree(&self, id: &str) -> Option<Vec<Vec<String>>> {
        self.trees.get(id).map(|l| levels(l))
    }
}

/// Walk two trees of the same shape from the top, descending only where the hashes differ.
///
/// `compared` counts the node hashes that were looked at, which is the whole point of the
/// exercise: a walk that reads every node has learnt nothing a plain leaf scan would not.
fn descend(
    a: &[Vec<String>],
    b: &[Vec<String>],
    level: usize,
    index: usize,
    compared: &mut usize,
    differing: &mut Vec<usize>,
) {
    let (Some(x), Some(y)) = (a[level].get(index), b[level].get(index)) else {
        return;
    };
    *compared += 1;
    if x == y {
        return;
    }
    if level == 0 {
        differing.push(index);
        return;
    }
    descend(a, b, level - 1, index * 2, compared, differing);
    descend(a, b, level - 1, index * 2 + 1, compared, differing);
}

/// Collapse sorted leaf indices into inclusive ranges.
fn ranges(indices: &[usize]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for i in indices {
        match out.last_mut() {
            Some(last) if last.1 + 1 == *i => last.1 = *i,
            _ => out.push((*i, *i)),
        }
    }
    out
}

impl Topic for Merkle {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "build" => {
                let id = match word(args, 0) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                // No second argument is the empty tree: a list of leaves has no way of
                // spelling "none", because the line is split on whitespace.
                let leaves: Vec<String> = match args.get(1) {
                    Some(list) => list.split(',').map(str::to_string).collect(),
                    None => Vec::new(),
                };
                let root = levels(&leaves)
                    .last()
                    .and_then(|l| l.first())
                    .cloned()
                    .unwrap_or_default();
                let n = leaves.len();
                self.trees.insert(id, leaves);
                json!({ "root": root, "leaves": n })
            }
            "root" => {
                let id = match word(args, 0) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                match self.tree(&id) {
                    Some(levels) => json!({
                        "root": levels.last().and_then(|l| l.first()).cloned().unwrap_or_default()
                    }),
                    None => err(format!("no tree {id:?}")),
                }
            }
            "node" => {
                let id = match word(args, 0) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let (level, index) = match (int(args, 1), int(args, 2)) {
                    (Ok(l), Ok(i)) => (l, i),
                    (Err(e), _) | (_, Err(e)) => return e,
                };
                let Some(levels) = self.tree(&id) else {
                    return err(format!("no tree {id:?}"));
                };
                if level < 0 || index < 0 {
                    return err("level and index are not negative");
                }
                match levels
                    .get(level as usize)
                    .and_then(|l| l.get(index as usize))
                {
                    Some(h) => json!({ "hash": h }),
                    None => err(format!("tree {id:?} has no node {level}/{index}")),
                }
            }
            "diff" => {
                let (a, b) = match (word(args, 0), word(args, 1)) {
                    (Ok(a), Ok(b)) => (a, b),
                    (Err(e), _) | (_, Err(e)) => return e,
                };
                let (Some(la), Some(lb)) = (self.tree(&a), self.tree(&b)) else {
                    return err("diff needs two trees that have been built");
                };
                let mut compared = 0usize;
                let mut differing = Vec::new();
                if la[0].len() == lb[0].len() && la.len() == lb.len() {
                    let top = la.len() - 1;
                    descend(&la, &lb, top, 0, &mut compared, &mut differing);
                } else {
                    // Two different shapes share no internal nodes, so there is nothing to
                    // descend through; the leaves are the only comparable level.
                    for i in 0..la[0].len().max(lb[0].len()) {
                        compared += 1;
                        if la[0].get(i) != lb[0].get(i) {
                            differing.push(i);
                        }
                    }
                }
                differing.sort_unstable();
                let ranges: Vec<Value> = ranges(&differing)
                    .into_iter()
                    .map(|(lo, hi)| json!([lo, hi]))
                    .collect();
                json!({ "ranges": ranges, "compared": compared })
            }
            other => err(format!("merkle: unknown command {other:?}")),
        }
    }
}
