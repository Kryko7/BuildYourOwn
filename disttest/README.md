# disttest

A stage-by-stage, black-box conformance tester for a **distributed key/value system** you
write yourself. Track id `dist`.

> Commands below are run from the **repo root**: this is a cargo workspace, so every
> binary and example lands in the one `target/` directory at the top.
```
disttest --target my_node --stage 22
disttest --target my_node --tag algorithms --all
disttest --target my_node --until 35 --json report.json
disttest --target my_node --all --skip-ext
disttest --target etcd --validate --all          # must be all green — the suite's self-check
disttest --list                                  # stages, ladders, test counts, PLAN.md ticks
disttest --list --json > catalog.json            # the stage catalog the site reads
```

Nothing here implements the thing you are building. This repository holds the harness, the
suite, the oracles, the fault injector and the docs.

**88 stages, 830 tests.** 20 stages on the primitives ladder, 33 on the algorithms
ladder, 15 on the node ladder, 20 on the cluster ladder; 544 of the tests are the core
track and the rest are `[ext]`.

---

## 1. Four ladders

This track is deliberately **multi-level**. You do not start by writing a replicated store;
you start by writing a vector clock, you work your way through Raft and two-phase commit as
exercises in their own right, and you finish by surviving a partition while a linearizability
checker watches. Every stage carries a `ladder` tag, which is also a `--tag` value and a field
in `catalog.json`.

| ladder | stages | what your program is | reference for `--validate` |
|---|---|---|---|
| `primitives` | 01–20 | a line-oriented CLI, one JSON object per command | `reference_primitives` (this crate's own example) |
| `algorithms` | 56–88 | the same CLI, one topic per classic algorithm or pattern | `reference_algorithms` (this crate's own example) |
| `node` | 21–35 | one server speaking a subset of the etcd v3 HTTP/JSON API | real **etcd 3.7.1** |
| `cluster` | 36–55 | three or five of those servers, replicating | real **etcd 3.7.1** |

```
disttest --target my_node --tag primitives --all      # just ladder A
disttest --target my_node --tag algorithms --all      # just ladder B
disttest --target my_node --tag node --all            # just ladder C
disttest --target my_node --tag cluster --all         # just ladder D
```

**The algorithms ladder is numbered 56 and upward, and that is deliberate.** It is the second
rung a learner climbs, and it is listed second everywhere the ladders are listed, but stage
numbers 1–55 were already cited by the resource library and by work in flight, so the new
stages were appended rather than inserted. Nothing reads a stage number as a position in the
ladder order; `--tag` and the `ladder` field are what say where a stage belongs.

One program serves all four ladders. The harness decides which ladder it is asking for by the
arguments it appends to your `command` in `targets.yaml`: a bare topic name means "be a CLI"
— the topic says which of the two CLI ladders — and the etcd flag subset means "be a server".

---

## 2. Ladder A — primitives

### 2.1 The program contract

```
./your_program.sh <topic>
```

Then, on stdin, **one command per line**; on stdout, **exactly one JSON object per line**,
in the same order. Nothing else may go to stdout — diagnostics belong on stderr, which the
harness captures and shows under a failure.

* Numbers may be JSON numbers or JSON strings; the harness accepts both.
* A command it cannot make sense of should answer `{"ok": false, "error": "..."}` rather
  than crashing or staying silent. No stage depends on the wording.
* Any JSON that appears **as an argument** is compact — no spaces — because commands are
  split on whitespace.
* State lives in the process and is per topic. The harness starts a fresh process for every
  topic, and starts a second one when a stage needs two replicas that cannot see each other.

### 2.2 The topics

Each topic is a tiny language. The stage that uses it names it in its hints.

#### `lamport` — Lamport clocks (stage 01)

| command | answer |
|---|---|
| `local <p>` | `{"ts": n}` — `p` does something on its own |
| `send <p>` | `{"ts": n}` — the timestamp to put on the message |
| `recv <p> <ts>` | `{"ts": n}` — after receiving a message stamped `ts` |
| `get <p>` | `{"ts": n}` — the current value, unchanged |

#### `vector-clock` — vector clocks (stage 02)

| command | answer |
|---|---|
| `local <p>` | `{"vc": {"a": 1, ...}}` |
| `send <p>` | `{"vc": {...}}` |
| `recv <p> <vc-json>` | `{"vc": {...}}` — merge, then increment `p` |
| `get <p>` | `{"vc": {...}}` |
| `cmp <vc-json> <vc-json>` | `{"rel": "before" \| "after" \| "equal" \| "concurrent"}` |

A process not mentioned in a clock counts as zero.

#### `version-vector` — version vectors and siblings (stage 03)

| command | answer |
|---|---|
| `put <replica> <key> <value>` | `{"vv": {...}}` |
| `read <replica> <key>` | `{"siblings": [{"value": "...", "vv": {...}}, ...]}` sorted by value |
| `sync <a> <b>` | `{"ok": true}` — both replicas exchange everything |
| `put-ctx <replica> <key> <value> <vv-json>` | `{"vv": {...}}` — a write that claims to have seen `vv` |

#### `hlc` — hybrid logical clocks (stage 04)

| command | answer |
|---|---|
| `now <p> <wall>` | `{"l": n, "c": n}` |
| `recv <p> <wall> <l> <c>` | `{"l": n, "c": n}` |
| `get <p>` | `{"l": n, "c": n}` |

#### `consistent-hash` — a hash ring (stages 05, 06)

| command | answer |
|---|---|
| `add <node> [vnodes]` | `{"nodes": n, "vnodes": n}` — `vnodes` defaults to 128 |
| `remove <node>` | `{"nodes": n}` |
| `locate <key>` | `{"node": "n1"}` |
| `stats <count>` | `{"nodes": {"n1": k, ...}}` — where `key0..key<count-1>` land |

`vnodes` in the answer may be this node's share or the ring's total; no test pins which, only
that it is at least the number of points the node was given.

#### `rendezvous` — highest random weight (stage 07)

| command | answer |
|---|---|
| `add <node>` / `remove <node>` | `{"nodes": n}` |
| `locate <key>` | `{"node": "n1"}` |
| `locate-k <key> <k>` | `{"nodes": ["n1", "n4", ...]}` in descending score order |

#### `quorum` — quorum arithmetic (stages 08, 09)

| command | answer |
|---|---|
| `check <n> <r> <w>` | `{"overlap": bool, "write_conflict": bool, "min_r": n}` |
| `sloppy <n> <w> <alive>` | `{"accepted": bool, "hints": n, "strict": bool}` |
| `hint <owner> <standin> <key>` | `{"hints": n}` |
| `handoff <owner>` | `{"delivered": n, "hints": n}` |
| `hints <owner>` | `{"keys": ["k1", ...]}` |

`strict` is false exactly when the sloppy quorum used a stand-in, because a sloppy quorum is
not a quorum.

#### `bloom` — a Bloom filter (stage 10)

| command | answer |
|---|---|
| `init <bits> <hashes>` | `{"bits": n, "hashes": n}` |
| `add <item>` | `{"ok": true}` |
| `contains <item>` | `{"maybe": bool}` |
| `stats` | `{"bits": n, "hashes": n, "set_bits": n, "items": n}` |

#### `hll` — HyperLogLog (stage 11)

| command | answer |
|---|---|
| `init <p>` | `{"registers": n}` — `2^p` registers |
| `add <item>` | `{"ok": true}` |
| `count` | `{"estimate": n}` |

#### `merkle` — a Merkle tree (stages 12, 13)

| command | answer |
|---|---|
| `build <id> <leaf,leaf,...>` | `{"root": "<64 hex>", "leaves": n}`; `build <id>` alone is the empty tree |
| `root <id>` | `{"root": "<64 hex>"}` |
| `node <id> <level> <index>` | `{"hash": "<64 hex>"}` — level 0 is the leaves |
| `diff <a> <b>` | `{"ranges": [[lo, hi], ...], "compared": n}` |

The hashes are pinned so the tester can compute them itself:

```
leaf(i)    = sha256(0x00 || leaf bytes)
node(l, r) = sha256(0x01 || lowercase-hex(l) || lowercase-hex(r))
```

An odd node at a level is carried up unchanged. The root of an empty tree is `sha256("")`.
`compared` is how many node hashes the diff looked at; a diff that walks the whole tree has
learnt nothing and the stage says so.

#### `g-counter`, `pn-counter` — counters (stage 14)

| command | answer |
|---|---|
| `inc <replica> <n>` | `{"value": n}` |
| `dec <replica> <n>` | `{"value": n}` — `pn-counter` only |
| `value <replica>` | `{"value": n}` |
| `state <replica>` | `{"state": {...}}` — whatever shape you like, as long as it round-trips |
| `merge <dst> <state-json>` | `{"value": n}` |

#### `lww-register`, `or-set` — a register and a set (stage 15)

| command (`lww-register`) | answer |
|---|---|
| `set <replica> <value> <ts>` | `{"value": "...", "ts": n}` |
| `value <replica>` | `{"value": "..." \| null, "ts": n}` |
| `state <replica>` / `merge <dst> <state-json>` | `{"state": {...}}` / `{"value": ...}` |

| command (`or-set`) | answer |
|---|---|
| `add <replica> <element>` | `{"elements": [...]}` |
| `remove <replica> <element>` | `{"elements": [...]}` |
| `elements <replica>` | `{"elements": [...]}` sorted |
| `state <replica>` / `merge <dst> <state-json>` | `{"state": {...}}` / `{"elements": [...]}` |

Ties on equal timestamps are broken by the larger replica name, so two replicas always agree.

#### `rga` — a replicated sequence (stage 16)

| command | answer |
|---|---|
| `insert <replica> <after-id\|root> <id> <char>` | `{"text": "..."}` |
| `delete <replica> <id>` | `{"text": "..."}` |
| `text <replica>` | `{"text": "..."}` |
| `state <replica>` / `merge <dst> <state-json>` | `{"state": {...}}` / `{"text": "..."}` |

#### `snapshot` — Chandy–Lamport (stage 17)

| command | answer |
|---|---|
| `init <p> <balance>` | `{"processes": n}` |
| `channel <from> <to>` | `{"channels": n}` |
| `send <from> <to> <amount>` | `{"in_flight": n}` |
| `deliver <from> <to>` | `{"balance": n}` |
| `snapshot <p>` | `{"started": true}` |
| `result` | `{"complete": bool, "total": n, "balances": {...}, "channels": {...}}` |

#### `causal-broadcast` — causal delivery (stage 18)

| command | answer |
|---|---|
| `init <p,p,...>` | `{"processes": n}` |
| `bcast <p> <msg>` | `{"vc": {...}}` |
| `recv <p> <from> <msg> <vc-json>` | `{"delivered": ["m1", ...], "buffered": n}` |
| `delivered <p>` | `{"messages": ["m1", ...]}` in delivery order |

#### `token-bucket`, `leaky-bucket` — rate limiting (stage 19)

| command | answer |
|---|---|
| `init <rate> <burst-or-capacity>` | `{"ok": true}` |
| `take <t_ms> <n>` (token) | `{"allowed": bool, "tokens": x}` |
| `offer <t_ms> <n>` (leaky) | `{"accepted": bool, "queue": x}` |

Time always arrives in the command. Never read a clock of your own: that is what makes these
testable at all.

#### `backoff`, `phi-accrual`, `swim` — timing and failure detection (stage 20)

| command | answer |
|---|---|
| `init <base_ms> <cap_ms>` (backoff) | `{"ok": true}` |
| `next <attempt> <u>` (backoff) | `{"delay": x}` — `u` is a sample from `[0, 1)` |
| `heartbeat <t_ms>` (phi) | `{"samples": n}` |
| `phi <t_ms>` (phi) | `{"phi": x}` |
| `init <suspect_ms> <dead_ms>` (swim) | `{"ok": true}` |
| `ack <node> <t_ms>` (swim) | `{"ok": true}` |
| `status <node> <t_ms>` (swim) | `{"state": "alive" \| "suspect" \| "dead"}` |

Full jitter is `u * min(cap, base * 2^attempt)`, and the harness supplies `u`, so a jittered
delay is still exactly reproducible.

### 2.3 How the oracles work

The tester never asks your program what the answer is and then believes it. For every
primitives stage it works the answer out independently:

* **brute force, where brute force is the specification** — an exact distinct count against
  HyperLogLog, an O(n²) componentwise comparison against a vector clock, every permutation of
  a delivery order replayed against a CRDT (six updates is 720 orders), a simulated ring
  against the key-movement claims;
* **closed-form arithmetic the specification pins down** — quorum overlap, full jitter, both
  buckets, the Merkle hashes;
* **a statistical envelope**, when the answer is a distribution rather than a value — the
  Bloom filter's false-positive rate against `(1 - e^(-kn/m))^k`, HyperLogLog's relative
  error against `1.04 / sqrt(2^p)`. The measured value always appears in the failure block
  next to the bound, so a near miss is visible rather than mysterious.

The oracles live in `src/prim/oracles.rs` and have their own unit tests.

---

## 3. Ladder B — algorithms and patterns

The classics, as exercises in their own right. Raft, Paxos, two- and three-phase commit,
sagas, outboxes, fencing tokens, gossip, circuit breakers — none of them need a cluster to
learn, and every one of them is a state machine or a property you can drive by hand.

These are **deterministic, single-process exercises**: no sockets, no clusters, no timing
races. Where an algorithm is a state machine, the harness feeds it the events a real node
would have received (`request-vote`, `append-entries`, `timeout`, `crash`, `recover`) and
asserts the transition the specification mandates. Where it is a property — compensations run
in reverse, a fenced write is rejected, no two participants decide differently — the harness
asserts it over an exhaustive or seeded set of event orders.

### 3.1 The program contract

The same contract as ladder A, to the letter:

```
./your_program.sh <topic>
```

Then, on stdin, **one command per line**; on stdout, **exactly one JSON object per line**, in
the same order. Nothing else may go to stdout. Numbers may be JSON numbers or JSON strings.
A command your program cannot make sense of should answer `{"ok": false, "error": "..."}`.
JSON that appears **as an argument** is compact, because commands are split on whitespace.
State lives in the process and is per topic.

**Time never comes from a clock of your own.** Every topic that needs time takes it as an
argument, so a run under `--seed` is reproducible to the millisecond.

### 3.2 The topics

#### `raft-election` — Raft leader election (stage 56)

One node's election state machine. `<n>` is the cluster size; this node is always the one
being driven, and `voted_for` is `"self"` when it voted for itself.

| command | answer |
|---|---|
| `init <n>` | `{"ok": true, "members": n, "majority": m}` |
| `state` | `{"state": "follower"\|"candidate"\|"leader", "term": t, "voted_for": s\|null, "votes": k, "leader": s\|null, "last_index": i, "last_term": t}` |
| `log-append <term>` | `{"last_index": i, "last_term": t}` — one entry onto this node's own log |
| `timeout` | the new `state`; a leader ignores it |
| `request-vote <term> <candidate> <last_index> <last_term>` | `{"term": t, "granted": bool, "state": s}` |
| `vote-response <term> <voter> <true\|false>` | the new `state` |
| `append-entries <term> <leader>` | `{"term": t, "success": bool, "state": s}` |

Any message carrying a higher term makes this node a follower, raises its term and clears
`votedFor`. A vote is granted only when `votedFor` is free (or is already this candidate) and
the candidate's `(last_term, last_index)` is at least this node's. Each voter counts once.

#### `raft-log` — AppendEntries on the follower (stage 57)

A log is an array of **entry terms**, index 1-based.

| command | answer |
|---|---|
| `init <terms-json>` | `{"term": t, "last_index": i, "last_term": t, "commit_index": 0}` |
| `term <t>` | `{"term": t}` |
| `append <term> <prev_index> <prev_term> <leader_commit> <entries-json>` | `{"term": t, "success": bool, "last_index": i, "last_term": t, "commit_index": c, "conflict_index": i}` |
| `log` | `{"term": t, "terms": [...], "last_index": i, "last_term": t, "commit_index": c}` |

A failed consistency check leaves the log **untouched**; a successful one truncates only from
the first entry whose term conflicts, so a retransmitted prefix cannot eat the tail.
`commit_index` is `min(leader_commit, index of the last new entry)`. On a refusal for a stale
*term* the `conflict_index` is `0`: nothing can be inferred about a log the follower never
looked at.

#### `raft-commit` — matchIndex, nextIndex and §5.4.2 (stage 58)

| command | answer |
|---|---|
| `init <n> <term> <terms-json>` | `{"term": t, "last_index": i, "commit_index": 0, "majority": m}` |
| `append` | `{"last_index": i, "term": t}` — one entry of the leader's current term |
| `ack <peer> <index>` | `{"match_index": m, "next_index": i, "commit_index": c}` |
| `reject <peer> <conflict_index>` | `{"next_index": i}` |
| `commit-safe <index>` | `{"safe": bool, "replicas": k, "reason": "..."}` |
| `entry <index>` | `{"term": t}` |
| `state` | `{"term": t, "last_index": i, "commit_index": c, "match": {...}, "next": {...}}` |

`reason` is one of `"current term"`, `"earlier term"`, `"no majority"`, `"already committed"`
(which answers `safe: true` — the index is committed, so it is certainly safe) and
`"past the end of the log"`. Peers are fixed at `init` as `s2`..`sN`, so `state` has a
deterministic shape and an `ack` or `reject` naming anyone else is an error. The leader counts
itself towards the majority, `ack` is monotonic, and **an entry from an earlier term is never
committed by replica count alone** — that is Figure 8, and stage 58 walks the whole scenario.

#### `raft-snapshot` — compaction and InstallSnapshot (stage 59)

| command | answer |
|---|---|
| `init <terms-json>` | `{"first_index": 1, "last_index": i, "commit_index": 0, "snapshot_index": 0, "snapshot_term": 0}` |
| `commit <index>` | `{"commit_index": c}` |
| `snapshot <index>` | `{"ok": bool, "snapshot_index": i, "snapshot_term": t, "first_index": i, "log_len": n}` |
| `install <term> <last_included_index> <last_included_term>` | `{"ok": bool, "action": "stale"\|"retained"\|"discarded", "first_index": i, "last_index": i, "commit_index": c}` |
| `append <term> <prev_index> <prev_term> <entries-json>` | `{"success": bool, "reason": "ok"\|"compacted"\|"missing"\|"term mismatch", "last_index": i}` |
| `log` | `{"terms": [...], "first_index": i, "last_index": i, "commit_index": c, "snapshot_index": i, "snapshot_term": t}` |

You may never compact past `commit_index`. An `append` whose `prev_index` is below the
snapshot boundary is refused with `reason: "compacted"` — the follower cannot check it, and
the leader must send a snapshot instead. On `install`, `ok` means *applied*: a `"stale"`
install answers `ok: false`, while `"retained"` and `"discarded"` answer `ok: true` and both
raise `commit_index` to `max(commit_index, last_included_index)`.

#### `raft-membership` — joint consensus (stage 60)

| command | answer |
|---|---|
| `init <members-json>` | `{"phase": "stable", "members": [...], "quorum": n}` |
| `joint <new-members-json>` | `{"ok": bool, "phase": "joint", "old": [...], "new": [...], "quorum_old": n, "quorum_new": n}` |
| `commit-joint` | `{"ok": bool, "phase": "new", "members": [...], "quorum": n}` |
| `commit-new` | `{"ok": bool, "phase": "stable", "members": [...], "quorum": n}` |
| `agree <voters-json>` | `{"ok": bool, "reason": "..."}` — `"old and new"`, `"old only"`, `"new only"` or `"neither"` inside the joint phase; `"majority"` or `"no majority"` outside it |
| `add <member>` / `remove <member>` | `{"ok": bool, "phase": "pending"\|"stable", "members": [...], "quorum": n}` |
| `commit-change` | `{"ok": bool, "phase": "stable", "members": [...], "quorum": n}` |
| `state` | `{"phase": "...", "members": [...], "quorum": n}` |

In the joint phase a decision needs a majority of **both** configurations. One change at a
time: a second `joint`, `add` or `remove` while one is in flight is refused, and removing the
last member is refused outright. Inside the joint phase `state.members` is the **union** of the
two configurations and `state.quorum` is `max(quorum_old, quorum_new)` — there is no single
quorum there, which is why `joint` reports both separately.

#### `paxos` — single-decree Paxos (stage 61)

Acceptors are `a1`..`a<n>`; proposal numbers are integers; values are single words.

| command | answer |
|---|---|
| `init <n>` | `{"ok": true, "acceptors": n, "majority": m}` |
| `prepare <acceptor> <n>` | `{"promised": bool, "promised_n": n, "last_n": n\|null, "last_value": v\|null}` |
| `accept <acceptor> <n> <value>` | `{"accepted": bool, "promised_n": n}` |
| `acceptor <acceptor>` | `{"promised_n": n, "last_n": n\|null, "last_value": v\|null}` |
| `learn` | `{"chosen": bool, "value": v\|null, "count": k}` — `count` is how many acceptors hold the chosen value, and `0` when nothing is chosen |
| `propose <n> <value>` | `{"phase": "prepare", "n": n, "value": v}` |
| `promise <acceptor> <last_n> <last_value>` | `{"promises": k, "value": v, "ready": bool}` — `-1` and `-` mean "nothing accepted"; promises are keyed by acceptor, so the same one twice is still one promise |
| `proposer` | `{"n": n, "phase": "prepare"\|"accept", "promises": k, "value": v, "ready": bool}` |

An acceptor promises `n` only when `n > promised_n`, strictly. Once a majority of promises is
in, the proposer must use the value of the **highest-numbered** accepted proposal among them,
and only its own if no promise reported one.

#### `multi-paxos` — a stable leader (stage 62)

| command | answer |
|---|---|
| `init <n>` | `{"ok": true, "acceptors": n, "majority": m}` |
| `phase <slot>` | `{"phase": "prepare"\|"accept", "reason": "no leader"\|"stable leader"\|"preempted"}` |
| `prepare <n>` | `{"promised": k, "leader": bool, "n": n}` |
| `propose <slot> <value>` | `{"ok": bool, "phase": "...", "round_trips": k, "error": s\|null}` — refused with `round_trips: 2` when this proposer is not the leader |
| `accepted <slot> <acceptor>` | `{"chosen": bool, "value": v\|null, "count": k}` |
| `chosen <slot>` | `{"chosen": bool, "value": v\|null}` |
| `preempt <n>` | `{"leader": bool, "promised_n": n}` |
| `applied` | `{"index": i, "gaps": [...]}` |
| `state` | `{"n": n, "leader": bool, "slots": {...}, "applied": i}` |

One prepare covers every future slot, so an established leader commits in **one** round trip
rather than two. That is precisely the property Raft builds in as leadership. A `prepare` that
wins no majority leaves any existing leadership alone; only a `preempt` at a strictly higher
ballot strips it.

#### `two-phase-commit` — the blocking window (stage 63)

| command | answer |
|---|---|
| `init <participants-json>` | `{"state": "init", "participants": [...]}` |
| `prepare` | `{"state": "preparing", "sent": n}` |
| `vote <participant> <yes\|no>` | `{"state": "...", "votes": k, "decision": "commit"\|"abort"\|null}` |
| `decide` | `{"decision": ..., "sent": n}` |
| `deliver <participant>` | `{"state": "committed"\|"aborted"}` |
| `p-state <participant>` | `{"state": "working"\|"prepared"\|"committed"\|"aborted"}` |
| `p-timeout <participant>` | `{"decision": "abort"\|null, "blocked": bool, "reason": "..."}` |
| `p-consult <participant>` | `{"decision": ..., "blocked": bool, "reason": "..."}` |
| `crash coordinator` | `{"ok": true, "coordinator": "crashed"}` |
| `state` | `{"state": "...", "decision": ..., "votes": {...}, "coordinator": "up"\|"crashed"}` |

A participant that voted yes is `prepared` and has given up its right to abort. With the
coordinator down, cooperative termination helps only when some peer knows the decision or
some peer never voted; when every peer is prepared, everyone is stuck. That is the blocking
window, and no amount of asking around closes it.

A vote is a promise, so a repeated `vote` from the same participant is ignored and the first
one stands. `p-timeout` on a participant that has not yet voted records its vote as `no`, which
is what makes "the coordinator can never commit now" observable rather than merely eventual.
On a participant that has already reached a terminal state, both `p-timeout` and `p-consult`
answer with its own decision and `reason: "already decided"`. Every answer carries every key
the grammar names, with `null` where it does not apply, so no test has to branch on whether a
key is present.

#### `three-phase-commit` — what the extra round buys (stage 64)

| command | answer |
|---|---|
| `init <participants-json>` | `{"state": "init", "participants": [...]}` |
| `vote <participant> <yes\|no>` | `{"state": "...", "votes": k, "decision": ...}` |
| `pre-commit` | `{"ok": bool, "state": "pre-committed", "sent": n}` |
| `p-precommit <participant>` | `{"state": "pre-committed"}` |
| `ack <participant>` | `{"acks": k}` |
| `commit` | `{"ok": bool, "decision": "commit"}` |
| `deliver <participant>` | `{"state": "committed"\|"aborted"}` |
| `p-state <participant>` | `{"state": "working"\|"prepared"\|"pre-committed"\|"committed"\|"aborted"}` |
| `p-timeout <participant>` | `{"decision": "commit"\|"abort", "blocked": bool, "reason": "..."}` |
| `crash coordinator` | `{"ok": true}` |
| `partition <group-json>` | `{"split": true, "groups": [[...], [...]]}` |
| `heal` | `{"split": false}` |
| `decisions` | `{"decisions": {...}, "inconsistent": bool}` |

The coordinator's states are `init`, `voting`, `pre-committed`, `committed` and `aborted`;
`p-state` and `decisions` are what a test reads, and there is deliberately no top-level `state`
command here.

The pre-commit round turns the blocking window into a termination rule: a pre-committed
participant times out to commit, a merely prepared one to abort, and neither is ever blocked.
It costs a round trip on every commit, and it still does not survive a partition — which is
what stage 64's last test demonstrates, and why real systems reach for consensus instead.

Be precise about what that buys. The termination rule this topic implements is the simple one
— pre-committed commits, prepared aborts, with no consultation between peers — so a *partially
delivered* pre-commit diverges on its own, partition or no partition. 3PC removes the
*blocking*, not the disagreement; only consensus removes the disagreement.

#### `commit-recovery` — learning the outcome from the log (stage 65)

| command | answer |
|---|---|
| `init <participant>` | `{"ok": true, "log": []}` |
| `begin <tx>` | `{"state": "working"}` |
| `prepare <tx> <yes\|no>` | `{"vote": "...", "logged": "prepared"\|"abort", "state": "..."}` |
| `vote-sent <tx>` | `{"ok": true}` |
| `decide <tx> <commit\|abort>` | `{"state": "...", "logged": "commit"\|"abort"}` |
| `apply <tx>` | `{"applied": true}` |
| `crash` | `{"ok": true, "lost": "volatile state"}` |
| `recover` | `{"pending": [...], "decided": [...], "actions": {...}}` |
| `outcome <tx>` | `{"state": "...", "applied": bool}` |
| `coordinator-says <tx> <commit\|abort>` | `{"state": "...", "applied": bool}` |
| `log` | `{"records": [...]}` |

`actions` values are `"ask-coordinator"`, `"redo"`, `"abort"` and `"none"`. The prepare record
is **forced to the log before the vote is sent**; a transaction with no prepare record is
presumed aborted; one that is prepared asks the coordinator rather than guessing. Between a
`crash` and a `recover` every `outcome` is `"unknown"` — that is what makes "volatile state
does not survive and the log does" observable. `coordinator-says <tx> commit` also applies the
effect, as a recovering participant really does, and recovery writes no abort record for a
presumed-abort transaction, so a crash loop cannot grow the log.

#### `saga` — an orchestrated saga (stage 66)

| command | answer |
|---|---|
| `init <steps-json>` | `{"steps": n, "names": [...]}` |
| `step <name> <ok\|fail>` | `{"phase": "forward"\|"compensating"\|"done", "completed": [...], "outcome": ...}` |
| `compensate <name> <ok\|fail>` | `{"phase": "...", "pending": [...], "outcome": ...}` |
| `run <fail_at>` | `{"executed": [...], "compensated": [...], "outcome": "committed"\|"compensated"}` |
| `ledger` | `{"entries": ["do:reserve", "undo:reserve", ...]}` |
| `state` | `{"phase": "...", "completed": [...], "pending": [...], "outcome": ...}` |

A failure at step *k* compensates steps *k-1 … 1*, in that order. The failing step is **not**
compensated — it never completed. Compensations are idempotent, and one that fails is retried
rather than skipped.

#### `saga-choreo` — the same guarantee, driven by events (stage 67)

| command | answer |
|---|---|
| `init <steps-json>` | `{"steps": n, "names": [...]}` |
| `event <name>` | `{"emitted": [...], "duplicate": bool, "ledger": [...]}` |
| `ledger` | `{"entries": [...]}` |
| `outcome` | `{"outcome": "running"\|"committed"\|"compensated", "inflight": k}` |
| `state` | `{"handled": [...], "emitted": [...], "outcome": "..."}` |

Event names are `start`, `<step>.ok`, `<step>.fail` and `<step>.undone`. A duplicate event
emits nothing the second time, and two failure events must not start two compensation chains —
that race is the pattern's characteristic bug. An event that does not apply in the current
state is dropped and **not** recorded as handled, so the same event arriving later, when it
does apply, is not swallowed as a duplicate.

The ledger records `do:<step>` when the step's `.do` is *emitted*, so a failed step still
leaves a `do:` entry here, where stage 66's orchestrated ledger leaves none. The happy-path
ledgers of the two stages are identical — a test asserts it — and so are the `undo:`
sequences; that one entry is the whole difference, and it is an artefact of who writes the
ledger, not of the guarantee.

#### `outbox` — a write and its event, atomically (stage 68)

| command | answer |
|---|---|
| `init` | `{"rows": 0, "outbox": 0, "published": 0}` |
| `write <key> <value> <event>` | `{"ok": true, "rows": n, "outbox": k}` |
| `naive-write <key> <value> <event>` | `{"ok": true, "rows": n, "published": k}` |
| `crash <before-commit\|after-row\|now>` | `{"ok": true, "lost": "..."}` |
| `recover` | `{"rows": n, "outbox": k, "published": k, "lost": k}` |
| `publish` | `{"published": [...], "outbox": k, "unacked": k}` |
| `ack <event>` | `{"outbox": k, "unacked": k}` |
| `delivered <event>` | `{"count": k, "distinct": k}` |
| `state` | `{"rows": {...}, "outbox": [...], "published": [...], "lost": [...]}` |

`crash before-commit` rewinds the most recent write entirely — no row, no outbox record, no
event — which is how the "loses both, never one" half of atomicity is observed; `after-row`
shows the other half. While the process is down, `write`, `naive-write`, `publish` and `ack`
are refused until `recover`; `state` and `delivered` still answer, being observations rather
than actions.

The outbox buys "never lost"; the consumer's dedup buys "never applied twice". Neither alone
is exactly-once, and a crash between publish and ack really does send the event twice.

#### `dedup` — an idempotent consumer (stage 69)

| command | answer |
|---|---|
| `init <window>` | `{"window": n, "seen": 0, "total": 0}` — `0` means unbounded |
| `deliver <id> <amount>` | `{"applied": bool, "duplicate": bool, "total": n, "seen": k}` |
| `forget <id>` | `{"ok": bool, "seen": k}` |
| `guarantee` | `{"exactly_once": bool, "reason": "..."}` |
| `state` | `{"total": n, "seen": k, "applied": k, "ids": [...]}` |

The dedup table is a FIFO of first sightings, evicted oldest-first. Once an id has been
forgotten the guarantee is gone, which is the difference between *effectively*-once and
*exactly*-once.

#### `idempotency-key` — a retry that does not double-charge (stage 70)

| command | answer |
|---|---|
| `init` | `{"balance": 0, "charges": 0}` |
| `charge <key> <amount>` | `{"ok": bool, "charge_id": "c1", "amount": n, "replayed": bool, "balance": n}` |
| `naive-charge <amount>` | `{"charge_id": "c1", "balance": n}` |
| `begin <key> <amount>` | `{"ok": bool, "state": "in-progress"\|"done"}` |
| `finish <key>` | `{"ok": bool, "charge_id": "c1", "balance": n}` |
| `lost-answer <key>` | `{"ok": true}` |
| `state` | `{"balance": n, "keys": {...}}` |

A key is bound to its request: the same key with a different amount is an error, not a replay.
A key that is still in progress refuses a second request rather than starting a second charge.

#### `fencing` — a lock that survives a paused holder (stage 71)

| command | answer |
|---|---|
| `init` | `{"holder": null, "token": 0, "fence": 0, "value": null}` |
| `acquire <client>` | `{"granted": bool, "token": n, "holder": "c1"}` |
| `release <client>` | `{"ok": bool, "holder": null}` |
| `expire` | `{"expired": "c1", "holder": null}` |
| `write <client> <token> <value>` | `{"ok": bool, "reason": "accepted"\|"stale token"\|"no token", "value": v, "fence": n}` |
| `write-unfenced <client> <value>` | `{"ok": true, "value": v}` |
| `state` | `{"holder": ..., "token": n, "fence": n, "value": v, "writes": [...]}` |

Tokens strictly increase across every grant, and the resource rejects any write below its
fence. Checking `holder` before writing closes nothing, because the pause happens between the
check and the write — which is exactly Kleppmann's scenario.

#### `leases` — a leader lease under clock skew (stage 72)

| command | answer |
|---|---|
| `init <lease_ms> <clock_error_ms>` | `{"lease_ms": n, "clock_error_ms": n}` |
| `grant <node> <now>` | `{"ok": bool, "holder": ..., "expires_at": t, "safe_until": t}` |
| `renew <node> <now>` | `{"ok": bool, "expires_at": t, "safe_until": t}` |
| `read <node> <now>` | `{"served": bool, "reason": "..."}` |
| `skew <node> <offset_ms>` | `{"ok": true, "offset_ms": n}` |
| `holder <now>` | `{"holder": ..., "expires_at": t}` |
| `state` | `{"holder": ..., "granted_at": t, "expires_at": t, "safe_until": t, "offsets": {...}}` |

`safe_until` is `expires_at - clock_error_ms`: a leader stops serving local reads one
clock-error before its lease ends, and the granter waits one clock-error past the old expiry
before handing the lease on. Those two margins are what stop two leaders from believing in
themselves at once.

#### `anti-entropy` — read repair, hints and sync (stage 73)

| command | answer |
|---|---|
| `init <replicas-json>` | `{"replicas": [...]}` |
| `put <replica> <key> <value> <version>` | `{"ok": true, "version": n}` |
| `down <replica>` / `up <replica>` | `{"up": [...], "down": [...]}` |
| `write <key> <value> <version>` | `{"stored": [...], "hinted": [{"for": r, "on": r}, ...]}` |
| `read <key>` | `{"value": v, "version": n, "stale": [...], "repaired": [...]}` |
| `hints <replica>` | `{"keys": [...]}` |
| `handoff` | `{"delivered": n, "remaining": n}` |
| `sync <a> <b>` | `{"transferred": n, "keys": [...]}` |
| `state <replica>` | `{"keys": {...}}` |

A read repairs exactly the replicas that were behind. The stand-in for a hint is always the
first live replica in `init` order, and `handoff` delivers a hint only when the owner **and**
its stand-in are up — which is what makes "a hint dies with its stand-in" observable.
Repairs, writes and handoffs are version-guarded; `put` is an unguarded fixture, so a test can
deliberately push a replica backwards. Hinted handoff is an optimisation; anti-entropy is the
guarantee, and stage 73 builds the case where only `sync` can finish the job.

#### `gossip` — rounds to convergence (stage 74)

The peer choice is **pinned**, so a gossip run is reproducible:

> Nodes are numbered `0 .. n-1`; node 0 starts infected. In round `r` (1-based), every already
> infected node `i` contacts the nodes `(i + (fanout+1)^(r-1) * k) mod n` for `k = 1 ..= fanout`.
> Every node contacted becomes infected at the end of the round, and every contact counts as one
> message whether or not the target was already infected.

| command | answer |
|---|---|
| `init <n> <fanout>` | `{"nodes": n, "fanout": f, "infected": 1, "round": 0}` |
| `round` | `{"round": r, "infected": n, "new": k, "messages": m}` |
| `converge` | `{"rounds": r, "messages": m, "infected": n}` |
| `infected` | `{"nodes": [...], "count": k}` |
| `state` | `{"nodes": n, "fanout": f, "round": r, "infected": [...], "messages": m}` |

The step is `(fanout+1)^(r-1)` rather than a plain doubling precisely so the round count
answers to the fanout: at `n = 32` the schedule converges in 5, 4, 3 and 3 rounds for fanouts
1 to 4, sending 31, 80, 63 and 124 messages. Convergence takes about `log_(fanout+1)(n)`
rounds, and the message count rises faster than the round count falls — which is the whole
reason the fanout is a knob and not a constant.

#### `circuit-breaker` — closed, open, half-open (stage 75)

| command | answer |
|---|---|
| `init <fail_threshold> <open_ms> <success_threshold>` | `{"state": "closed", "failures": 0, "successes": 0}` |
| `call <now> <ok\|fail>` | `{"allowed": bool, "state": "closed"\|"open"\|"half-open", "failures": k, "successes": k, "reason": "attempted"\|"rejected while open"\|"trial"}` |
| `state <now>` | `{"state": "...", "failures": k, "successes": k, "opened_at": t, "retry_at": t}` |

A rejection while open is not a failure — the breaker exists to stop hammering a service that
is down. A failure during a trial reopens the circuit and restarts the timer.

#### `hedging` — hedged requests (stage 76)

| command | answer |
|---|---|
| `init <hedge_after_ms>` | `{"hedge_after_ms": n, "requests": 0, "messages": 0}` |
| `request <id> <latencies-json>` | `{"latency": n, "messages": k, "winner": "primary"\|"hedge", "inflight": 0}` |
| `plain <id> <latencies-json>` | `{"latency": n, "messages": 1}` |
| `stats` | `{"requests": n, "messages": k, "extra_load_pct": p, "p50": n, "p99": n, "max": n}` |
| `state` | `{"hedge_after_ms": n, "latencies": [...], "messages": k}` |

`latencies-json` is `[primary_ms, hedge_ms]`. A hedged request answers in
`min(primary, hedge_after_ms + hedge)` and the loser is cancelled, so `inflight` is always 0
afterwards. `request` and `plain` report the `messages` **that request** cost, 1 or 2, while
`stats.messages` is the running total. The hedge fires when the primary is not strictly faster
than the threshold, so a primary at exactly `hedge_after_ms` is hedged. The tail falls and the
load rises; stage 76 asserts both halves at once.

#### `bulkhead` — pools and load shedding (stage 77)

| command | answer |
|---|---|
| `init <pools-json>` | `{"pools": {"a": {"limit": n}, ...}}` |
| `init-shared <limit>` | `{"pools": {"shared": {"limit": n}}}` |
| `call <pool> <id> <now>` | `{"admitted": bool, "pool": p, "inflight": k, "rejected": n, "reason": "admitted"\|"pool is full"\|"no such pool"}` |
| `done <pool> <id>` | `{"ok": bool, "inflight": k}` |
| `shed <now> <deadline_ms>` | `{"shed": [...], "inflight": {...}}` |
| `stats` | `{"pools": {...}, "total_rejected": n}` |

A full pool rejects immediately rather than queueing, and saturating one pool leaves the
others untouched — which a single shared pool conspicuously does not. `shed` sweeps every pool
at once, and its `inflight` is an object of pool name to count.

### 3.3 How the oracles work

Every oracle on this ladder is **the tester's own model of the specified rules**, written out
in the stage file next to the tests that use it. Nothing here is a second implementation the
learner could copy: the model encodes invariants, not behaviour.

* **State transitions, asserted one at a time** — the election rules of §5.1 and §5.2, the
  AppendEntries consistency check, the acceptor's "promise only a strictly higher number", the
  breaker's three states. Each transition gets its own test, so a failure names the rule.
* **Traps, each with a test of its own** — Raft's Figure 8, the 2PC blocking window, saga
  compensation order and the failing step that must not be compensated, the fencing-token
  scenario, Paxos livelock, the racing compensations of a choreographed saga, the forgotten
  dedup entry. These are the point of the ladder, not a bonus round.
* **Properties over many orders** — "no two participants decide differently", "at most one
  value is ever chosen", "the pool's inflight never exceeds its limit". Asserted over an
  exhaustive sweep where the space is small, and over a seeded replay against the tester's
  model where it is not. The seed is printed with the failure, so a red test is reproducible.

`reference_algorithms` exists only so this ladder can be self-checked, exactly as
`reference_primitives` does for ladder A. **Reading `examples/reference_algorithms.rs` or
anything under `examples/alg/` spoils ladder B.**

---

## 4. Ladder C — one node

### 4.1 The program contract

```
./your_program.sh --name <n> --data-dir <dir> \
  --listen-client-urls http://127.0.0.1:<port> --advertise-client-urls http://127.0.0.1:<port> \
  --listen-peer-urls http://127.0.0.1:<pport> --initial-advertise-peer-urls http://127.0.0.1:<pport> \
  --initial-cluster <name=peerurl,...>
```

Flags always arrive in that order, each as two argv entries. The harness allocates every
port; nothing is ever fixed. Stage 48 adds one further flag, `--initial-cluster-state
existing`, for a member joining a cluster that already exists.

Your server must answer HTTP/1.1 on the client URL, keep connections alive, and speak the
following subset. Everything is `POST` with a JSON body unless stated, and every key and
value on the wire is **base64**.

| endpoint | what it does |
|---|---|
| `POST /v3/kv/put` | write one key |
| `POST /v3/kv/range` | read a key, a range or the whole store |
| `POST /v3/kv/deleterange` | delete a key or a range |
| `POST /v3/kv/txn` | compare, then run one of two branches |
| `POST /v3/kv/compaction` | drop history below a revision |
| `POST /v3/lease/grant` | create a lease |
| `POST /v3/lease/revoke` | destroy one |
| `POST /v3/lease/keepalive` | a stream; renew one |
| `POST /v3/watch` | a stream; events from a revision |
| `POST /v3/maintenance/status` | revision, term, leader, sizes |
| `POST /v3/cluster/member/{list,add,remove}` | membership (ladder D) |
| `GET /version`, `GET /health` | what they say |

### 4.2 Two properties of this wire format

Both are protobuf's JSON mapping, and both are load-bearing:

* **64-bit integers are strings.** `"revision":"7"`, not `7`. The harness accepts a plain
  number too, because that is what a hand-written server usually emits first, but the
  reference always writes strings.
* **A field at its zero value may be omitted entirely.** `{"count":"0"}` and no `count` at
  all mean the same thing, and real etcd omits it. No test in this suite ever asserts that a
  field is *present*; everything decodes a missing field as its zero.

### 4.3 Worked exchanges

A put, and the header it answers with:

```
POST /v3/kv/put
{"key":"Zm9v","value":"YmFy"}

HTTP 200 OK
{"header":{"cluster_id":"12101053776118486841","member_id":"15985099378392235387",
           "revision":"2","raft_term":"2"}}
```

Reading it back:

```
POST /v3/kv/range
{"key":"Zm9v"}

{"header":{...,"revision":"2","raft_term":"2"},
 "kvs":[{"key":"Zm9v","create_revision":"2","mod_revision":"2","version":"1","value":"YmFy"}],
 "count":"1"}
```

A prefix range is `[key, key-with-last-byte-incremented)`; `key` and `range_end` both `"AA=="`
(a single zero byte) means every key in the store.

A compare-and-swap, as a transaction:

```
POST /v3/kv/txn
{"compare":[{"key":"Zm9v","target":"VALUE","result":"EQUAL","value":"YmFy"}],
 "success":[{"requestPut":{"key":"Zm9v","value":"YmF6"}}],
 "failure":[{"requestRange":{"key":"Zm9v"}}]}

{"header":{...,"revision":"3"},"succeeded":true,
 "responses":[{"response_put":{"header":{"revision":"3"}}}]}
```

`VERSION` compared against `"0"` is how you say "this key does not exist".

An error:

```
POST /v3/kv/range
{"key":"Zm9v","revision":2}          # revision 2 was compacted away

HTTP 400
{"code":11,"message":"etcdserver: mvcc: required revision has been compacted"}
```

The codes the suite names: **3** invalid argument (bad base64, a malformed body), **5** not
found (a lease that is gone), **8** resource exhausted (too large), **11** out of range, **14**
unavailable (no quorum, no leader). Code 11 covers both ends of the revision range: a read
below what has been compacted away, and a read at a revision the store has not reached yet.

A handful of other things real etcd does, which the suite therefore accepts:

* `GET /health` answers `{"health":"true","reason":""}`, so a test reads the `health` field
  rather than comparing the whole object.
* A `GET` on a POST-only endpoint answers **501**, so the suite asks only that it is not a
  success — 404 and 405 are equally defensible.
* An over-large request comes back as HTTP **429** with code 8, not a 4xx in the 400s.
* A lease TTL below two seconds is rounded up, so the lease stages ask for two and wait with
  a poll rather than a sleep. Expiry is allowed to be late; it is never allowed to be early.
* A `VALUE` comparison against a key that does not exist always fails, because etcd refuses
  to equate a missing value with an empty one. The suite makes its "compares against zeros"
  point with `VERSION`, `CREATE` and `MOD` instead.
* A range whose `range_end` sorts before its `key` is an empty 200, not an error.

A stream — `/v3/watch` and `/v3/lease/keepalive` — answers `Transfer-Encoding: chunked` with
**one JSON object per line, each wrapped in `{"result": ...}`**:

```
POST /v3/watch
{"create_request":{"key":"a3c="}}

{"result":{"header":{...},"created":true}}
{"result":{"header":{...,"revision":"9"},"events":[{"kv":{"key":"a3c=","value":"djE=",
                                                          "create_revision":"9",
                                                          "mod_revision":"9","version":"1"}}]}}
```

An event with no `type` is a put, because `PUT` is the zero value of that enum. Several
request messages may be sent in one request body, one per line; that is how the suite creates
a watch and then cancels it on the same stream.

### 4.4 A note on `/tmp`

Each node preallocates a 64 MB write-ahead log the moment it starts, and a full run starts
several hundred of them. The harness deletes every test's scratch directory as soon as the
node or cluster that owned it is gone, and sweeps the leftovers of runs whose process no
longer exists, so the steady state is a few hundred megabytes. If your `/tmp` is a small
tmpfs and you are running several testers at once, point `TMPDIR` at real disk.

### 4.5 Durability

Stage 35 is the one that hurts. The harness runs a workload, `SIGKILL`s the process in the
middle of it, starts it again from the same `--data-dir`, and demands that **every
acknowledged write is still there**. Nothing is flushed for you: if you answered before the
write reached the disk, you will fail, and `examples/broken_node.rs` exists to show you
exactly what that failure looks like.

---

## 5. Ladder D — a cluster

### 5.1 How a cluster is started

Three (or five) copies of your program, each with its own `--name`, `--data-dir` and ports,
and all with the same `--initial-cluster`. Members are called `m1`, `m2`, ... and are
numbered from zero inside the harness.

The peer URL each member **advertises** is not the port it listens on. Every member's peer
URL points at a **userspace TCP proxy** the harness runs in front of it:

```
m1 ──advertised m1=http://127.0.0.1:PROXY1──▶ [proxy 1] ──▶ 127.0.0.1:PEER1 (m1 really listens here)
```

Client traffic goes straight to the member; peer traffic always goes through a proxy. That is
what lets the suite partition a cluster with no privileges, no `iptables` and no network
namespaces.

### 5.2 The fault-injection model

`src/cluster/proxy.rs`. Faults are expressed per **link** — an unordered pair of members —
because both directions of a pair share the same TCP connections.

| fault | what the proxy does |
|---|---|
| `cut` | refuses new connections and closes open ones, in both directions |
| `delay` | holds every chunk for the given time before passing it on |
| `drop` | refuses a new connection with the given probability, so the link flaps |
| `duplicate` | in message mode, sends a parsed peer message twice |
| `reorder` | in message mode, holds a message back and sends it after the next one |

**Which member dialled?** A proxy only ever sees the destination, so the harness asks the
kernel: the accepted connection's source port names a socket in `/proc/net/tcp`, that
socket's inode appears in exactly one process's `/proc/<pid>/fd`, and the harness started
every one of those processes. The answer is cached per source port. Where the lookup cannot
work the connection is treated as "some peer" and only destination-wide faults apply, which
is counted and reported rather than hidden.

**When framing is decided.** A connection is either framed or not from the moment it is
accepted, because starting to frame a stream halfway through would split it in the wrong
place. A stage that wants duplication or reordering on a cluster that is already talking
therefore cuts the links first, so the peers redial and the new connections come up in
message mode.

**Why duplication and reordering need framing.** Deleting or swapping *bytes* of a TCP stream
corrupts it; it does not model a network. In message mode the proxy parses the dialer's
direction as HTTP/1.1 requests — which is what a raft transport over HTTP sends — and
duplicates or swaps whole messages. A stream it cannot frame (a chunked one, say) is passed
through untouched and counted, and the stage reports how many messages were actually mangled
rather than pretending.

Everything is driven by `--seed`: which fault fires, when, and on which members.

**What the injector does not promise.** The identification above is a heuristic about the
kernel's view of a socket, not a guarantee. Cached answers are thrown away whenever the fault
table changes, and the cache is keyed on both ports so a recycled ephemeral port cannot be
mistaken for the connection that used to own it — but a cut that is held for tens of seconds
is stressing the mechanism harder than it was built for. The cluster stages therefore keep
their isolation windows to a few seconds, which is also what makes them quick, and assert on
what the cluster *answered* rather than on how many packets the proxy stopped.

**Three things about the reference that shaped the cluster stages.**

* A member with no quorum answers a write with code 14 or code 4 (deadline exceeded), after
  about seven seconds — etcd's own commit timeout. Both codes mean the same thing to a
  client, and the suite accepts either.
* A linearizable read taken in the instant a new leader takes over answers
  `etcdserver: leader changed`. That says nothing about the data, so a read-back after a
  failover retries rather than calling it a lost write.
* A configuration change is refused with `etcdserver: unhealthy cluster` unless the leader
  has been in continuous contact with every voting member for the last five seconds, which a
  freshly started cluster has not been. The membership stages therefore re-offer the change
  until it is accepted, which is most of what makes them the slowest two stages in the run.

### 5.3 The linearizability checker

`src/lin/`. Stage 54 runs a randomized concurrent workload — several client tasks, a handful
of keys, requests spread across members, puts and reads and compare-and-swaps and deletes —
while the seeded fault schedule cuts and heals the network underneath it. Every call is
recorded with the instant it was made and the instant the answer came back, and the resulting
history is checked.

A history is linearizable when every operation can be placed at a single instant inside its
own call/return window, in some total order, such that a sequential key/value store would
have produced exactly the answers the clients saw.

The search is Wing and Gong's, in the shape Lowe and Porcupine describe, with two things
keeping it tractable:

* **the candidate rule** — an operation may only go next when no other remaining operation
  had already returned before it was called;
* **memoisation** — a branch is identified by (the set of operations already linearized, the
  model state), and the same pair can never end differently twice.

Keys are checked independently, because no operation in the workload spans two keys.

**An operation whose answer never arrived is optional.** A timeout, a killed node or a
partitioned client leaves the harness genuinely unable to say whether the write took effect,
so such an operation may be linearized anywhere after it was called — or left out entirely.
That is the only sound reading of a lost response, and it is what makes fault injection
testable rather than a source of false failures.

On failure the report shows the **smallest offending sub-history**: the few operations that
already admit no linearization, each with its concurrency window, the one that made it
impossible marked `>>`, the furthest the search ever got, and the state that left the
remaining operations with nowhere to go.

```
key "t054/k1": no linearization exists for these 4 operations

  id    client  call..return (ms)     key    operation              result
  #7    c1      12.004..  12.310 ms  k1     write a                ok
  #9    c2      12.415..  12.902 ms  k1     read                   = a
>>#12   c0      13.001..  13.204 ms  k1     read                   = <absent>

the operation marked >> cannot be placed anywhere inside its own window
the furthest the search ever got was 2 of 3 operations: #7 → #9
leaving the key holding a, which none of the remaining operations accept
(184 linearization states explored)
```

The checker has its own unit tests over known-linearizable and known-non-linearizable
histories — stale reads, vanished acknowledged writes, two compare-and-swaps that both claim
to have won, lost responses that later turn out to have happened — because this is the part
of the suite most likely to be subtly wrong.

---

## 6. Targets and `--validate`

`targets.yaml`:

```yaml
targets:
  etcd:                 # reference for the node and cluster ladders
    kind: reference
    version: "3.7.1"
  reference_primitives: # reference for the primitives ladder
    kind: example
  reference_algorithms: # reference for the algorithms ladder
    kind: example
  broken_node:          # a deliberately wrong node
    kind: example
  my_node:
    command: ["./your_program.sh"]
    cwd: "."
```

* `kind: reference` — real etcd, downloaded once into `~/.cache/disttest` (override with
  `DISTTEST_CACHE`) behind an `flock`ed lock file, with its SHA-256 checked against a pinned
  digest before it is unpacked. Nothing is downloaded when it is already there, so a
  `--validate` run does no network I/O.
* `kind: example` — one of this crate's own `examples/*.rs` binaries, built on first use.
* anything else — your program. The ladder's own arguments are appended to `command`.
  `{NAME}`, `{DATADIR}`, `{TMP}`, `{PORT}` and `{PEERPORT}` are substituted if you use them.

### How `--validate` picks the reference

**`--validate` runs every stage against the reference that stage's own ladder names,
whatever `--target` said.** `primitives` is routed to `reference_primitives`, `algorithms` to
`reference_algorithms`, and `node` and `cluster` to `etcd`. That is why one command can prove
the whole suite:

```
disttest --target etcd --validate --all
  primitives  validated against reference_primitives
  algorithms  validated against reference_algorithms
  node        validated against etcd
  cluster     validated against etcd
```

`--target` still has to name a registered reference — passing your own program to
`--validate` is a usage error, because a self-check against an unproven implementation proves
nothing. `--validate` never changes which stages are selected: `--validate --stage 5` runs
stage 5, and `--validate` on its own means `--all`. A failure under `--validate` is worded as
a **suite bug**, because the reference is by definition right.

`reference_primitives` and `reference_algorithms` exist only so the two CLI ladders can be
self-checked, exactly as `broken_node` exists only so the red output can be demonstrated.
**Reading `examples/reference_primitives.rs` spoils ladder A, and reading
`examples/reference_algorithms.rs` or anything under `examples/alg/` spoils ladder B.**

### Watching it go red

```
disttest --target broken_node --stage 35
```

`broken_node` speaks enough of the API to get through the early node stages — puts, ranges,
deletes, transactions, status — and is then wrong in one specific, famous way: it keeps the
store in memory and flushes it on a lazy five-second timer, so it acknowledges a write before
the write is durable. Stage 35 kills it mid-workload and the acknowledged write is gone:

```
Stage 35 Durability across SIGKILL [node]
  ✘ an acknowledged write survives a kill FAIL (72 ms)
      range.count: expected 1, got 0
      range.kvs[0].value: expected "still here", got ""
      range.header.revision: expected at least 2, got 1
  ✘ a hundred acknowledged writes all survive FAIL (78 ms)
      acknowledged writes that are missing: expected 0, got 100
    ┌ the keys that were lost
    │ t002/hundred/0000
    │ ... and 90 more
  …
Stage 35  Durability across SIGKILL                         0/9 passed
```

It is not a cluster either — it ignores `--initial-cluster` and always claims to be its own
leader — so the cluster ladder fails against it too, loudly.

---

## 7. Flags

| flag | meaning |
|---|---|
| `--target <name\|path>` | the program to test: a name from `targets.yaml`, or a path |
| `--stage N` | run one stage |
| `--until N` | run stages `1..=N` (or `--from..=N`) |
| `--from N` | run stages `N..` (or `N..=--until`) |
| `--all` | run every stage |
| `--only SUBSTR` | only tests whose name contains this |
| `--tag T` | only tests carrying this tag: a ladder (`primitives`, `algorithms`, `node`, `cluster`), `ext`, or any tag a stage added |
| `--skip-ext` | hide everything past a sensible core |
| `--verbose` | say what the harness is doing between tests |
| `--keep-tmp` | keep every test's temporary directory and print the paths |
| `--timeout-ms MS` | default per-test timeout (starting a node is not counted) |
| `--validate` | run against each ladder's reference; word failures as suite bugs |
| `--list` | stages, ladders, test counts, `PLAN.md` tickboxes |
| `--list --json [FILE]` | the stage catalog; `-` or no value means stdout |
| `--json FILE` | write the machine-readable run report |
| `--no-color` | never colour the output (`NO_COLOR` does the same) |
| `--targets-file FILE` | where `targets.yaml` lives |
| `--seed N` | seed for every random choice |
| `--capture-examples FILE` | run every stage's examples against the references and record them |

Naming a ladder with `--tag` selects whole stages, so the run header stays honest about what
was left out.

---

## 8. The JSON schemas

### 8.1 `--json report.json`

Field for field the schema `shelltest` and `kafkatest` write, so `byo` and the site ingest it
with no branch of their own. The ladder rides along in each test's `tags`; nothing was added
at the top level.

```json
{
  "target": "etcd",
  "validate": true,
  "stages": [
    {
      "stage": 22, "name": "Put and the response header",
      "file": "src/stages/s22_put_header.rs",
      "passed": 8, "failed": 0, "skipped": 0,
      "tests": [
        {
          "name": "a put answers with a header",
          "status": "pass",
          "ext": false,
          "tags": ["node"],
          "duration_ms": 14,
          "failures": [],
          "skip_reason": null,
          "actual": [],
          "failure_kind": null,
          "notes": []
        }
      ]
    }
  ],
  "passed": 720, "failed": 0, "skipped": 0, "elapsed_ms": 1041236
}
```

`status` is `pass`, `fail` or `skip`. `failure_kind` is one of `assertion`, `timeout`,
`connection`, `protocol`, `node_crash`, `linearizability`, `harness` — a crashed node never
looks like a wrong value.

### 8.2 `catalog.json`

```json
{
  "track": "dist",
  "generatedAt": "2026-09-18T18:22:04Z",
  "sections": [{ "id": "a", "title": "Primitives: clocks and causality", "stages": [1,2,3,4] }],
  "stages": [
    {
      "number": 22, "slug": "put_header", "name": "Put and the response header",
      "ext": false, "ladder": "node", "file": "src/stages/s22_put_header.rs",
      "hints": ["...", "..."],
      "tests": [{ "name": "...", "tags": ["node"] }],
      "examples": [
        {
          "title": "A put and the header it answers with",
          "kind": "node",
          "request": "…the words…\n\nPOST /v3/kv/put\n{ … }",
          "request_hex": "7b226b6579…",
          "response": "…the words…\n\nHTTP 200 OK\n{ … }",
          "response_hex": "7b22686561…",
          "note": "…the trap…",
          "request_fields": [{ "offset": 1, "length": 5, "field": "request.key", "value": "\"Zm9v\"" }],
          "response_fields": [{ "offset": 1, "length": 8, "field": "response.header", "value": "…", "varies": true }]
        }
      ]
    }
  ]
}
```

`ladder` is `primitives`, `algorithms`, `node` or `cluster`, and it is also the first entry of
every test's `tags`. `kind` is `node`, `cluster`, `primitives` or `workload`; a ladder B stage
carries `primitives` examples, because the exchange it records has exactly that shape — a CLI
transcript — and the site renders it with the same component. `*_hex` holds the exact bytes that
went over the wire or the pipe, so an example can never quietly drift from what the suite
actually sends; the readable form is in `request` and `response`. Fields marked `varies` are
the ones that legitimately differ every run — ids, revisions, terms.

`catalog.json` is committed, and `tests/catalog_is_current.rs` fails when it drifts:

```
disttest --capture-examples examples/captured.json --target etcd   # only when bytes moved
disttest --list --json catalog.json
```

---

## 9. Layout

```
disttest/
  Cargo.toml
  targets.yaml          the programs disttest can point at
  PLAN.md               the tickbox stage plan; --list reads its boxes
  catalog.json          --list --json output, committed
  examples/
    reference_primitives.rs   ladder A's reference (reading it spoils ladder A)
    prim/*.rs                 one file per group of ladder A topics
    reference_algorithms.rs   ladder B's reference (reading it spoils ladder B)
    alg/*.rs                  one file per group of ladder B topics
    broken_node.rs            a node that acknowledges writes before they are durable
    captured.json             what --capture-examples recorded
  src/
    main.rs             the CLI
    config.rs           targets.yaml, and which reference each ladder validates against
    assert.rs           Check and Failure: field paths, diffs, verbatim blocks
    report.rs           the terminal report and the --json schema
    catalog.rs          --list and catalog.json
    runner.rs           one test: which reference, which node or cluster, which timeout
    node/mod.rs         starting, watching and killing one process
    node/reference.rs   the etcd download and cache, and the example binaries
    etcd/mod.rs         the client: request builders and tolerant decoding
    etcd/http.rs        a small HTTP/1.1 client with keep-alive and chunked streams
    cluster/mod.rs      a cluster: leaders, kills, partitions, membership
    cluster/proxy.rs    the peer proxy and the fault table
    cluster/workload.rs the randomized workload and the seeded fault schedule
    lin/mod.rs          the linearizability checker
    prim/mod.rs         driving the CLI of ladders A and B
    prim/oracles.rs     what the answer should be, worked out independently
    examples/           the example specs, and --capture-examples
    stages/sNN_*.rs     one file per stage
  tests/                catalog freshness, the checker, the CLI, the codec
```

---

## 10. Adding a stage

1. Write `src/stages/sNN_slug.rs` with a `pub fn stage() -> Stage`.
2. Add two lines to `src/stages/mod.rs`: the `mod` and the `push`.
3. Add the stage number to a section in `sections()`, and a tickbox to `PLAN.md`.
4. Re-run `--capture-examples` and `--list --json catalog.json`.

**Stage numbers are append-only.** The site's resource library cites them, so a new stage
takes the next free number whatever ladder it belongs to; the `ladder` field and the section
letter are what place it. That is why the algorithms ladder is stages 56–88 and still sits
second in every listing.

A stage looks like this:

```rust
pub fn stage() -> Stage {
    Stage {
        number: 22,
        slug: "put_header",
        name: "Put and the response header",
        ext: false,
        ladder: Ladder::Node,
        hints: &["…", "…"],          // 2–4 lines: what to write, where the trap is
        examples,                     // 1–3 worked examples
        tests: vec![
            Test::new("a put answers with a header", a_put_answers_with_a_header),
            Test::new("the revision increases by one", revision_increases).ext(),
        ],
    }
}

dist_test!(a_put_answers_with_a_header, |ctx| {
    let key = ctx.key("k");
    let put = ok(ctx.kv()?.put(&key, b"v").await, "put k")?;
    let mut c = Check::new("the header of a put");
    c.at_least("put.header.revision", 1, put.header.revision);
    c.finish()
});
```

The rules the suite keeps to, and a new stage has to keep to as well:

* every stage carries **2–4 hints** and **1–3 examples**;
* a test never binds a fixed port and never writes outside its own temporary directory;
* a cluster test that kills a member, cuts a link or changes membership is marked `.fresh()`,
  and the runner throws a cluster away by itself once a test has dirtied it;
* keys are namespaced with `ctx.key(..)`, so tests sharing a cluster cannot collide;
* every random choice comes from `ctx.rng` or `ctx.seed`, so `--seed` reproduces a run
  exactly;
* no `unwrap()` or `expect()` on I/O, decoding or subprocess paths;
* a failure names the field path, shows what was expected and what arrived, and attaches
  whatever block would answer the next question — the body, the transcript, the history.
