//! Stage 13 — Merkle diff with the fewest round trips.
//!
//! Stage 12 built the tree; this is what the tree was for. Two replicas that hold nearly the
//! same data want to find the handful of keys they disagree about without shipping either
//! side's contents, and a Merkle tree turns that into a descent: compare the roots, and
//! recurse only into the children whose hashes differ.
//!
//! So the interesting assertion in this stage is not `ranges`, which any implementation that
//! compares the leaves pairwise gets right. It is `compared`. A diff that walks the whole
//! tree has learnt nothing a linear scan would not have told it, and the whole point of the
//! structure is that the work is logarithmic in the size of the tree and linear only in the
//! number of differences. Every test here that checks the ranges also checks the cost, with
//! `oracles::descent_bound` as the ceiling and the total node count as the thing to stay
//! well under.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::{oracles, PrimProc};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use serde_json::Value;

/// Stage 13.
pub fn stage() -> Stage {
    Stage {
        number: 13,
        slug: "merkle_diff",
        name: "Merkle diff with the fewest round trips",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Compare roots first: equal roots mean equal trees and no further work",
            "Descend only into subtrees whose hashes differ — that is the whole saving",
            "Report the differing leaves as ranges, so a caller can fetch them in one go",
            "Count the nodes you compared: a walk that visits everything has learnt nothing",
        ],
        examples,
        tests: vec![
            Test::new(
                "two identical trees diff to no ranges at all",
                identical_trees,
            ),
            Test::new(
                "diffing a tree against itself costs one comparison",
                against_itself,
            ),
            Test::new("one differing leaf is found exactly", one_differing_leaf),
            Test::new(
                "adjacent differing leaves merge into one range",
                adjacent_leaves_merge,
            ),
            Test::new(
                "scattered differences match the ranges the oracle computes",
                scattered_differences,
            ),
            Test::new(
                "a diff never looks at every node in the tree",
                cheaper_than_a_full_walk,
            ),
            Test::new(
                "a large tree with two scattered differences is still cheap",
                a_large_tree,
            ),
            Test::new("the diff is the same in both directions", symmetric).ext(),
            Test::new(
                "two trees that differ everywhere report one whole range",
                differ_everywhere,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Two replicas that agree", "merkle", || {
            lines(&[
                "build a a,b,c,d,e,f,g,h",
                "build b a,b,c,d,e,f,g,h",
                "diff a b",
            ])
        })
        .request("two eight-leaf trees built from the same data")
        .response("no ranges, and exactly one comparison")
        .note(
            "The roots match, so there is nothing below them to look at. This is the case \
             anti-entropy spends almost all of its time in, and it has to cost one hash \
             comparison and not fifteen.",
        ),
        prim_example("Two replicas that disagree about one key", "merkle", || {
            lines(&[
                "build a a,b,c,d,e,f,g,h",
                "build b a,b,c,d,e,X,g,h",
                "diff a b",
            ])
        })
        .request("the same trees with leaf 5 changed on one side")
        .response("the single range [5, 5], found in seven comparisons rather than fifteen")
        .note(
            "Seven is 1 + 2 per level: the root, then the two children at each of the three \
             levels below it, of which only one differs and is followed. `compared` is the \
             assertion this stage is really about — a diff that answers [[5,5]] after \
             reading all fifteen nodes has done a linear scan with extra steps.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Shared moves
// ---------------------------------------------------------------------------------------

/// What one `diff` answered: the ranges, and how many node hashes it looked at.
struct Diff {
    ranges: Vec<(usize, usize)>,
    compared: usize,
}

/// The leaf names of a tree of `n` leaves.
fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("v{i}")).collect()
}

/// The same names as the byte vectors the oracle takes.
fn as_leaves(names: &[String]) -> Vec<Vec<u8>> {
    names.iter().map(|s| s.as_bytes().to_vec()).collect()
}

/// Build one tree under an id.
async fn build(p: &mut PrimProc, id: &str, names: &[String]) -> Result<(), Failure> {
    let cmd = format!("build {id} {}", names.join(","));
    p.send(&cmd).await?;
    Ok(())
}

/// Run one diff and decode both fields of the answer.
async fn diff(p: &mut PrimProc, a: &str, b: &str) -> Result<Diff, Failure> {
    let cmd = format!("diff {a} {b}");
    let v = p.send(&cmd).await?;
    let compared = p.expect_i64(&v, &cmd, "compared")?;
    if compared < 0 {
        return Err(p.shape(&cmd, "compared is not negative"));
    }
    let Some(Value::Array(items)) = v.get("ranges") else {
        return Err(p.shape(&cmd, "ranges should be an array of [lo, hi] pairs"));
    };
    let mut ranges = Vec::new();
    for item in items {
        let Some(pair) = item.as_array().filter(|a| a.len() == 2) else {
            return Err(p.shape(&cmd, &format!("{item} is not a [lo, hi] pair")));
        };
        let (Some(lo), Some(hi)) = (pair[0].as_u64(), pair[1].as_u64()) else {
            return Err(p.shape(&cmd, &format!("{item} is not a pair of whole numbers")));
        };
        if hi < lo {
            return Err(p.shape(&cmd, &format!("{item} runs backwards")));
        }
        ranges.push((lo as usize, hi as usize));
    }
    Ok(Diff {
        ranges,
        compared: compared as usize,
    })
}

/// How many node hashes the whole tree holds, which is what a diff must stay under.
fn total_nodes(leaves: usize) -> usize {
    oracles::merkle_levels(&as_leaves(&names(leaves)))
        .iter()
        .map(Vec::len)
        .sum()
}

/// How many leaves the oracle's ranges cover.
fn covered(ranges: &[(usize, usize)]) -> usize {
    ranges.iter().map(|(lo, hi)| hi + 1 - lo).sum()
}

/// Build both sides of a comparison and diff them, with the oracle's answer alongside.
async fn compare(
    p: &mut PrimProc,
    left: &[String],
    right: &[String],
) -> Result<(Diff, Vec<(usize, usize)>), Failure> {
    build(p, "l", left).await?;
    build(p, "r", right).await?;
    let got = diff(p, "l", "r").await?;
    let want = oracles::differing_ranges(&as_leaves(left), &as_leaves(right));
    Ok((got, want))
}

/// Assert the ranges and the cost of one diff at once.
fn check(c: &mut Check, what: &str, got: &Diff, want: &[(usize, usize)], leaves: usize) {
    c.eq(&format!("{what}.ranges"), want.to_vec(), got.ranges.clone());
    c.observe(&format!("{what}.compared"), got.compared);
    c.at_most(
        &format!("{what}.compared"),
        oracles::descent_bound(leaves, covered(want)),
        got.compared,
    );
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(identical_trees, |ctx| {
    let leaves = names(16);
    let p = ctx.prim("merkle").await?;
    let (got, want) = compare(p, &leaves, &leaves).await?;

    let mut c = Check::new("two trees built from the same sixteen leaves");
    check(&mut c, "diff", &got, &want, 16);
    c.eq("diff.ranges", Vec::new(), got.ranges.clone());
    c.eq("diff.compared", 1, got.compared);
    c.note("equal roots settle it: there is no honest reason to read a second hash, and the bound the oracle gives for zero differences is one");
    c.finish()
});

dist_test!(against_itself, |ctx| {
    let leaves = names(64);
    let p = ctx.prim("merkle").await?;
    build(p, "self", &leaves).await?;
    let got = diff(p, "self", "self").await?;

    let mut c = Check::new("a tree diffed against itself");
    c.eq("diff.ranges", Vec::new(), got.ranges);
    c.eq("diff.compared", 1, got.compared);
    c.note("sixty-four leaves and one hundred and twenty-seven nodes, and the answer is settled by the first comparison");
    c.finish()
});

dist_test!(one_differing_leaf, |ctx| {
    let n = 16usize;
    let mut results = Vec::new();
    let p = ctx.prim("merkle").await?;
    for victim in [0usize, 5, 15] {
        let left = names(n);
        let mut right = left.clone();
        right[victim] = format!("changed-{victim}");
        let (got, want) = compare(p, &left, &right).await?;
        results.push((victim, got, want));
    }

    let mut c = Check::new("one leaf out of sixteen changed, at three positions");
    for (victim, got, want) in &results {
        c.eq(
            &format!("diff[leaf {victim}].ranges"),
            vec![(*victim, *victim)],
            got.ranges.clone(),
        );
        check(&mut c, &format!("diff[leaf {victim}]"), got, want, n);
        c.at_most(
            &format!("diff[leaf {victim}].compared vs a full walk"),
            total_nodes(n) / 2,
            got.compared,
        );
    }
    c.note("a single differing leaf is one range of one, whether it sits at an edge of the tree or in the middle of it");
    c.finish()
});

dist_test!(adjacent_leaves_merge, |ctx| {
    let n = 32usize;
    let left = names(n);
    let mut right = left.clone();
    for i in [9usize, 10, 11] {
        right[i] = format!("moved-{i}");
    }
    let p = ctx.prim("merkle").await?;
    let (got, want) = compare(p, &left, &right).await?;

    let mut c = Check::new("three neighbouring leaves changed");
    c.eq("oracle.ranges", vec![(9usize, 11usize)], want.clone());
    check(&mut c, "diff", &got, &want, n);
    c.note("the ranges are the point of the answer: a caller fetches [9, 11] in one request, where three separate ranges would be three");
    c.finish()
});

dist_test!(scattered_differences, |ctx| {
    let n = 64usize;
    let left = names(n);
    let mut right = left.clone();
    // A seeded pattern, so a failure names the seed that produced it.
    let mut victims: Vec<usize> = Vec::new();
    while victims.len() < 6 {
        let i = ctx.rng.random_range(0..n);
        if !victims.contains(&i) {
            victims.push(i);
        }
    }
    for i in &victims {
        right[*i] = format!("x-{i}");
    }
    victims.sort_unstable();
    let seed = ctx.seed;
    let p = ctx.prim("merkle").await?;
    let (got, want) = compare(p, &left, &right).await?;

    let mut c = Check::new("six leaves out of sixty-four changed at seeded positions");
    check(&mut c, "diff", &got, &want, n);
    c.at_most("diff.compared vs a full walk", total_nodes(n), got.compared);
    c.note(format!(
        "the changed leaves are {victims:?}, from --seed {seed}"
    ));
    c.note("adjacent victims have to come back merged and distant ones separately, which is the only thing that distinguishes a range list from a leaf list");
    c.finish()
});

dist_test!(cheaper_than_a_full_walk, |ctx| {
    let n = 64usize;
    let left = names(n);
    let mut right = left.clone();
    right[40] = "only-this-one".to_string();
    let nodes = total_nodes(n);
    let p = ctx.prim("merkle").await?;
    let (got, want) = compare(p, &left, &right).await?;

    let mut c = Check::new("the cost of finding one difference in sixty-four leaves");
    check(&mut c, "diff", &got, &want, n);
    c.observe("tree.nodes", nodes);
    c.at_most("diff.compared", nodes / 4, got.compared);
    c.note(format!(
        "the tree holds {nodes} node hashes; a descent reads about 1 + 2 log2(n) of them, so \
         anything near {nodes} means the subtrees that matched were followed anyway"
    ));
    c.finish()?;
    ctx.note(format!(
        "64 leaves, one difference: {} comparisons out of {nodes} nodes",
        got.compared
    ));
    Ok(())
});

dist_test!(a_large_tree, |ctx| {
    let n = 256usize;
    let left = names(n);
    let mut right = left.clone();
    right[3] = "early".to_string();
    right[200] = "late".to_string();
    let nodes = total_nodes(n);
    let p = ctx.prim("merkle").await?;
    let (got, want) = compare(p, &left, &right).await?;

    let mut c = Check::new("two differences at opposite ends of a 256-leaf tree");
    c.eq(
        "diff.ranges",
        vec![(3usize, 3usize), (200usize, 200usize)],
        got.ranges.clone(),
    );
    check(&mut c, "diff", &got, &want, n);
    c.observe("tree.nodes", nodes);
    c.at_most("diff.compared", nodes / 4, got.compared);
    c.note("two differences in different halves share only the root, so this is the most a two-difference descent can cost, and it is still a small fraction of the tree");
    c.finish()?;
    ctx.note(format!(
        "256 leaves, two differences: {} comparisons out of {nodes} nodes",
        got.compared
    ));
    Ok(())
});

dist_test!(symmetric, |ctx| {
    let n = 32usize;
    let left = names(n);
    let mut right = left.clone();
    for i in [2usize, 3, 17] {
        right[i] = format!("s-{i}");
    }
    let p = ctx.prim("merkle").await?;
    let (forwards, want) = compare(p, &left, &right).await?;
    let backwards = diff(p, "r", "l").await?;

    let mut c = Check::new("the same pair of trees diffed in both orders");
    check(&mut c, "diff(l, r)", &forwards, &want, n);
    c.eq(
        "diff(r, l).ranges",
        forwards.ranges.clone(),
        backwards.ranges,
    );
    c.eq("diff(r, l).compared", forwards.compared, backwards.compared);
    c.note("the descent is over pairs of hashes, and a pair has no direction; an answer that depends on which side was named first is carrying state it should not have");
    c.finish()
});

dist_test!(differ_everywhere, |ctx| {
    let n = 8usize;
    let left = names(n);
    let right: Vec<String> = (0..n).map(|i| format!("w{i}")).collect();
    let nodes = total_nodes(n);
    let p = ctx.prim("merkle").await?;
    let (got, want) = compare(p, &left, &right).await?;

    let mut c = Check::new("two trees with nothing in common");
    c.eq("diff.ranges", vec![(0usize, n - 1)], got.ranges.clone());
    check(&mut c, "diff", &got, &want, n);
    c.observe("tree.nodes", nodes);
    c.at_most("diff.compared", nodes, got.compared);
    c.note("when everything differs the descent does read the whole tree, and that is correct: the saving was never unconditional, only proportional to how much the two sides agree");
    c.finish()
});
