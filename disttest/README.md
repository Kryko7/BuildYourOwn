# disttest

A stage-by-stage, black-box conformance tester for a **distributed key/value system** you
write yourself. Track id `dist`.

```
disttest --target my_node --stage 22
disttest --target my_node --until 35 --json report.json
disttest --target my_node --all --skip-ext
disttest --target etcd --validate --all          # must be all green — the suite's self-check
disttest --list                                  # stages, ladders, test counts, PLAN.md ticks
disttest --list --json > catalog.json            # the stage catalog the site reads
```

Nothing here implements the thing you are building. This repository holds the harness, the
suite, the oracles, the fault injector and the docs.

---

## 1. Three ladders

This track is deliberately **multi-level**. You do not start by writing a replicated store;
you start by writing a vector clock, and you finish by surviving a partition while a
linearizability checker watches. Every stage carries a `ladder` tag, which is also a `--tag`
value and a field in `catalog.json`.

| ladder | stages | what your program is | reference for `--validate` |
|---|---|---|---|
| `primitives` | 01–20 | a line-oriented CLI, one JSON object per command | `reference_primitives` (this crate's own example) |
| `node` | 21–35 | one server speaking a subset of the etcd v3 HTTP/JSON API | real **etcd 3.7.1** |
| `cluster` | 36–55 | three or five of those servers, replicating | real **etcd 3.7.1** |

```
disttest --target my_node --tag primitives --all      # just ladder A
disttest --target my_node --tag node --all            # just ladder B
disttest --target my_node --tag cluster --all         # just ladder C
```

One program serves all three ladders. The harness decides which ladder it is asking for by
the arguments it appends to your `command` in `targets.yaml`: a bare topic name means "be a
primitives CLI", the etcd flag subset means "be a server".

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
| `build <id> <leaf,leaf,...>` | `{"root": "<64 hex>", "leaves": n}` |
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

## 3. Ladder B — one node

### 3.1 The program contract

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
| `POST /v3/cluster/member/{list,add,remove}` | membership (ladder C) |
| `GET /version`, `GET /health` | what they say |

### 3.2 Two properties of this wire format

Both are protobuf's JSON mapping, and both are load-bearing:

* **64-bit integers are strings.** `"revision":"7"`, not `7`. The harness accepts a plain
  number too, because that is what a hand-written server usually emits first, but the
  reference always writes strings.
* **A field at its zero value may be omitted entirely.** `{"count":"0"}` and no `count` at
  all mean the same thing, and real etcd omits it. No test in this suite ever asserts that a
  field is *present*; everything decodes a missing field as its zero.

### 3.3 Worked exchanges

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

The codes the suite names: **3** invalid argument (bad base64, an impossible range), **5**
not found (a lease that is gone), **8** resource exhausted (too large), **11** out of range
(compacted), **14** unavailable (no quorum, no leader).

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

### 3.4 Durability

Stage 35 is the one that hurts. The harness runs a workload, `SIGKILL`s the process in the
middle of it, starts it again from the same `--data-dir`, and demands that **every
acknowledged write is still there**. Nothing is flushed for you: if you answered before the
write reached the disk, you will fail, and `examples/broken_node.rs` exists to show you
exactly what that failure looks like.

---

## 4. Ladder C — a cluster

### 4.1 How a cluster is started

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

### 4.2 The fault-injection model

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

**Why duplication and reordering need framing.** Deleting or swapping *bytes* of a TCP stream
corrupts it; it does not model a network. In message mode the proxy parses the dialer's
direction as HTTP/1.1 requests — which is what a raft transport over HTTP sends — and
duplicates or swaps whole messages. A stream it cannot frame (a chunked one, say) is passed
through untouched and counted, and the stage reports how many messages were actually mangled
rather than pretending.

Everything is driven by `--seed`: which fault fires, when, and on which members.

### 4.3 The linearizability checker

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

## 5. Targets and `--validate`

`targets.yaml`:

```yaml
targets:
  etcd:                 # reference for the node and cluster ladders
    kind: reference
    version: "3.7.1"
  reference_primitives: # reference for the primitives ladder
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
whatever `--target` said.** `primitives` is routed to `reference_primitives`; `node` and
`cluster` are routed to `etcd`. That is why one command can prove the whole suite:

```
disttest --target etcd --validate --all
  primitives  validated against reference_primitives
  node        validated against etcd
  cluster     validated against etcd
```

`--target` still has to name a registered reference — passing your own program to
`--validate` is a usage error, because a self-check against an unproven implementation proves
nothing. `--validate` never changes which stages are selected: `--validate --stage 5` runs
stage 5, and `--validate` on its own means `--all`. A failure under `--validate` is worded as
a **suite bug**, because the reference is by definition right.

`reference_primitives` exists only so the primitives ladder can be self-checked, exactly as
`broken_node` exists only so the red output can be demonstrated. **Reading
`examples/reference_primitives.rs` spoils ladder A.**

### Watching it go red

```
disttest --target broken_node --stage 35
```

`broken_node` speaks enough of the API to get through the early node stages and is then wrong
in one specific, famous way: it acknowledges a write before the write is durable. Stage 35
kills it mid-workload and the acknowledged write is gone.

---

## 6. Flags

| flag | meaning |
|---|---|
| `--target <name\|path>` | the program to test: a name from `targets.yaml`, or a path |
| `--stage N` | run one stage |
| `--until N` | run stages `1..=N` (or `--from..=N`) |
| `--from N` | run stages `N..` (or `N..=--until`) |
| `--all` | run every stage |
| `--only SUBSTR` | only tests whose name contains this |
| `--tag T` | only tests carrying this tag: a ladder, `ext`, or any tag a stage added |
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

## 7. The JSON schemas

### 7.1 `--json report.json`

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
  "passed": 431, "failed": 0, "skipped": 0, "elapsed_ms": 512340
}
```

`status` is `pass`, `fail` or `skip`. `failure_kind` is one of `assertion`, `timeout`,
`connection`, `protocol`, `node_crash`, `linearizability`, `harness` — a crashed node never
looks like a wrong value.

### 7.2 `catalog.json`

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

`kind` is `node`, `cluster`, `primitives` or `workload`. `*_hex` holds the exact bytes that
went over the wire or the pipe, so an example can never quietly drift from what the suite
actually sends; the readable form is in `request` and `response`. Fields marked `varies` are
the ones that legitimately differ every run — ids, revisions, terms.

`catalog.json` is committed, and `tests/catalog_is_current.rs` fails when it drifts:

```
disttest --capture-examples examples/captured.json --target etcd   # only when bytes moved
disttest --list --json catalog.json
```

---

## 8. Layout

```
disttest/
  Cargo.toml
  targets.yaml          the programs disttest can point at
  PLAN.md               the tickbox stage plan; --list reads its boxes
  catalog.json          --list --json output, committed
  examples/
    reference_primitives.rs   ladder A's reference (reading it spoils ladder A)
    prim/*.rs                 one file per group of topics
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
    prim/mod.rs         driving the primitives CLI
    prim/oracles.rs     what the answer should be, worked out independently
    examples/           the example specs, and --capture-examples
    stages/sNN_*.rs     one file per stage
  tests/                catalog freshness, the checker, the CLI, the codec
```

---

## 9. Adding a stage

1. Write `src/stages/sNN_slug.rs` with a `pub fn stage() -> Stage`.
2. Add two lines to `src/stages/mod.rs`: the `mod` and the `push`.
3. Add the stage number to a section in `sections()`, and a tickbox to `PLAN.md`.
4. Re-run `--capture-examples` and `--list --json catalog.json`.

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
