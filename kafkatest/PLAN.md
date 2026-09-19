# kafkatest stage plan

Tick a stage when `kafkatest --broker my_broker --stage N` is green. `kafkatest --list` reads
these boxes. Stages marked **[ext]** go beyond the core track (`--skip-ext`
hides them). All 45 stages are implemented — 296 tests — so no entry says **(planned)** any
more; each one names its source file and its test count. See README.md, "Adding a stage".

Run one stage: `kafkatest --broker my_broker --stage 5` — everything so far: `--until 12` —
the lot: `--all`. Prove the suite itself: `kafkatest --broker apache_kafka --validate --all`.

Every stage also carries 1-3 **worked examples** — the exact request bytes and the bytes
Apache Kafka 4.1.2 answered, annotated field by field. They live in the stage's own file,
are recaptured with `kafkatest --capture-examples examples/captured.json --broker
apache_kafka`, and reach the site through `catalog.json`. See README.md, "Examples".

## A. Bootstrap & framing

- [ ] **Stage 01** — Bind to the broker port (`src/stages/s01_bind.rs`, 6 tests)
  - Create a TCP listener on 0.0.0.0:9092 and accept connections in a loop
  - Set SO_REUSEADDR so a restart does not hit 'address already in use'
  - Say nothing until a request arrives: a client speaks first
  - Accept the next connection even when the previous one is still open
- [ ] **Stage 02** — Respond with the correlation id (`src/stages/s02_correlation_id.rs`, 6 tests)
  - Read the 4-byte big-endian message size, then that many bytes
  - The request header starts api_key(int16) api_version(int16) correlation_id(int32)
  - Write back size(int32) then the same correlation id, unchanged, big-endian
  - The correlation id is signed: do not clamp, mask or renumber it
- [ ] **Stage 03** — Parse the request header (`src/stages/s03_request_header.rs`, 7 tests)
  - Header v2: api_key(int16) api_version(int16) correlation_id(int32) client_id(nullable string: int16 length, -1 = null) then tagged fields
  - client_id is a plain (non-compact) string even in flexible versions
  - Tagged fields are a uvarint count followed by (uvarint tag, uvarint length, bytes)
  - Skip tagged fields you do not know instead of rejecting the request
- [ ] **Stage 04** — UNSUPPORTED_VERSION (35) for a bad ApiVersions version (`src/stages/s04_unsupported_version.rs`, 6 tests)
  - Check api_version before decoding the body; ApiVersions is valid for v0-v4
  - Answer correlation_id(int32) then error_code(int16) = 35
  - The error response for ApiVersions always uses the v0 header (no tagged fields)
  - Keep the connection open afterwards: one bad request is not a fatal error
- [ ] **Stage 05** — ApiVersions v4 response body (`src/stages/s05_api_versions_body.rs`, 8 tests)
  - v4 body: error_code(int16), api_keys(COMPACT_ARRAY), throttle_time_ms(int32), tagged fields
  - A compact array is uvarint(count + 1) followed by the entries
  - Each entry is api_key(int16) min_version(int16) max_version(int16) + tagged fields
  - Advertise ApiVersions(18) itself with max_version >= 4
- [ ] **Stage 06** — Sequential requests on one connection (`src/stages/s06_sequential_requests.rs`, 5 tests)
  - Loop inside the connection handler instead of closing after one response
  - Read exactly message_size bytes each time; never assume one read = one request
  - Flush the response before reading the next request
  - Treat read() returning 0 as the client closing, not as an error
- [ ] **Stage 07** — Concurrent connections (`src/stages/s07_concurrent_connections.rs`, 5 tests)
  - Handle each accepted socket in its own thread, task or poll loop
  - Never let one slow client block the accept loop
  - Connection state (buffers, correlation ids) must be per connection, not global
  - Clean up when a client goes away so file descriptors are not leaked
- [ ] **Stage 08** — Pipelined requests **[ext]** (`src/stages/s08_pipelined_requests.rs`, 5 tests)
  - A client may write several frames before reading any response
  - Buffer incoming bytes: one read() can hold two requests, or half of one
  - Answer in the order the requests arrived — Kafka guarantees per-connection order
  - Do not start reading a new frame until message_size bytes of the current one are in
- [ ] **Stage 09** — Framing robustness **[ext]** (`src/stages/s09_framing_robustness.rs`, 8 tests)
  - Validate message_size before allocating: reject anything absurd instead of reserving gigabytes
  - A truncated frame is a closed connection, never a crashed process
  - One bad connection must not take down the accept loop or the other clients
  - A half-closed socket (client shut down its write side) is a normal end of stream

## B. Metadata & topics

- [ ] **Stage 10** — ApiVersions advertises DescribeTopicPartitions(75) (`src/stages/s10_advertise_describe_topic_partitions.rs`, 4 tests)
  - Add an entry with api_key 75, min_version 0 and max_version 0 to the api_keys array
  - The array is compact: its length byte is count + 1, so two entries encode as 0x03
  - Every entry ends with its own empty tagged-field section (a single 0x00)
- [ ] **Stage 11** — DescribeTopicPartitions: unknown topic (`src/stages/s11_describe_unknown_topic.rs`, 7 tests)
  - Look the name up in the metadata you loaded; a miss is not an error for the connection, only for that topic entry
  - error_code 3 (UNKNOWN_TOPIC_OR_PARTITION), topic_id all-zero, partitions empty
  - The response topic name echoes the requested name, so the client can match entries
  - next_cursor is null (0xff) when there is nothing left to page through
- [ ] **Stage 12** — DescribeTopicPartitions: a single-partition topic (`src/stages/s12_describe_single_partition.rs`, 8 tests)
  - Read __cluster_metadata-0/00000000000000000000.log at startup and index TopicRecord (type 2) and PartitionRecord (type 3) by name
  - The topic id comes from the TopicRecord, not from the topic name
  - Each partition entry carries error_code, partition_index, leader_id, leader_epoch, replica_nodes, isr_nodes and three more arrays
  - error_code is 0 for a topic that exists, even when the partition is empty
- [ ] **Stage 13** — DescribeTopicPartitions: multiple partitions (`src/stages/s13_describe_multiple_partitions.rs`, 6 tests)
  - One PartitionRecord per partition: collect them all before answering
  - Partitions come back sorted by partition_index, not in metadata-log order
  - replica_nodes, isr_nodes, eligible_leader_replicas, last_known_elr and offline_replicas are five separate compact arrays
  - A topic with N partitions still has exactly one topic entry in the response
- [ ] **Stage 14** — DescribeTopicPartitions: multiple topics (`src/stages/s14_describe_multiple_topics.rs`, 7 tests)
  - The request's topics array is a compact array of {name, tagged fields}
  - Answer one entry per requested topic, in order of topic name
  - Unknown names sit next to known ones: each entry carries its own error_code
  - Keep the per-topic tagged-field byte after topic_authorized_operations
- [ ] **Stage 15** — DescribeTopicPartitions: partition limit and cursor **[ext]** (`src/stages/s15_describe_pagination.rs`, 7 tests)
  - response_partition_limit counts partitions across the whole response, not per topic
  - When the budget runs out, next_cursor is {topic_name, partition_index} of the first partition that did not fit; otherwise it is a null (0xff) compact struct
  - A request carrying a cursor resumes at that topic and partition and skips every topic sorted before it
  - Walk topics in name order so paging is stable between requests
- [ ] **Stage 16** — Metadata (3) v12: brokers, controller and topic ids **[ext]** (`src/stages/s16_metadata_v12.rs`, 8 tests)
  - v12 is flexible: compact arrays and strings, a tagged-field byte after every struct, and a nullable cluster_id
  - brokers[] must advertise a host and port a client can really connect to, and controller_id must be one of those node ids
  - A null topics array means 'every topic'; an empty array means 'no topics'
  - An unknown topic is error 3 inside its own entry with a zero topic_id, and allow_auto_topic_creation=false must never create it
- [ ] **Stage 17** — CreateTopics (19) v7 **[ext]** (`src/stages/s17_create_topics.rs`, 8 tests)
  - Mint a fresh topic id, write a TopicRecord and one PartitionRecord per partition, then answer with that id, num_partitions and replication_factor
  - A duplicate name is error 36 TOPIC_ALREADY_EXISTS in that topic's entry only
  - An illegal name (empty, '.', '..', >249 chars, anything outside [a-zA-Z0-9._-]) is error 17, and num_partitions=0 is error 37 INVALID_PARTITIONS
  - validate_only=true runs every check and changes nothing
- [ ] **Stage 18** — DeleteTopics (20) v6 and the describe that follows **[ext]** (`src/stages/s18_delete_topics.rs`, 7 tests)
  - v6 takes {name, topic_id} structs: resolve the name to an id, then write a RemoveTopicRecord keyed by that id
  - Deleting an unknown name is error 3, deleting an unknown id is error 100 — the request as a whole still succeeds
  - After the delete, DescribeTopicPartitions for the name must answer error 3 again and the partition directories must be gone
  - Re-creating the same name mints a NEW topic id; never reuse the old one

## C. Fetch

- [ ] **Stage 19** — ApiVersions advertises Fetch(1) v16 (`src/stages/s19_advertise_fetch.rs`, 4 tests)
  - Add {api_key: 1, min_version: 0, max_version: 16} to the api_keys array
  - Fetch v12 and later are flexible versions: the request and response use compact arrays and tagged fields
  - From v13 the request names topics by topic id, not by name
- [ ] **Stage 20** — Fetch with no topics (`src/stages/s20_fetch_no_topics.rs`, 5 tests)
  - v16 response: throttle_time_ms(int32), error_code(int16), session_id(int32), responses(COMPACT_ARRAY), tagged fields
  - An empty compact array is the single byte 0x01 (count + 1)
  - Answer immediately when there is nothing to wait for, whatever max_wait_ms says
- [ ] **Stage 21** — Fetch an unknown topic id → UNKNOWN_TOPIC_ID (100) (`src/stages/s21_fetch_unknown_topic_id.rs`, 6 tests)
  - The top-level error_code stays 0: the failure belongs to the partition entry
  - Per partition: error_code 100, high_watermark 0, records null (0xff as a compact nullable bytes length)
  - Echo the topic id you were given; there is no name to fall back on in v13+
- [ ] **Stage 22** — Fetch an empty topic (`src/stages/s22_fetch_empty_topic.rs`, 7 tests)
  - A topic that exists but holds nothing is error_code 0, not 3 or 100
  - high_watermark and log_start_offset are both 0 for an empty partition
  - records may be null or a zero-length compact bytes field; both mean 'nothing'
  - The partition file <topic>-<n>/00000000000000000000.log may be zero bytes long
- [ ] **Stage 23** — Fetch a single record from disk (`src/stages/s23_fetch_single_record.rs`, 7 tests)
  - Read <topic>-<partition>/00000000000000000000.log and send the batch bytes straight through: no re-encoding is needed
  - records is a compact nullable bytes field: uvarint(length + 1) then the bytes
  - high_watermark is the offset after the last record, so 1 for a single record
  - The batch header already carries baseOffset, lastOffsetDelta and the CRC-32C
- [ ] **Stage 24** — Fetch multiple batches and partitions (`src/stages/s24_fetch_multiple_batches.rs`, 6 tests)
  - A partition's records field holds every batch, concatenated, not one batch
  - Offsets continue across batches: batch 2's baseOffset is batch 1's baseOffset + count
  - high_watermark is the offset after the last record of the partition
  - Each requested partition gets its own entry, in the order it was asked for
- [ ] **Stage 25** — Fetch from the middle, and OFFSET_OUT_OF_RANGE (1) **[ext]** (`src/stages/s25_fetch_from_offset.rs`, 7 tests)
  - A fetch offset inside the log starts at the first batch whose last offset is >= it, so the response may begin before the offset that was asked for
  - Never split a batch: the client drops the records below its fetch offset
  - fetch_offset == high_watermark is not an error — answer with error 0, no records and the same high watermark
  - fetch_offset above the high watermark or below log_start_offset (a negative offset included) is error 1 OFFSET_OUT_OF_RANGE
- [ ] **Stage 26** — max_bytes and partition_max_bytes truncation **[ext]** (`src/stages/s26_fetch_max_bytes.rs`, 7 tests)
  - Fill a partition with whole batches until the next one would pass partition_max_bytes, then stop — never send half a batch you chose to send
  - The first batch is exempt: a batch larger than the limit still goes out in full, or a consumer whose limit is too small never makes progress
  - The top-level max_bytes is the budget for the whole response; subtract what each partition used and give the rest to the next one
  - Only the very first partition of the response gets the 'at least one batch' exemption; once the budget is gone the others come back empty
- [ ] **Stage 27** — Compressed batches: gzip, snappy, lz4, zstd **[ext]** (`src/stages/s27_compressed_batches.rs`, 7 tests)
  - The codec lives in attribute bits 0-2 of the batch header: 1 gzip, 2 snappy, 3 lz4, 4 zstd — everything from recordCount onwards is the compressed blob
  - Never recompress or re-encode on the fetch path: copy the stored bytes out, attribute bits and all, and the CRC still verifies
  - The record count in the header counts the uncompressed records, so you do not have to decompress anything to serve a fetch
  - Kafka's framings are specific: snappy is xerial-framed, lz4 is the LZ4 frame format, gzip and zstd are plain streams
- [ ] **Stage 28** — Long poll: max_wait_ms and min_bytes **[ext]** (`src/stages/s28_long_poll.rs`, 7 tests)
  - Park a fetch that cannot be satisfied instead of answering it: hold it until min_bytes of records exist or max_wait_ms has passed, then answer with what there is (possibly nothing) and error 0
  - An append must complete the parked fetch straight away — poll or notify, but do not make the consumer wait out the full max_wait_ms for data already there
  - max_wait_ms 0, or min_bytes 0, means answer now; a parked fetch must never block the other connections or the accept loop
  - The tests allow the answer to be up to 100 ms early and 1500 ms late, so a sleep-based implementation is fine, but busy-waiting a whole core is not

## D. Produce

- [ ] **Stage 29** — ApiVersions advertises Produce(0) v11 (`src/stages/s29_advertise_produce.rs`, 4 tests)
  - Produce is api_key 0, which is easy to miss when you index the array by key
  - Add {api_key: 0, min_version: 0, max_version: 11}
  - Produce v9 and later are flexible; v11 still names topics by name, not id
- [ ] **Stage 30** — Produce to an unknown topic → error 3 (`src/stages/s30_produce_unknown_topic.rs`, 6 tests)
  - Check the topic exists before touching the log; do not create it implicitly
  - The error lives in the per-partition entry, not at the top level
  - Answer error_code 3, base_offset -1, log_append_time_ms -1, log_start_offset -1
  - Echo the topic name so the producer can match the entry
- [ ] **Stage 31** — Produce one record (`src/stages/s31_produce_one_record.rs`, 7 tests)
  - Append the batch bytes to <topic>-<partition>/00000000000000000000.log as they are
  - Rewrite only baseOffset, to the current end of the log; the CRC covers the bytes after it, so it stays valid
  - Answer error_code 0, base_offset = the offset the first record landed on, log_append_time_ms -1 (the records keep their CreateTime)
- [ ] **Stage 32** — Produce multiple records, partitions and topics (`src/stages/s32_produce_many.rs`, 6 tests)
  - One Produce request can carry several topics, each with several partitions
  - base_offset is the offset of the *first* record of the batch; the rest follow it
  - Every partition has its own log and its own offsets — never share a counter
  - Answer one entry per requested partition, in the order they were sent
- [ ] **Stage 33** — acks 0, 1 and -1 **[ext]** (`src/stages/s33_produce_acks.rs`, 7 tests)
  - acks=0 means no response at all: append the records and write nothing back, but keep reading the connection — the next request still needs an answer
  - acks=1 answers once the leader's own log has the batch, acks=-1 once every in-sync replica does; with one broker both answer the same base_offset
  - Any other value (2, 3, -2, ...) is error 21 INVALID_REQUIRED_ACKS, one entry per partition, and the request is not appended
  - Check acks before you append, not after: an invalid request must not change the log
- [ ] **Stage 34** — Produced records are persisted on disk **[ext]** (`src/stages/s34_produce_persisted.rs`, 7 tests)
  - Append the batch to <log.dirs>/<topic>-<partition>/00000000000000000000.log byte for byte; only baseOffset is rewritten, and the CRC-32C does not cover it
  - A second Produce appends a second batch after the first — a segment is a concatenation of batches, never a rewrite
  - Create the empty 00000000000000000000.index and .timeindex next to the segment; real clients and kafka-dump-log.sh expect them to be there
  - kafka-dump-log.sh --deep-iteration over your segment is the ground truth
- [ ] **Stage 35** — Produce to fetch round trip across restarts **[ext]** (`src/stages/s35_produce_fetch_restart.rs`, 6 tests)
  - A Fetch must hand back what Produce was given: keys, values, headers and CreateTime timestamps, in offset order
  - At startup, read the last segment of every partition to recover its end offset; offsets continue from there, they never reset to 0
  - The high watermark comes back from the log too, and the log start offset stays 0 until something is deleted
  - Only baseOffset and partitionLeaderEpoch are the broker's to rewrite; the record bytes inside the batch belong to the producer
- [ ] **Stage 36** — Idempotent producer: InitProducerId (22) and sequences **[ext]** (`src/stages/s36_idempotent_producer.rs`, 7 tests)
  - InitProducerId with a null transactional_id hands out a fresh producer id (>= 0) and epoch (>= 0); the producer stamps both into every batch header
  - Keep the last sequence number per (producer id, partition): the next batch must start at last + 1, and its last sequence is baseSequence + lastOffsetDelta
  - A batch that repeats a sequence already appended is a retry — return the offset it got the first time (real Kafka) or error 46 DUPLICATE_SEQUENCE_NUMBER, and append nothing
  - A gap is error 45 OUT_OF_ORDER_SEQUENCE_NUMBER; an older epoch is error 47 INVALID_PRODUCER_EPOCH (45 is accepted too if you check the sequence first) — either way the log is left untouched
- [ ] **Stage 37** — Record validation: CORRUPT_MESSAGE (2) and MESSAGE_TOO_LARGE (10) **[ext]** (`src/stages/s37_record_validation.rs`, 7 tests)
  - Recompute the CRC-32C over everything after the crc field and compare: a mismatch is error 2 CORRUPT_MESSAGE and nothing is appended
  - A recordCount that does not match the records, or a magic other than 2, is rejected too: real Kafka answers 87 INVALID_RECORD there, and 2 CORRUPT_MESSAGE is accepted as well
  - A batch bigger than message.max.bytes from the properties file is error 10 MESSAGE_TOO_LARGE, per partition
  - Every one of these is a per-partition error_code in a normal response: never close the connection, never stop the accept loop

## E. Offsets & consumer groups

- [ ] **Stage 38** — ListOffsets (2): earliest, latest, by timestamp **[ext]** (`src/stages/s38_list_offsets.rs`, 8 tests)
  - timestamp -2 means the log start offset, -1 means the high watermark; neither reads a record
  - Any other timestamp returns the first offset whose record timestamp is >= it, and that record's timestamp; past the end it is offset -1, timestamp -1
  - Answer per partition: error_code, timestamp, offset, leader_epoch — earliest and latest report timestamp -1
  - An unknown topic or partition is error 3 in that partition's entry, not a top-level failure
- [ ] **Stage 39** — FindCoordinator (10) for a consumer group **[ext]** (`src/stages/s39_find_coordinator.rs`, 8 tests)
  - v4 takes coordinator_keys[] and answers coordinators[], one entry per key, each with its own error_code — the top-level error_code stays 0
  - The coordinator of a group is the broker leading __consumer_offsets-(hash(group) % partitions); with one broker that is always this broker, but node_id, host and port must be the ones Metadata advertises
  - Kafka creates __consumer_offsets lazily, so the first lookup may answer 15 COORDINATOR_NOT_AVAILABLE for a few hundred ms; a client retries
  - key_type 0 is a group, 1 is a transactional id; an unimplemented key type is an error code, never a dropped connection
- [ ] **Stage 40** — JoinGroup, SyncGroup, Heartbeat, LeaveGroup **[ext]** (`src/stages/s40_consumer_group.rs`, 8 tests)
  - A join with an empty member_id gets 79 MEMBER_ID_REQUIRED plus a freshly minted member id (KIP-394); the client joins again with that id
  - The first member to join is the leader: its response carries members[] with every subscription, and only the leader sends assignments in SyncGroup
  - SyncGroup hands each member back the bytes the leader addressed to it, unchanged; the group is Stable only after the leader has synced
  - A second member makes the group rebalance: the existing member's Heartbeat turns into 27 REBALANCE_IN_PROGRESS and it must re-join to reach generation 2
- [ ] **Stage 41** — OffsetCommit (8) and OffsetFetch (9) **[ext]** (`src/stages/s41_offset_commit_fetch.rs`, 8 tests)
  - A commit with generation_id -1 and an empty member_id is a standalone commit: no group membership is needed, and the answer is error 0 per partition
  - Committed offsets live in __consumer_offsets keyed by (group, topic, partition); the metadata string is stored with the offset and comes back unchanged
  - OffsetFetch for a group or a partition that never committed is offset -1, metadata "" and error 0 — absence is not an error
  - A commit from a member with an out-of-date generation is 22 ILLEGAL_GENERATION; a commit for a topic that does not exist is 3
- [ ] **Stage 42** — Group errors: UNKNOWN_MEMBER_ID, REBALANCE_IN_PROGRESS, ILLEGAL_GENERATION **[ext]** (`src/stages/s42_group_errors.rs`, 8 tests)
  - Check the member id first: one the group does not hold is 25 UNKNOWN_MEMBER_ID, for Heartbeat, SyncGroup, JoinGroup and LeaveGroup alike
  - Then check the generation: a known member sending a stale generation gets 22 ILLEGAL_GENERATION, never 25
  - While the group is rebalancing every Heartbeat is 27 REBALANCE_IN_PROGRESS — that is how a consumer learns it has to re-join
  - LeaveGroup reports per member: the top-level error stays 0 and the bad member carries its own 25

## F. Interop, robustness, performance

- [ ] **Stage 43** — Interop with Kafka's own command line tools **[ext]** (`src/stages/s43_cli_interop.rs`, 6 tests)
  - kafka-topics.sh --describe exercises Metadata(3) and DescribeTopicPartitions(75) together; the tool needs every partition to name a live leader
  - kafka-console-producer.sh opens with ApiVersions and Metadata, then Produce; keep acks=1 and enable.idempotence=false working before you add InitProducerId(22)
  - kafka-console-consumer.sh needs FindCoordinator(10), the group protocol, ListOffsets(2) and Fetch(1) — the whole consumer path in one command
  - kafka-broker-api-versions.sh prints exactly what your ApiVersions response says, so a key advertised there and unimplemented here is a lie a real client believes
- [ ] **Stage 44** — Fuzz: 500 seeded mutated frames **[ext]** (`src/stages/s44_fuzz.rs`, 6 tests)
  - Bit flips, truncations, huge lengths and negative array sizes, all from --seed: validate every length against the bytes you actually have before you index
  - The broker must never crash, never hang and never leak memory — a bad frame is at worst a closed connection, and the accept loop keeps running
  - After the whole run it must still answer a clean ApiVersions request on a new connection within two seconds
  - Whatever you do send must be framed: a size prefix and then exactly that many bytes, never a prefix that promises more than you write
- [ ] **Stage 45** — Soak: 10 000 records and 1 000 connections **[ext]** (`src/stages/s45_soak.rs`, 6 tests)
  - Produce and fetch 10 000 records and compare them byte for byte; offsets inside a partition must be contiguous with no gap and no repeat
  - Open and close 1 000 connections without leaking file descriptors or threads — close the socket when the client goes away, not when the process exits
  - Fifty concurrent connections must not serialize behind one another, and per connection the responses still come back in request order
  - p50/p95/p99 latency is reported for information; the only hard limit is that each test finishes within 60 seconds

## G. Transactions [ext]

Stage 36 built the idempotent producer, which gets exactly-once within one producer
session. A transactional id is what makes the guarantee outlive the process — and the epoch
the coordinator hands out with it is what stops two instances of the same job both writing.

- [ ] **Stage 46** — The transaction coordinator and producer fencing **[ext]** (`src/stages/s46_transaction_coordinator.rs`, 10 tests)
  - `FindCoordinator` with `key_type` 1 asks for a *transaction* coordinator; the same request with 0 asks for a consumer group's, and the two are different lookups
  - `InitProducerId` with a transactional id is a different operation from the same request with a null one: it is coordinator state that outlives the connection
  - Asking twice for the same transactional id must return the same producer id with a higher epoch — that bump is the entire fencing mechanism
  - Fencing happens at the coordinator: a superseded instance resuming its session is refused with INVALID_PRODUCER_EPOCH (47) or PRODUCER_FENCED (90), while the live instance presenting the epoch it holds is let through
- [ ] **Stage 47** — A transaction, end to end **[ext]** (`src/stages/s47_transactional_writes.rs`, 9 tests)
  - `AddPartitionsToTxn` (24) comes before the first write to a partition: the coordinator must know where to put markers if the transaction aborts
  - A transactional batch sets bit 4 of the record batch attributes as well as carrying the producer id and epoch, and the Produce request names the transactional id too
  - `EndTxn` (26) with `committed` true or false makes the coordinator write a control record into every partition the transaction touched — a real record at a real offset
  - A consumer at isolation_level 1 reads nothing past the last stable offset and is given the aborted transactions to filter; at level 0 it sees everything immediately
