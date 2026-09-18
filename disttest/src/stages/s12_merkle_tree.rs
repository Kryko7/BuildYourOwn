//! Stage 12 — Merkle tree build.
//!
//! Unlike the two sketches before it, nothing here is a distribution: the hashes are pinned
//! by the specification, so the tester computes every one of them itself and compares
//! digests. `leaf = sha256(0x00 || bytes)` and `node = sha256(0x01 || lowercase-hex(left) ||
//! lowercase-hex(right))` — the two prefix bytes are domain separation, and they are the
//! reason a leaf holding the concatenation of two others cannot forge their parent.
//!
//! Three details account for almost every failure of this stage. The children are hashed as
//! their lower-case hex *text*, not as the raw 32 bytes, because that is what the answers
//! carry over the wire and a tree the tester cannot recompute is not testable. An odd node
//! at a level is carried up unchanged rather than paired with itself, which changes the root
//! of every tree with an odd count anywhere in it. And the empty tree is `sha256("")`, not
//! an absent root, not sixty-four zeros.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::{oracles, PrimProc};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 12.
pub fn stage() -> Stage {
    Stage {
        number: 12,
        slug: "merkle_tree",
        name: "Merkle tree build",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `merkle`: leaf = sha256(0x00 || leaf bytes), node = sha256(0x01 || left hex || right hex)",
            "An odd node at a level is carried up unchanged, not duplicated",
            "The root changes when any leaf changes, and only then",
            "The same leaves always produce the same root, whatever order they were added in",
        ],
        examples,
        tests: vec![
            Test::new("the root of four leaves is the one the oracle computes", four_leaves),
            Test::new("a tree of one leaf is that leaf's hash", one_leaf),
            Test::new("the root of an empty tree is the hash of nothing", no_leaves),
            Test::new("an odd number of leaves carries the last node up", odd_counts),
            Test::new("level zero holds the leaf hashes", level_zero_is_the_leaves),
            Test::new("the top level holds the root", the_top_is_the_root),
            Test::new("changing one leaf changes the root and nothing else", one_changed_leaf),
            Test::new("the same leaves always give the same root", rebuilding_is_stable),
            Test::new("trees of every size up to seventeen match the oracle", every_small_size).ext(),
            Test::new("every node of every level matches the oracle", every_node).ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "A tree of four leaves, and the nodes under its root",
            "merkle",
            || {
                lines(&[
                    "build t a,b,c,d",
                    "root t",
                    "node t 0 0",
                    "node t 1 0",
                    "node t 2 0",
                ])
            },
        )
        .request("four leaves, then the root and one node from each level")
        .response("a 64-character lower-case hex hash each time; level 2 node 0 is the root")
        .note(
            "Level 0 is the leaves and the top level holds the single root, so `node t 2 0` \
             and `root t` are the same string. `node t 0 0` is sha256(0x00 || \"a\") and \
             nothing else: the 0x00 is what stops a leaf from ever colliding with a node.",
        ),
        prim_example(
            "Three leaves, and the one that is carried up",
            "merkle",
            || lines(&["build o a,b,c", "node o 1 1", "node o 0 2", "root o"]),
        )
        .request("an odd tree, then the lone node at level 1 and the leaf it came from")
        .response("`node o 1 1` and `node o 0 2` are the same hash")
        .note(
            "`c` has no sibling, so it rises to level 1 untouched. Hashing it with itself \
             instead would be the other obvious rule and gives a different root — and it is \
             the rule behind the CVE-2012-2459 duplicate-transaction bug in Bitcoin, where \
             two different leaf lists hash to the same tree.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Shared moves
// ---------------------------------------------------------------------------------------

/// The leaf names a tree of `n` leaves is built from, under a tag that keeps trees apart.
fn names(tag: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{tag}{i}")).collect()
}

/// The same names as the byte vectors the oracle takes.
fn as_leaves(names: &[String]) -> Vec<Vec<u8>> {
    names.iter().map(|s| s.as_bytes().to_vec()).collect()
}

/// The comma-separated argument `build` takes. An empty list has no spelling, so a tree of
/// no leaves is built by leaving the argument off altogether.
fn arg(names: &[String]) -> String {
    names.join(",")
}

/// Build a tree and hand back the root the program answered with.
async fn build(p: &mut PrimProc, id: &str, names: &[String]) -> Result<String, Failure> {
    let cmd = if names.is_empty() {
        format!("build {id}")
    } else {
        format!("build {id} {}", arg(names))
    };
    let v = p.send(&cmd).await?;
    let leaves = p.expect_i64(&v, &cmd, "leaves")?;
    if leaves != names.len() as i64 {
        return Err(p.shape(
            &cmd,
            &format!("leaves should be {}, got {leaves}", names.len()),
        ));
    }
    p.expect_str(&v, &cmd, "root")
}

/// Ask for one node hash.
async fn node(p: &mut PrimProc, id: &str, level: usize, index: usize) -> Result<String, Failure> {
    let cmd = format!("node {id} {level} {index}");
    let v = p.send(&cmd).await?;
    p.expect_str(&v, &cmd, "hash")
}

/// Assert a hash is the 64 lower-case hex characters the grammar promises.
fn check_shape(c: &mut Check, path: &str, hash: &str) {
    c.that(
        path,
        "64 characters of lower-case hex",
        hash.len() == 64
            && hash
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
        hash.to_string(),
    );
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(four_leaves, |ctx| {
    let leaves = names("leaf", 4);
    let want = oracles::merkle_root(&as_leaves(&leaves));
    let p = ctx.prim("merkle").await?;
    let built = build(p, "t", &leaves).await?;
    let asked = p.send("root t").await?;
    let again = p.expect_str(&asked, "root t", "root")?;

    let mut c = Check::new("the root of a four-leaf tree");
    check_shape(&mut c, "build.root", &built);
    c.eq("build.root", want.clone(), built);
    c.eq("root.root", want, again);
    c.note("two levels of pairing, no carry-up: the plainest tree there is, and the one to get right before anything else");
    c.finish()
});

dist_test!(one_leaf, |ctx| {
    let leaves = names("only", 1);
    let want = oracles::merkle_leaf(leaves[0].as_bytes());
    let p = ctx.prim("merkle").await?;
    let built = build(p, "one", &leaves).await?;
    let level0 = node(p, "one", 0, 0).await?;

    let mut c = Check::new("a tree with a single leaf");
    c.eq("build.root", want.clone(), built);
    c.eq("node 0 0", want, level0);
    c.note("with one leaf there is nothing to pair, so the root is the leaf hash itself and never sha256(0x01 || h || h)");
    c.finish()
});

dist_test!(no_leaves, |ctx| {
    let want = oracles::merkle_root(&[]);
    let p = ctx.prim("merkle").await?;
    let built = build(p, "empty", &[]).await?;
    let asked = p.send("root empty").await?;
    let again = p.expect_str(&asked, "root empty", "root")?;

    let mut c = Check::new("a tree with no leaves at all");
    c.eq("build.root", want.clone(), built);
    c.eq("root.root", want, again);
    c.note("sha256 of the empty string, e3b0c442...; an empty tree still has a root, because a replica with nothing in it still has to be comparable with one that has");
    c.finish()
});

dist_test!(odd_counts, |ctx| {
    let sizes = [3usize, 5, 7, 9, 11];
    let mut wanted = Vec::new();
    for n in sizes {
        wanted.push(oracles::merkle_root(&as_leaves(&names("odd", n))));
    }
    let p = ctx.prim("merkle").await?;
    let mut got = Vec::new();
    for (i, n) in sizes.iter().enumerate() {
        got.push(build(p, &format!("odd{i}"), &names("odd", *n)).await?);
    }

    let mut c = Check::new("trees whose levels do not divide evenly");
    for ((n, want), have) in sizes.iter().zip(wanted).zip(got) {
        c.eq(&format!("build.root for {n} leaves"), want, have);
    }
    c.note("three leaves give a level of two, then one: the lone node rises unchanged at every level it is lonely on");
    c.finish()
});

dist_test!(level_zero_is_the_leaves, |ctx| {
    let leaves = names("lv", 6);
    let want: Vec<String> = leaves
        .iter()
        .map(|l| oracles::merkle_leaf(l.as_bytes()))
        .collect();
    let p = ctx.prim("merkle").await?;
    build(p, "lv", &leaves).await?;
    let mut got = Vec::new();
    for i in 0..leaves.len() {
        got.push(node(p, "lv", 0, i).await?);
    }

    let mut c = Check::new("level zero of a six-leaf tree");
    for (i, (want, have)) in want.iter().zip(&got).enumerate() {
        c.eq(&format!("node 0 {i}"), want.clone(), have.clone());
    }
    c.note("level 0 is the leaf hashes, not the leaf bytes and not the first level of pairs");
    c.finish()
});

dist_test!(the_top_is_the_root, |ctx| {
    let sizes = [1usize, 2, 4, 5, 8];
    let mut tops = Vec::new();
    for n in sizes {
        let levels = oracles::merkle_levels(&as_leaves(&names("top", n)));
        tops.push((
            levels.len() - 1,
            oracles::merkle_root(&as_leaves(&names("top", n))),
        ));
    }
    let p = ctx.prim("merkle").await?;
    let mut got = Vec::new();
    for (i, n) in sizes.iter().enumerate() {
        let id = format!("top{i}");
        let root = build(p, &id, &names("top", *n)).await?;
        let top = node(p, &id, tops[i].0, 0).await?;
        got.push((root, top));
    }

    let mut c = Check::new("the single node at the top of each tree");
    for ((n, (level, want)), (root, top)) in sizes.iter().zip(&tops).zip(&got) {
        c.eq(
            &format!("build.root for {n} leaves"),
            want.clone(),
            root.clone(),
        );
        c.eq(
            &format!("node {level} 0 for {n} leaves"),
            want.clone(),
            top.clone(),
        );
    }
    c.note("the top level always holds exactly one node, and that node is what `root` answers");
    c.finish()
});

dist_test!(one_changed_leaf, |ctx| {
    let n = 8usize;
    let victim = 5usize;
    let before = names("ch", n);
    let mut after = before.clone();
    after[victim] = "different".to_string();
    let want_before = oracles::merkle_root(&as_leaves(&before));
    let want_after = oracles::merkle_root(&as_leaves(&after));
    let want_leaves: Vec<String> = after
        .iter()
        .map(|l| oracles::merkle_leaf(l.as_bytes()))
        .collect();
    let p = ctx.prim("merkle").await?;
    let root_a = build(p, "a", &before).await?;
    let root_b = build(p, "b", &after).await?;
    let mut leaves_a = Vec::new();
    let mut leaves_b = Vec::new();
    for i in 0..n {
        leaves_a.push(node(p, "a", 0, i).await?);
        leaves_b.push(node(p, "b", 0, i).await?);
    }

    let mut c = Check::new("one leaf out of eight replaced");
    c.eq("build.root (before)", want_before.clone(), root_a);
    c.eq("build.root (after)", want_after.clone(), root_b);
    c.ne(
        "build.root (after) != build.root (before)",
        want_before,
        want_after,
    );
    for i in 0..n {
        c.eq(
            &format!("b.node 0 {i}"),
            want_leaves[i].clone(),
            leaves_b[i].clone(),
        );
        if i == victim {
            c.ne(
                &format!("node 0 {i} moved"),
                leaves_a[i].clone(),
                leaves_b[i].clone(),
            );
        } else {
            c.eq(
                &format!("node 0 {i} untouched"),
                leaves_a[i].clone(),
                leaves_b[i].clone(),
            );
        }
    }
    c.note("a leaf change has to reach the root, and it must not reach any leaf but its own — that is exactly what makes the next stage's descent sound");
    c.finish()
});

dist_test!(rebuilding_is_stable, |ctx| {
    let leaves = names("st", 7);
    let want = oracles::merkle_root(&as_leaves(&leaves));
    let p = ctx.prim("merkle").await?;
    let first = build(p, "x", &leaves).await?;
    let second = build(p, "y", &leaves).await?;
    let third = build(p, "x", &leaves).await?;
    let asked = p.send("root y").await?;
    let fourth = p.expect_str(&asked, "root y", "root")?;

    let mut c = Check::new("the same seven leaves built three times");
    c.eq("build.root (first)", want.clone(), first);
    c.eq("build.root (under another id)", want.clone(), second);
    c.eq("build.root (rebuilt over the first)", want.clone(), third);
    c.eq("root.root", want, fourth);
    c.note("nothing about a tree may depend on how many trees were built before it, or on a hasher carried over between builds");
    c.finish()
});

dist_test!(every_small_size, |ctx| {
    let mut sizes: Vec<usize> = (1..=17).collect();
    // A few larger sizes from the seed, so the stage is not only ever asked about 1..17.
    for _ in 0..3 {
        sizes.push(ctx.rng.random_range(18..200));
    }
    let seed = ctx.seed;
    let wanted: Vec<String> = sizes
        .iter()
        .map(|n| oracles::merkle_root(&as_leaves(&names("sz", *n))))
        .collect();
    let p = ctx.prim("merkle").await?;
    let mut got = Vec::new();
    for (i, n) in sizes.iter().enumerate() {
        got.push(build(p, &format!("s{i}"), &names("sz", *n)).await?);
    }

    let mut c = Check::new("every leaf count from one to seventeen, and a few larger ones");
    for ((n, want), have) in sizes.iter().zip(wanted).zip(got) {
        c.eq(&format!("build.root for {n} leaves"), want, have);
    }
    c.note(format!("the three larger counts come from --seed {seed}"));
    c.finish()
});

dist_test!(every_node, |ctx| {
    let leaves = names("full", 9);
    let want = oracles::merkle_levels(&as_leaves(&leaves));
    let p = ctx.prim("merkle").await?;
    build(p, "full", &leaves).await?;
    let mut got: Vec<Vec<String>> = Vec::new();
    for (level, nodes) in want.iter().enumerate() {
        let mut row = Vec::new();
        for index in 0..nodes.len() {
            row.push(node(p, "full", level, index).await?);
        }
        got.push(row);
    }

    let mut c = Check::new("all twenty-one nodes of a nine-leaf tree");
    for (level, nodes) in want.iter().enumerate() {
        for (index, hash) in nodes.iter().enumerate() {
            c.eq(
                &format!("node {level} {index}"),
                hash.clone(),
                got[level][index].clone(),
            );
        }
    }
    c.note("nine leaves make levels of 9, 5, 3, 2 and 1, so a carry-up happens on three of the four steps");
    c.finish()
});
