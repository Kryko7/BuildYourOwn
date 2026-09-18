/**
 * Placeholder kafka catalog, transcribed from BuildYourOwn/PLAN.md section 1.5.
 * Used only until ../kafkatest/catalog.json exists; the sync script always
 * prefers the real file. Every stage here is marked pending (no test list yet).
 */

const SECTIONS = [
	['A', 'Bootstrap & framing'],
	['B', 'Metadata & topics'],
	['C', 'Fetch'],
	['D', 'Produce'],
	['E', 'Offsets & consumer groups'],
	['F', 'Interop, robustness, performance']
];

// [number, section, name, ext, hints...]
const STAGES = [
	[1, 'A', 'Bind to port 9092', false,
		'Listen on 0.0.0.0:9092 and accept; send nothing until a request arrives',
		'A bare TCP connect must succeed and stay open — no greeting, no banner'],
	[2, 'A', 'Respond with the correlation id', false,
		'Read the 4-byte big-endian size prefix, then the header; echo correlation_id back',
		'The response is also length-prefixed: 4-byte size + 4-byte correlation id'],
	[3, 'A', 'Parse the request header', false,
		'Header v2: api_key(i16) api_version(i16) correlation_id(i32) client_id(nullable string) tagged fields',
		'client_id uses the legacy (non-compact) nullable string even in flexible headers',
		'Skip an empty tagged-field buffer with a single 0x00 varint'],
	[4, 'A', 'UNSUPPORTED_VERSION (35)', false,
		'Validate api_version before decoding the body; ApiVersions accepts v0-v4',
		'Error responses for ApiVersions use header v0 — correlation id only, no tagged fields'],
	[5, 'A', 'ApiVersions v4 response body', false,
		'Body: error_code, compact array of {api_key, min_version, max_version, tagged}, throttle_time_ms, tagged',
		'Compact arrays are length+1 as an unsigned varint; an empty array is 0x01',
		'You must advertise ApiVersions(18) with min <= 4 <= max'],
	[6, 'A', 'Sequential requests on one connection', false,
		'Loop on the socket instead of closing after one response',
		'Read exactly size bytes per request; never assume one read() is one frame'],
	[7, 'A', 'Concurrent connections', false,
		'One task/thread per connection, or an event loop; state must not leak between clients',
		'Each connection has its own correlation id sequence'],
	[8, 'A', 'Pipelined requests', true,
		'Several frames can arrive in one TCP segment; responses must come back in order',
		'Do not wait for a write to drain before parsing the next frame'],
	[9, 'A', 'Framing robustness', true,
		'Partial frames, zero length, absurd sizes, garbage bytes and half-close must not crash the broker',
		'Cap the accepted frame size and drop the connection rather than allocating it',
		'Other connections must keep being served while one misbehaves'],

	[10, 'B', 'Advertise DescribeTopicPartitions(75) v0', false,
		'Add api_key 75 with min_version 0 and max_version 0 to the ApiVersions array',
		'Keep the array sorted-ish and complete: clients look up by key, not position'],
	[11, 'B', 'Unknown topic -> error 3', false,
		'Respond UNKNOWN_TOPIC_OR_PARTITION (3) with topic_id = 00000000-0000-0000-0000-000000000000',
		'next_cursor is a nullable field: write 0xff for null'],
	[12, 'B', 'Single partition topic', false,
		'Read the topic uuid and partition metadata out of __cluster_metadata-0',
		'Partition record fields: leader id, leader epoch, replicas, ISR'],
	[13, 'B', 'Multiple partitions', false,
		'Partitions are returned in index order with their own replica/ISR arrays',
		'Every array in this response is a compact array'],
	[14, 'B', 'Multiple topics in one request', false,
		'The response is ordered by topic name, not by request order',
		'Unknown and known topics can be mixed in one request'],
	[15, 'B', 'Partition limit & cursor pagination', true,
		'response_partition_limit caps the partitions in one response',
		'Return a next_cursor of {topic_name, partition_index} when you truncate'],
	[16, 'B', 'Metadata (3) v12', true,
		'Brokers, controller id, topics with their uuids; flexible version, so compact everything',
		'topic_id matters: modern clients fetch by id, not name'],
	[17, 'B', 'CreateTopics (19)', true,
		'Append TopicRecord + PartitionRecord to the metadata log, then create the log dirs',
		'Duplicate name -> TOPIC_ALREADY_EXISTS (36); validate the name charset and length'],
	[18, 'B', 'DeleteTopics (20)', true,
		'RemoveTopicRecord in the metadata log; describe afterwards must say unknown',
		'Delete by topic id, and handle the unknown-id case'],

	[19, 'C', 'Advertise Fetch(1) v16', false,
		'api_key 1 with max_version >= 16 in ApiVersions',
		'Fetch v16 is flexible and identifies topics by uuid, not name'],
	[20, 'C', 'Fetch with no topics', false,
		'Empty responses: throttle_time_ms, error_code 0, session_id, empty compact topic array'],
	[21, 'C', 'Unknown topic id -> 100', false,
		'UNKNOWN_TOPIC_ID (100) is per-partition, not per-response',
		'Echo the requested topic_id back in the response'],
	[22, 'C', 'Fetch an empty topic', false,
		'records is a nullable compact bytes field; high_watermark and last_stable_offset are 0'],
	[23, 'C', 'Fetch a single record from disk', false,
		'Send the RecordBatch bytes from the segment file verbatim — do not re-encode',
		'The batch header carries base_offset, batch_length, CRC32C and the record count'],
	[24, 'C', 'Multiple batches and partitions', false,
		'Concatenate whole batches; never split one',
		'high_watermark advances past the last committed offset'],
	[25, 'C', 'Offsets and OFFSET_OUT_OF_RANGE', true,
		'Find the batch containing fetch_offset via the .index file or a scan',
		'Past the high watermark -> OFFSET_OUT_OF_RANGE (1); before log start -> the same'],
	[26, 'C', 'max_bytes truncation', true,
		'Honour partition_max_bytes and the request max_bytes, but always return at least one batch',
		'Truncate at a batch boundary so clients can always make progress'],
	[27, 'C', 'Compressed batches', true,
		'gzip/snappy/lz4/zstd live in the batch attributes low three bits',
		'The broker passes compressed batches through untouched on fetch'],
	[28, 'C', 'Long poll: max_wait_ms / min_bytes', true,
		'Park the fetch until min_bytes are available or max_wait_ms elapses',
		'A produce to that partition must wake the parked fetch immediately'],

	[29, 'D', 'Advertise Produce(0) v11', false,
		'api_key 0 with max_version >= 11; v9+ is flexible'],
	[30, 'D', 'Produce to an unknown topic', false,
		'UNKNOWN_TOPIC_OR_PARTITION (3) per partition, base_offset -1'],
	[31, 'D', 'Produce one record', false,
		'Append the batch to the segment, answer base_offset 0, log_append_time -1, error 0',
		'Rewrite base_offset in the stored batch to the assigned offset'],
	[32, 'D', 'Multiple records, partitions, topics', false,
		'One response entry per partition, in request order',
		'Offsets are assigned per partition, independently'],
	[33, 'D', 'acks 0, 1 and -1', true,
		'acks=0 means write nothing back at all — not an empty response',
		'acks=-1 waits for the ISR; with one replica that is the same as acks=1'],
	[34, 'D', 'Persisted on disk', true,
		'The segment must be readable by kafka-dump-log.sh: correct CRC32C over the batch body',
		'Offsets inside the batch are deltas from base_offset'],
	[35, 'D', 'Produce -> Fetch round trip', true,
		'Bytes fetched back must equal the bytes produced',
		'Offsets continue after a broker restart: recover the log end offset from the segment'],
	[36, 'D', 'Idempotent producer', true,
		'InitProducerId (22) hands out a producer id and epoch',
		'Track the last 5 sequence numbers per producer/partition',
		'Replay -> DUPLICATE_SEQUENCE_NUMBER; gap -> OUT_OF_ORDER_SEQUENCE_NUMBER'],
	[37, 'D', 'Record validation', true,
		'Recompute CRC32C on ingest: mismatch -> CORRUPT_MESSAGE (2)',
		'Oversized batch -> MESSAGE_TOO_LARGE (10) without writing anything'],

	[38, 'E', 'ListOffsets (2)', true,
		'timestamp -2 = earliest, -1 = latest, anything else = first offset at or after it',
		'The answer for an empty partition is the log start offset'],
	[39, 'E', 'FindCoordinator (10)', true,
		'Hash the group id to a __consumer_offsets partition and return that partition leader',
		'With one broker the coordinator is always you — but the fields still have to be right'],
	[40, 'E', 'JoinGroup / SyncGroup / Heartbeat / LeaveGroup', true,
		'First member becomes the leader and receives every members metadata',
		'generation_id increases on every rebalance; stale generations are rejected',
		'A second member joining forces a rebalance of the first'],
	[41, 'E', 'OffsetCommit (8) / OffsetFetch (9)', true,
		'Committed offsets are per group/topic/partition; unknown -> -1',
		'Commits are just records in __consumer_offsets'],
	[42, 'E', 'Group error codes', true,
		'UNKNOWN_MEMBER_ID (25), REBALANCE_IN_PROGRESS (27), ILLEGAL_GENERATION (22)',
		'Clients rely on these to drive their state machine — the code matters more than the message'],

	[43, 'F', 'Real client interop', true,
		'kafka-console-producer.sh / -consumer.sh / kafka-topics.sh --describe must work',
		'Real clients send Metadata, ApiVersions and Fetch versions you did not plan for'],
	[44, 'F', 'Fuzz', true,
		'500 seeded mutated frames: bit flips, truncation, huge lengths, negative array sizes',
		'Never panic, never hang; still answer ApiVersions afterwards'],
	[45, 'F', 'Soak', true,
		'10 000 records produced and fetched back byte-identical',
		'1 000 connect/close cycles with no fd or memory leak',
		'p99 latency is informational; the whole run must finish inside 60 s']
];

function slugify(name) {
	return name
		.toLowerCase()
		.replace(/\([^)]*\)/g, ' ')
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-|-$/g, '')
		.slice(0, 48);
}

/** Same shape as parsePlan()'s return value, so buildCatalog can consume it. */
export function kafkaPlaceholderPlan() {
	const sections = SECTIONS.map(([id, title]) => ({ id, title, stages: [] }));
	const byId = Object.fromEntries(sections.map((s) => [s.id, s]));
	const stages = STAGES.map(([number, section, name, ext, ...hints]) => {
		byId[section].stages.push(number);
		return {
			number,
			slug: slugify(name),
			name,
			ext,
			file: `stages/s${String(number).padStart(2, '0')}_${slugify(name).replace(/-/g, '_')}.rs`,
			planDone: false,
			plannedTests: 0,
			section,
			hints,
			tests: []
		};
	});
	return { sections, stages, declared: { stages: stages.length, tests: 0 } };
}
