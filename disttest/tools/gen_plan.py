#!/usr/bin/env python3
"""Regenerate PLAN.md from the stage registry.

    cd disttest && ./target/release/disttest --list --json | python3 tools/gen_plan.py

PLAN.md is the tickbox plan `--list` reads, so this script only ever rewrites the stage
entries: the tickbox state of every stage that already exists is carried over, and a new
stage starts unticked.
"""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PLAN = os.path.join(HERE, "PLAN.md")

HEADER = """# disttest stage plan

Tick a stage when `disttest --target my_node --stage N` is green. `disttest --list` reads
these boxes. Each entry names its **ladder** — `primitives`, `node` or `cluster` — its source
file and its test count. Stages marked **[ext]** go beyond the core track, and `--skip-ext`
hides them.

Run one stage: `disttest --target my_node --stage 22` — everything so far: `--until 35` —
one ladder: `--tag cluster --all` — the lot: `--all`. Prove the suite itself:
`disttest --target etcd --validate --all`, which routes each ladder to its own reference.

Every stage also carries 1-3 **worked examples**: a request and what the reference answered,
a transcript of a conversation with the primitives CLI, or a recorded history and the
linearizability checker's verdict on it. They live in the stage's own file, are recaptured
with `disttest --capture-examples examples/captured.json --target etcd`, and reach the site
through `catalog.json`. See README.md, "Adding a stage".
"""

LADDER_BLURB = {
    "primitives": (
        "Small deterministic exercises in a line-oriented CLI: `./your_program.sh <topic>`, "
        "one command per line in, one JSON object per line out. The tester works every "
        "answer out independently — brute force, closed-form arithmetic, or a statistical "
        "bound with the measured value printed next to it."
    ),
    "node": (
        "One server speaking a subset of the etcd v3 HTTP/JSON API, validated against real "
        "etcd 3.7.1. Revisions, MVCC, transactions, compaction, leases, watches — and a "
        "durability stage that kills the process mid-workload."
    ),
    "cluster": (
        "Three or five of those servers, with a userspace TCP proxy in front of every peer "
        "URL so the harness can partition, delay, drop, duplicate and reorder peer traffic "
        "with no privileges. It ends with a seeded fault schedule and a linearizability "
        "check over the recorded history."
    ),
}


def existing_ticks(path):
    ticks = {}
    if not os.path.exists(path):
        return ticks
    with open(path) as f:
        for line in f:
            m = re.match(r"^- \[([ xX])\] \*\*Stage (\d+)", line)
            if m:
                ticks[int(m.group(2))] = m.group(1) != " "
    return ticks


def main():
    catalog = json.load(sys.stdin)
    ticks = existing_ticks(PLAN)
    by_number = {s["number"]: s for s in catalog["stages"]}

    out = [HEADER]
    seen_ladders = set()
    for section in catalog["sections"]:
        title = section["title"]
        out.append("\n## %s. %s\n" % (section["id"].upper(), title))
        ladder = None
        for number in section["stages"]:
            stage = by_number.get(number)
            if stage is None:
                continue
            ladder = stage["ladder"]
        if ladder and ladder not in seen_ladders:
            seen_ladders.add(ladder)
            out.append("%s\n\n" % LADDER_BLURB[ladder])
        for number in section["stages"]:
            stage = by_number.get(number)
            if stage is None:
                out.append("- [ ] **Stage %02d** — (planned)\n" % number)
                continue
            box = "x" if ticks.get(number) else " "
            ext = " **[ext]**" if stage["ext"] else ""
            tests = len(stage["tests"])
            ext_tests = sum(1 for t in stage["tests"] if t.get("ext"))
            count = "%d test%s" % (tests, "" if tests == 1 else "s")
            if ext_tests and not stage["ext"]:
                count += ", %d ext" % ext_tests
            out.append(
                "- [%s] **Stage %02d** — %s%s `%s` (`%s`, %s)\n"
                % (box, number, stage["name"], ext, stage["ladder"], stage["file"], count)
            )
            for hint in stage["hints"]:
                out.append("  - %s\n" % hint)

    totals = {}
    for s in catalog["stages"]:
        t = totals.setdefault(s["ladder"], [0, 0])
        t[0] += 1
        t[1] += len(s["tests"])
    out.append("\n## Totals\n\n")
    out.append("| ladder | stages | tests |\n|---|---:|---:|\n")
    for ladder in ("primitives", "node", "cluster"):
        if ladder in totals:
            out.append(
                "| `%s` | %d | %d |\n" % (ladder, totals[ladder][0], totals[ladder][1])
            )
    out.append(
        "| **all** | **%d** | **%d** |\n"
        % (
            sum(v[0] for v in totals.values()),
            sum(v[1] for v in totals.values()),
        )
    )

    with open(PLAN, "w") as f:
        f.write("".join(out))
    print("wrote %s: %d stages" % (PLAN, len(catalog["stages"])))


if __name__ == "__main__":
    main()
