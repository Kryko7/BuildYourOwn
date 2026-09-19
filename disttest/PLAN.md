# disttest stage plan

Tick a stage when `disttest --target my_node --stage N` is green. `disttest --list` reads
these boxes. Each entry names its **ladder** — `primitives`, `algorithms`, `node` or
`cluster` — its source file and its test count. Stages marked **[ext]** go beyond the core
track, and `--skip-ext` hides them.

Stage numbers are **append-only**: the algorithms ladder is the second rung a learner climbs
and is listed second everywhere, but it was added after stages 1-55 were already cited
elsewhere, so it took numbers 56-77 rather than moving anything. The `ladder` field and the
section letter are what place a stage, never its number.

Run one stage: `disttest --target my_node --stage 22` — everything so far: `--until 35` —
one ladder: `--tag algorithms --all` — the lot: `--all`. Prove the suite itself:
`disttest --target etcd --validate --all`, which routes each ladder to its own reference.

Every stage also carries 1-3 **worked examples**: a request and what the reference answered,
a transcript of a conversation with the line-oriented CLI, or a recorded history and the
linearizability checker's verdict on it. They live in the stage's own file, are recaptured
with `disttest --capture-examples examples/captured.json --target etcd`, and reach the site
through `catalog.json`. See README.md, "Adding a stage".

## A. Primitives: clocks and causality
Small deterministic exercises in a line-oriented CLI: `./your_program.sh <topic>`, one command per line in, one JSON object per line out. The tester works every answer out independently — brute force, closed-form arithmetic, or a statistical bound with the measured value printed next to it.

- [ ] **Stage 01** — Lamport clocks `primitives` (`src/stages/s01_lamport_clocks.rs`, 10 tests, 2 ext)
  - Topic `lamport`: keep one counter per process and hand it out with `send`
  - A local event and a send both increment the counter before it is used
  - On receive the counter becomes max(local, received) + 1 — the +1 is the whole point
  - Counters never go backwards, even when a message arrives from the past
- [ ] **Stage 02** — Vector clocks and causal comparison `primitives` (`src/stages/s02_vector_clocks.rs`, 10 tests, 2 ext)
  - Topic `vector-clock`: a map from process name to counter, and a missing entry is a zero
  - `cmp` answers before, after, equal or concurrent — four cases, not three
  - Merging on receive takes the componentwise maximum, then increments the receiver's own entry
  - Two clocks are concurrent when neither dominates: test both directions
- [ ] **Stage 03** — Version vectors and sibling detection **[ext]** `primitives` (`src/stages/s03_version_vectors.rs`, 10 tests)
  - Topic `version-vector`: a value carries the vector of the replica that wrote it
  - A write that does not dominate an existing value creates a sibling instead of replacing it
  - Sync merges both replicas' sets and drops any value dominated by another
  - Siblings come back sorted so two replicas that agree answer identically
- [ ] **Stage 04** — Hybrid logical clocks **[ext]** `primitives` (`src/stages/s04_hybrid_logical_clocks.rs`, 10 tests)
  - Topic `hlc`: a timestamp is (l, c) — a physical part and a counter that breaks ties
  - `now` takes l = max(l, wall); when l did not move, c increments, otherwise c resets to 0
  - `recv` takes l = max(l, l_local, l_msg) and picks c from whichever source it matched
  - The result must be strictly greater than both the local clock and the message's

## B. Primitives: placement, quorums and sketches
- [ ] **Stage 05** — Consistent hashing with virtual nodes `primitives` (`src/stages/s05_consistent_hashing.rs`, 9 tests, 2 ext)
  - Topic `consistent-hash`: place `vnodes` points per node on a ring and walk clockwise
  - `locate` is a function: the same key must always answer the same node
  - Virtual nodes are what makes the load even — one point per node is visibly lumpy
  - `stats` reports how many of the sampled keys landed on each node
- [ ] **Stage 06** — Key movement when the ring changes **[ext]** `primitives` (`src/stages/s06_ring_key_movement.rs`, 8 tests)
  - Adding a node may only take keys, never shuffle keys between two nodes that stayed
  - Removing a node may only give its keys away; everything else stays put
  - Roughly 1/N of the keys move when the Nth node joins — that is the whole point
  - Re-adding a node that was removed must restore the mapping exactly
- [ ] **Stage 07** — Rendezvous (HRW) hashing **[ext]** `primitives` (`src/stages/s07_rendezvous_hashing.rs`, 10 tests)
  - Topic `rendezvous`: score every node for the key and take the highest
  - No ring and no virtual nodes: the score function does all the work
  - `locate-k` returns the top k in score order, and its head is what `locate` answers
  - Removing a node only moves the keys it owned, exactly as a ring does
- [ ] **Stage 08** — Quorum math: N, R, W and overlap `primitives` (`src/stages/s08_quorum_math.rs`, 8 tests, 2 ext)
  - Topic `quorum`: a read and a write quorum overlap when R + W > N
  - Two writes can conflict when 2W <= N — that is a different question from overlap
  - The smallest read quorum that always sees the latest write is N - W + 1
  - Answer for the numbers you were given, not for the defaults
- [ ] **Stage 09** — Sloppy quorums and hinted handoff **[ext]** `primitives` (`src/stages/s09_sloppy_quorums.rs`, 9 tests)
  - A sloppy quorum accepts a write on a node outside the preference list when a member is down
  - The stand-in records a hint naming the node the write really belongs to
  - When the real owner comes back, the hint is handed off and then forgotten
  - A sloppy quorum is not a quorum: it can lose the overlap guarantee, and must say so
- [ ] **Stage 10** — Bloom filter `primitives` (`src/stages/s10_bloom_filter.rs`, 9 tests, 2 ext)
  - Topic `bloom`: k hash positions per item over m bits, all set on add
  - `contains` is only ever 'maybe' or 'no': a false negative is a bug, never a tuning issue
  - The measured false-positive rate must sit inside the theoretical bound for m, k and n
  - Derive the k positions from one or two hashes; k independent hash functions are not needed
- [ ] **Stage 11** — HyperLogLog **[ext]** `primitives` (`src/stages/s11_hyperloglog.rs`, 9 tests)
  - Topic `hll`: 2^p registers, each holding the longest run of leading zeros it has seen
  - The estimate is the harmonic mean of the registers, times alpha times m squared
  - Small cardinalities need linear counting, or the estimate is badly wrong
  - The relative error must sit inside 1.04 / sqrt(2^p) over many trials
- [ ] **Stage 12** — Merkle tree build `primitives` (`src/stages/s12_merkle_tree.rs`, 10 tests, 2 ext)
  - Topic `merkle`: leaf = sha256(0x00 || leaf bytes), node = sha256(0x01 || left hex || right hex)
  - An odd node at a level is carried up unchanged, not duplicated
  - The root changes when any leaf changes, and only then
  - The same leaves always produce the same root, whatever order they were added in
- [ ] **Stage 13** — Merkle diff with the fewest round trips **[ext]** `primitives` (`src/stages/s13_merkle_diff.rs`, 9 tests)
  - Compare roots first: equal roots mean equal trees and no further work
  - Descend only into subtrees whose hashes differ — that is the whole saving
  - Report the differing leaves as ranges, so a caller can fetch them in one go
  - Count the nodes you compared: a walk that visits everything has learnt nothing

## C. Primitives: CRDTs, snapshots and timing
- [ ] **Stage 14** — G-Counter and PN-Counter `primitives` (`src/stages/s14_counters.rs`, 9 tests, 1 ext)
  - Topics `g-counter` and `pn-counter`: one entry per replica, merged by maximum
  - A PN-Counter is two G-Counters: increments and decrements, never one signed number
  - Merge must be commutative, associative and idempotent — every delivery order converges
  - A replica only ever writes its own entry; it copies others by taking the maximum
- [ ] **Stage 15** — LWW-Register and OR-Set `primitives` (`src/stages/s15_registers_and_sets.rs`, 9 tests)
  - Topic `lww-register`: value plus timestamp, and a deterministic tie-break on equal timestamps
  - Topic `or-set`: an add carries a unique tag; a remove takes away the tags it saw
  - An add concurrent with a remove survives — that is the whole difference from a 2P-Set
  - Re-adding after a remove must work, which is why tags cannot be reused
- [ ] **Stage 16** — RGA: a replicated sequence **[ext]** `primitives` (`src/stages/s16_rga_sequence.rs`, 8 tests)
  - Topic `rga`: every element has an id and points at the element it was inserted after
  - Concurrent inserts at the same position are ordered by id, consistently on every replica
  - A delete leaves a tombstone: later inserts may still point at it
  - Every delivery order of the same operations must read back the same sequence
- [ ] **Stage 17** — Chandy-Lamport snapshots **[ext]** `primitives` (`src/stages/s17_chandy_lamport.rs`, 9 tests)
  - Topic `snapshot`: a process records its own state when it first sees a marker
  - After that it records every message arriving on a channel until that channel's marker
  - The snapshot's total must equal the system's total, whatever the interleaving
  - A process sends a marker on every outgoing channel immediately after recording
- [ ] **Stage 18** — Causal broadcast delivery order `primitives` (`src/stages/s18_causal_broadcast.rs`, 9 tests, 2 ext)
  - Topic `causal-broadcast`: a message carries the sender's vector clock
  - Deliver only when the message is the sender's next, and its other entries are already seen
  - Anything else waits in a buffer and is re-checked after every delivery
  - Delivering a message out of causal order is the failure this stage looks for
- [ ] **Stage 19** — Token bucket and leaky bucket `primitives` (`src/stages/s19_rate_limiting.rs`, 9 tests, 1 ext)
  - Topic `token-bucket`: refill rate * elapsed, capped at the burst, then spend
  - Topic `leaky-bucket`: drain rate * elapsed, then refuse whatever would overflow
  - Time arrives with every command: never read a clock of your own
  - A long idle period fills the token bucket to the burst and no further
- [ ] **Stage 20** — Backoff, phi-accrual and SWIM suspicion **[ext]** `primitives` (`src/stages/s20_failure_detection.rs`, 9 tests)
  - Topic `backoff`: full jitter is uniform(0, min(cap, base * 2^attempt))
  - Topic `phi-accrual`: phi rises with the time since the last heartbeat, smoothly
  - Topic `swim`: alive, then suspect, then dead — two deadlines, both sharp
  - The sampled value arrives with the command, so a jittered delay is still reproducible

## D. Single node: the key/value API
One server speaking a subset of the etcd v3 HTTP/JSON API, validated against real etcd 3.7.1. Revisions, MVCC, transactions, compaction, leases, watches — and a durability stage that kills the process mid-workload.

- [ ] **Stage 21** — Bind, /version and /health `node` (`src/stages/s21_bind_version_health.rs`, 8 tests, 1 ext)
  - Listen on the --listen-client-urls address and answer HTTP/1.1 on it
  - GET /version answers {"etcdserver":..,"etcdcluster":..} and GET /health {"health":"true"}
  - Keep the connection alive: the suite reuses one connection for thousands of requests
  - An unknown path is 404, and a GET on a POST-only endpoint is not a 200
- [ ] **Stage 22** — Put and the response header `node` (`src/stages/s22_put_header.rs`, 9 tests, 2 ext)
  - POST /v3/kv/put with base64 key and value, answering a header
  - Every write increments the store revision by one, and the header reports the new value
  - 64-bit numbers are JSON strings; a field at its zero value may be left out entirely
  - raft_term is in the header too, and it never goes backwards
- [ ] **Stage 23** — Range: reading one key `node` (`src/stages/s23_range_one_key.rs`, 8 tests)
  - POST /v3/kv/range with just a key is a point read
  - A key that is not there answers no kvs and count 0 — not an error
  - create_revision is set once, mod_revision moves with every put, version counts the puts
  - count is the number of keys the range holds, whatever limit was asked for
- [ ] **Stage 24** — Ranges, prefixes and the whole store `node` (`src/stages/s24_range_over_ranges.rs`, 8 tests)
  - range_end makes the request a half-open range [key, range_end)
  - A prefix range ends at the key with its last byte incremented
  - key and range_end both   means every key in the store
  - Results come back in key order, which is byte order, not string order
- [ ] **Stage 25** — limit, sort order, count_only and keys_only **[ext]** `node` (`src/stages/s25_range_options.rs`, 9 tests)
  - limit cuts the answer and sets more, but never changes count
  - sort_order and sort_target together decide the order; the default is ascending by key
  - count_only answers the count and no kvs at all
  - keys_only answers the kvs with their values left out, not with empty values invented
- [ ] **Stage 26** — MVCC: reading at a past revision **[ext]** `node` (`src/stages/s26_mvcc_past_revisions.rs`, 8 tests)
  - revision in a range request reads the store as it was at that revision
  - A key deleted later is still there when read at a revision before the delete
  - The header of a historical read still reports the store's current revision
  - Revision 0 means 'now', because 0 is the zero value and cannot mean 'the beginning'
- [ ] **Stage 27** — prev_kv and deleting ranges `node` (`src/stages/s27_prev_kv_and_delete.rs`, 9 tests, 1 ext)
  - prev_kv on a put answers the pair as it was before the write, or nothing when it is new
  - POST /v3/kv/deleterange answers how many keys it removed
  - A delete over a range removes every key in it and counts them all
  - Deleting a key that is not there is a successful request that deleted zero keys
- [ ] **Stage 28** — Transactions: compare, success, failure `node` (`src/stages/s28_txn_branches.rs`, 8 tests)
  - POST /v3/kv/txn runs the success branch when every comparison holds, the failure one otherwise
  - succeeded is false when the failure branch ran, and may be omitted rather than sent as false
  - responses carries one entry per operation of the branch that ran, in order
  - A transaction is one atomic step: its writes share one revision
- [ ] **Stage 29** — Compare-and-swap semantics **[ext]** `node` (`src/stages/s29_txn_compare_and_swap.rs`, 9 tests)
  - VERSION = 0 is how you say 'this key does not exist'
  - CREATE, MOD, VALUE and VERSION are four different comparisons with four different fields
  - Two concurrent swaps from the same value: exactly one may succeed
  - A comparison naming a key that does not exist compares against zeros, it does not fail the request
- [ ] **Stage 30** — Compaction and ErrCompacted **[ext]** `node` (`src/stages/s30_compaction.rs`, 9 tests)
  - POST /v3/kv/compaction drops every revision below the one given
  - A read below the compacted revision is code 11, not an empty answer
  - Compaction never removes the newest version of a live key
  - Compacting twice, or below what is already compacted, is an error, not a no-op

## E. Single node: leases, watches and durability
- [ ] **Stage 31** — Leases: grant, attach, expire, revoke `node` (`src/stages/s31_leases.rs`, 9 tests, 2 ext)
  - POST /v3/lease/grant answers an ID and the TTL it settled on
  - A put naming a lease binds the key to it; the kv answers with that lease id
  - When the lease expires every key attached to it disappears in one revision
  - Revoking is the same thing on demand, and revoking twice is an error
- [ ] **Stage 32** — Lease keepalive and TTL **[ext]** `node` (`src/stages/s32_lease_keepalive.rs`, 8 tests)
  - POST /v3/lease/keepalive is a stream: the answer is wrapped in {"result": ...}
  - A keepalive resets the lease's remaining time to its TTL
  - A keepalive for a lease that is gone answers TTL 0, not an error
  - Keys survive exactly as long as something keeps the lease alive
- [ ] **Stage 33** — Watches **[ext]** `node` (`src/stages/s33_watches.rs`, 9 tests)
  - POST /v3/watch is a stream: every message is a line of JSON wrapped in {"result": ...}
  - The first message acknowledges the create request and carries created: true
  - start_revision replays everything from that revision onwards, in revision order
  - A delete event carries type DELETE and a kv with only the key and mod_revision
- [ ] **Stage 34** — Error responses **[ext]** `node` (`src/stages/s34_error_responses.rs`, 8 tests)
  - Errors are a 4xx status and a body of {"code": <grpc code>, "message": "..."}
  - Bad base64 is code 3, a lease that does not exist is code 5, too large is code 8
  - An unknown path is 404 and never a 200 with an empty body
  - A bad request must not change the store, and must not close the connection
- [ ] **Stage 35** — Durability across SIGKILL **[ext]** `node` (`src/stages/s35_durability.rs`, 9 tests)
  - An acknowledged write must survive kill -9 and a restart: fsync before you answer
  - The data directory must be reopenable without being reformatted
  - The revision sequence continues where it left off; it never restarts at 1
  - A torn record at the tail of the log may be dropped, but never an acknowledged write

## F. Cluster: replication and agreement
Three or five of those servers, with a userspace TCP proxy in front of every peer URL so the harness can partition, delay, drop, duplicate and reorder peer traffic with no privileges. It ends with a seeded fault schedule and a linearizability check over the recorded history.

- [ ] **Stage 36** — A cluster forms and elects one leader `cluster` (`src/stages/s36_cluster_forms.rs`, 10 tests, 2 ext)
  - --initial-cluster lists every member as name=peer-url, and every member gets the same list
  - Dial the peer URL you were told, not the address the member listens on
  - /v3/maintenance/status reports the leader's member id and the current raft term
  - Every member must name the same leader, and the term must be the same on all of them
- [ ] **Stage 37** — A write on any member is visible on every member `cluster` (`src/stages/s37_replicated_writes.rs`, 9 tests, 2 ext)
  - A write accepted by a follower is forwarded to the leader, not applied locally
  - The revision a member answers with is the cluster's, not its own counter
  - Every member must end up holding the same value for the key
  - Writes through different members still produce one increasing revision sequence
- [ ] **Stage 38** — Linearizable reads after a write elsewhere `cluster` (`src/stages/s38_linearizable_reads.rs`, 9 tests, 2 ext)
  - A read that is not serializable must not answer from a stale local state
  - Confirm leadership (a read index, or a round of heartbeats) before answering
  - A write acknowledged by one member must be visible to a read on any other, immediately
  - serializable: true is the opt-out, and it is the only way to answer locally
- [ ] **Stage 39** — Revisions and terms agree across members **[ext]** `cluster` (`src/stages/s39_revision_agreement.rs`, 9 tests)
  - cluster_id is the same on every member; member_id is not
  - Revisions are cluster-wide: two members never answer different revisions for the same state
  - raft_term is the same on every member once an election has settled
  - raft_applied_index catches up to raft_index on a quiet cluster
- [ ] **Stage 40** — The minority of a partition refuses writes **[ext]** `cluster` (`src/stages/s40_minority_refuses_writes.rs`, 8 tests)
  - A member that cannot reach a quorum must refuse a write rather than apply it locally
  - It must also refuse a linearizable read: answering from stale state is the same bug
  - Refuse with an error the client can see, and do not hang forever
  - A serializable read may still be answered, and may be stale — that is what it means
- [ ] **Stage 41** — The majority of a partition keeps serving **[ext]** `cluster` (`src/stages/s41_majority_keeps_serving.rs`, 8 tests)
  - A quorum of members is enough: the cluster does not need everyone
  - The majority side elects a leader among itself if the old one was cut off
  - Writes keep succeeding on the majority side while the partition stands
  - The revision keeps advancing, and the minority side knows nothing about it
- [ ] **Stage 42** — A healed partition catches the minority up **[ext]** `cluster` (`src/stages/s42_partition_heals.rs`, 8 tests)
  - When the link comes back, the stale member learns the entries it missed
  - It adopts the higher term and steps down if it still thought it was leader
  - Nothing the minority side accepted may appear in the log afterwards
  - The catch-up must finish without a restart or an operator

## G. Cluster: failure, recovery and membership
- [ ] **Stage 43** — A killed leader is replaced **[ext]** `cluster` (`src/stages/s43_leader_election.rs`, 8 tests)
  - A follower that stops hearing from the leader starts an election
  - Only a member whose log is at least as up to date may win
  - The new term is strictly greater than the old one
  - The cluster must be serving again within a bounded time, not eventually
- [ ] **Stage 44** — No acknowledged write is lost **[ext]** `cluster` (`src/stages/s44_no_acknowledged_write_lost.rs`, 8 tests)
  - A write is only acknowledged once a quorum has it on disk
  - After the leader is killed, every acknowledged write must still be readable
  - A write that was never acknowledged may be there or not: both are correct
  - This is the property the whole design exists to provide
- [ ] **Stage 45** — An old leader rejoins without resurrecting anything **[ext]** `cluster` (`src/stages/s45_old_leader_rejoins.rs`, 8 tests)
  - A leader that was cut off may hold entries no quorum ever accepted
  - On rejoining it sees a higher term, steps down, and truncates those entries
  - A client that read from the new majority must never see the old entries appear
  - Its own view of the key must become the cluster's, not the other way round
- [ ] **Stage 46** — A long-partitioned follower catches up **[ext]** `cluster` (`src/stages/s46_follower_catch_up.rs`, 8 tests)
  - The leader keeps sending from the follower's next index until it catches up
  - Hundreds of missed writes must arrive without a restart
  - Until it has caught up, the follower must not answer linearizable reads from stale state
  - Catching up must not disturb the members that were healthy
- [ ] **Stage 47** — A far-behind follower is caught up by snapshot **[ext]** `cluster` (`src/stages/s47_snapshot_catch_up.rs`, 8 tests)
  - When the entries a follower needs have been compacted away, send a snapshot instead
  - The follower replaces its whole state with the snapshot and continues from its index
  - After a snapshot install the follower answers the same values as everyone else
  - Compaction on the leader is what makes this path necessary at all
- [ ] **Stage 48** — Adding a member **[ext]** `cluster` (`src/stages/s48_member_add.rs`, 7 tests)
  - POST /v3/cluster/member/add takes the new member's peer URLs and changes the configuration
  - The new member starts with --initial-cluster-state existing and the full member list
  - The cluster keeps serving throughout: a configuration change is one more log entry
  - The new member catches up from the leader and then counts towards the quorum
- [ ] **Stage 49** — Removing a member **[ext]** `cluster` (`src/stages/s49_member_remove.rs`, 8 tests)
  - POST /v3/cluster/member/remove takes the member id, not its name
  - The quorum size changes with the membership, so removing a member can restore availability
  - The removed member must stop being counted, and the rest must keep serving
  - Removing a member that does not exist is an error, not a silent success
- [ ] **Stage 50** — Restarting the whole cluster loses nothing **[ext]** `cluster` (`src/stages/s50_whole_cluster_restart.rs`, 9 tests)
  - Every member restarts from its own data directory with no reformatting
  - The cluster re-elects a leader and continues from the revision it had
  - Every acknowledged write is still there, with the same revisions
  - A member that restarts must not rejoin as a brand new one
- [ ] **Stage 51** — Clock skew changes nothing about safety **[ext]** `cluster` (`src/stages/s51_clock_skew.rs`, 8 tests)
  - Raft safety rests on terms and indexes, never on wall-clock time
  - A member whose clock is hours off must still agree on every value
  - Lease expiry may be affected; linearizability may not
  - Never compare timestamps from two different machines to order writes

## H. Cluster: linearizability under fault injection
- [ ] **Stage 52** — A slow, lossy link degrades throughput, not safety **[ext]** `cluster` (`src/stages/s52_lossy_link.rs`, 8 tests)
  - Delay and connection loss slow the cluster down; they never change what it answers
  - A transport that cannot deliver must retry, not drop the entry
  - Throughput falls and latency rises: the test records both and asserts neither
  - Every acknowledged write must still be readable when the link recovers
- [ ] **Stage 53** — Duplicated and reordered peer messages are tolerated **[ext]** `cluster` (`src/stages/s53_duplicate_and_reorder.rs`, 8 tests)
  - A retransmitted append must be idempotent: applying it twice changes nothing
  - An out-of-order append is rejected on its previous index, not applied at the wrong place
  - Terms and indexes are what make this safe, not the order bytes happen to arrive in
  - The cluster must keep making progress, not merely avoid corruption
- [ ] **Stage 54** — Linearizability under fault injection **[ext]** `cluster` (`src/stages/s54_linearizability.rs`, 8 tests)
  - A randomized concurrent workload runs while the seeded schedule cuts and heals the network
  - Every call is recorded with the instant it was made and the instant it answered
  - An operation whose answer was lost may have happened or not; everything else is pinned
  - The checker looks for one total order that a single key/value store could have produced
- [ ] **Stage 55** — Five members, mixed faults, and nothing left behind **[ext]** `cluster` (`src/stages/s55_five_node_soak.rs`, 7 tests)
  - Five members tolerate two failures; the quorum arithmetic is the only thing that changes
  - Partitions, kills, delays and message mangling all in one run
  - After the faults heal, every member must converge on the same values
  - No process, port or temporary file may survive the end of the run

## I. Algorithms: consensus
The classics as exercises in their own right, over the same line-oriented CLI: Raft, Paxos, two- and three-phase commit, sagas, outboxes, fencing tokens, gossip and circuit breakers. Every oracle is the tester's own encoding of the specified rules, and every famous trap - Figure 8, the 2PC blocking window, compensation order, the fencing scenario, Paxos livelock - has a test of its own.

- [ ] **Stage 56** — Raft leader election `algorithms` (`src/stages/s56_raft_leader_election.rs`, 15 tests, 4 ext)
  - Topic `raft-election`: one node's role, term, votedFor and vote tally
  - Any message carrying a higher term makes this node a follower and clears votedFor
  - One vote per term, and only for a candidate whose log is at least as up to date
  - Count each voter once: a retransmitted vote must not make a minority a majority
- [ ] **Stage 57** — Raft log replication `algorithms` (`src/stages/s57_raft_log_replication.rs`, 11 tests, 3 ext)
  - Topic `raft-log`: a log is an array of entry terms, index 1 upwards
  - Refuse the message when prev_index is past the end, or its term disagrees — and leave the log exactly as it was
  - Truncate only at the first entry that really conflicts; entries that already match must stay, tail and all
  - commit_index is min(leader_commit, the index of the last new entry), and it never goes backwards
- [ ] **Stage 58** — Raft commitment and the Figure 8 trap `algorithms` (`src/stages/s58_raft_commit_safety.rs`, 11 tests, 3 ext)
  - Topic `raft-commit`: matchIndex and nextIndex per follower, and one commit index
  - The leader counts itself, so a five-member cluster commits on two acknowledgements
  - Advance the commit index only to an entry of the leader's *own* term — §5.4.2, the Figure 8 trap — and let it carry the earlier entries with it
  - An acknowledgement that arrived late must never lower a match index
- [ ] **Stage 59** — Raft snapshots and log compaction `algorithms` (`src/stages/s59_raft_snapshots.rs`, 11 tests, 2 ext)
  - Topic `raft-snapshot`: the log no longer starts at index 1, so keep first_index, snapshot_index and snapshot_term
  - Never compact past the commit index: an uncommitted entry may still be truncated
  - The snapshot boundary still answers a consistency check, so keep its term; an append that reaches below the boundary can only be refused
  - An InstallSnapshot the follower already covers is ignored; one whose boundary it holds keeps the tail; anything else replaces the whole log
- [ ] **Stage 60** — Raft membership change `algorithms` (`src/stages/s60_raft_membership.rs`, 11 tests, 2 ext)
  - Topic `raft-membership`: a configuration is a set of voters, and its quorum is len/2 + 1
  - In the joint phase a decision needs a majority of C_old *and* a majority of C_new
  - Leaving the joint phase takes two committed steps: C_old,new, then C_new
  - One change at a time: refuse a second while the first is still uncommitted
- [ ] **Stage 61** — Single-decree Paxos `algorithms` (`src/stages/s61_paxos_single_decree.rs`, 15 tests, 3 ext)
  - Topic `paxos`: every acceptor keeps a promised number and its highest accepted proposal
  - An acceptor promises a number only when it is strictly greater than the last one
  - Accepting implies promising, and an accept below the promise is refused outright
  - A proposer must propose the value of the highest-numbered proposal its promises reported, and its own only when none did
- [ ] **Stage 62** — Multi-Paxos and a stable leader `algorithms` (`src/stages/s62_multi_paxos.rs`, 14 tests, 2 ext)
  - Topic `multi-paxos`: one ballot, every slot — phase one is run once, not per slot
  - While a proposer holds a majority of promises, every proposal costs one round trip; without them it costs two
  - A higher ballot strips leadership but never un-chooses a slot that was settled
  - The state machine applies a contiguous prefix: a gap blocks every slot above it

## J. Algorithms: atomic commit
- [ ] **Stage 63** — Two-phase commit `algorithms` (`src/stages/s63_two_phase_commit.rs`, 13 tests, 3 ext)
  - Topic `two-phase-commit`: a coordinator state machine and one per participant
  - One no is enough to abort; commit needs every vote, so a missing vote decides nothing
  - A yes vote is a promise: from then on the participant may not decide for itself
  - Cooperative termination settles some cases and not the one that matters
- [ ] **Stage 64** — Three-phase commit `algorithms` (`src/stages/s64_three_phase_commit.rs`, 13 tests, 4 ext)
  - Topic `three-phase-commit`: vote, then pre-commit, then commit
  - Pre-commit is only legal once every participant has voted yes
  - On a timeout a pre-committed participant commits and a merely prepared one aborts
  - That rule reads 'no pre-commit arrived' as 'none was sent', which a partition breaks
- [ ] **Stage 65** — Commit recovery from the log `algorithms` (`src/stages/s65_commit_recovery.rs`, 14 tests, 4 ext)
  - Topic `commit-recovery`: a durable log, a durable store, and volatile state a crash eats
  - The prepare record is forced out before the vote is sent, never after
  - No prepare record means presumed abort; a prepare record with no decision means ask
  - A commit record whose effect never landed is redone, and redo is idempotent

## K. Algorithms: sagas and messaging
- [ ] **Stage 66** — Orchestrated saga `algorithms` (`src/stages/s66_saga_orchestrated.rs`, 10 tests, 3 ext)
  - Topic `saga`: remember the steps that completed, and undo them newest first
  - A failure at step k compensates k-1 … 1; the step that failed never completed
  - Compensations are retried, so running one twice must leave one undo behind
  - A compensation that fails stays at the head of the queue until it succeeds
- [ ] **Stage 67** — Choreographed saga `algorithms` (`src/stages/s67_saga_choreographed.rs`, 10 tests, 3 ext)
  - Topic `saga-choreo`: each service reacts to one event and emits the next
  - `<step>.fail` starts the chain; `<step>.undone` walks it one step further back
  - The bus is at-least-once, so an event already handled must emit nothing
  - Once a chain is running, a second failure is absorbed — one saga, one chain
- [ ] **Stage 68** — Transactional outbox `algorithms` (`src/stages/s68_transactional_outbox.rs`, 10 tests, 3 ext)
  - Topic `outbox`: the row and its event record commit in one transaction
  - A relay drains the table afterwards; publishing is not part of the write
  - A crash between publishing and acknowledging leaves the record, so it is resent
  - The outbox buys 'never lost'; only a deduplicating consumer buys 'never twice'
- [ ] **Stage 69** — Idempotent consumer `algorithms` (`src/stages/s69_idempotent_consumer.rs`, 9 tests, 3 ext)
  - Topic `dedup`: look the message id up before applying the effect, not after
  - The table is a queue of first sightings: the oldest id is always the one evicted
  - A redelivery is not a refresh — an entry ages from when it was first seen
  - Exactly-once holds only while the table still remembers every id it applied
- [ ] **Stage 70** — Retry with an idempotency key `algorithms` (`src/stages/s70_idempotency_keys.rs`, 10 tests, 3 ext)
  - Topic `idempotency-key`: store the answer under the key, and replay it on a retry
  - A key is bound to its request: the same key with a different amount is an error
  - A key whose first request is still running refuses the second, it does not race it
  - The client cannot tell a lost answer from a lost request, which is why it retries

## L. Algorithms: coordination and resilience
- [ ] **Stage 71** — Distributed locks and fencing tokens `algorithms` (`src/stages/s71_fencing_tokens.rs`, 10 tests, 2 ext)
  - Topic `fencing`: every grant hands out a token one higher than the last
  - The resource keeps a fence: the highest token it has accepted a write from
  - A write below the fence is refused, however firmly its client believes it holds the lock
  - Checking `holder` before writing proves nothing: the pause happens after the check
- [ ] **Stage 72** — Leader leases and clock skew **[ext]** `algorithms` (`src/stages/s72_leader_leases.rs`, 10 tests)
  - Topic `leases`: `now` always arrives as an argument; never read a clock of your own
  - The holder stops serving at `expires_at - clock_error`, because its clock may be slow
  - The granter waits until `expires_at + clock_error`, because the old holder's may be slow too
  - Renewing extends from `now`, not from the old expiry: a late renewal buys no extra time
- [ ] **Stage 73** — Anti-entropy: read repair and hinted handoff `algorithms` (`src/stages/s73_anti_entropy.rs`, 10 tests, 2 ext)
  - Topic `anti-entropy`: a read takes the highest version any live replica holds
  - Repair exactly the replicas that were behind — repairing the current ones is wasted work
  - A write to a replica that is down leaves a hint on the first live replica instead
  - A hint dies with its stand-in; only `sync` is a guarantee, and it compares everything
- [ ] **Stage 74** — Gossip dissemination `algorithms` (`src/stages/s74_gossip_dissemination.rs`, 10 tests, 2 ext)
  - Topic `gossip`: nodes are 0..n-1, node 0 starts infected, and the schedule is fixed
  - In round r every infected node i contacts (i + (fanout+1)^(r-1) * k) mod n for k = 1..=fanout
  - Everyone contacted becomes infected at the end of the round, not during it
  - Count every message, including the ones that land on a node that already knew
- [ ] **Stage 75** — Circuit breaker `algorithms` (`src/stages/s75_circuit_breaker.rs`, 10 tests, 2 ext)
  - Topic `circuit-breaker`: closed, open, half-open, and `now` always arrives as an argument
  - It is *consecutive* failures that open the circuit; one success resets the count
  - A call rejected while open never reached the dependency, so it is not evidence about it
  - A failed trial reopens the circuit and restarts the window from that instant
- [ ] **Stage 76** — Hedged requests **[ext]** `algorithms` (`src/stages/s76_hedged_requests.rs`, 9 tests)
  - Topic `hedging`: service times arrive as `[primary_ms, hedge_ms]`; never time anything yourself
  - Nothing is hedged below the threshold: `primary < hedge_after_ms` costs one message
  - A hedged request answers in min(primary, hedge_after_ms + hedge) — the hedge starts late
  - Cancel the loser: `inflight` is zero the moment either replica has answered
- [ ] **Stage 77** — Bulkheads and load shedding **[ext]** `algorithms` (`src/stages/s77_bulkheads.rs`, 9 tests)
  - Topic `bulkhead`: one concurrency limit per pool, and `now` arrives as an argument
  - A full pool rejects at once — an unbounded queue is how one slow dependency takes everything
  - Pools are independent: saturating one must not change what another admits
  - Shedding work whose deadline has passed is free capacity, and it is not a rejection

## Totals

| ladder | stages | tests |
|---|---:|---:|
| `primitives` | 20 | 183 |
| `algorithms` | 22 | 245 |
| `node` | 15 | 128 |
| `cluster` | 20 | 164 |
| **all** | **77** | **720** |
