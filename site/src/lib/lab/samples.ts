/** Four realistic Kafka requests, built from the codec so the annotated decode always lines up. */
import { Writer } from './kafka';
import { encodeRecordBatch } from './recordbatch';

export interface Sample {
	id: string;
	label: string;
	api: string;
	stages: number[];
	blurb: string;
	bytes: Uint8Array;
}

function header(apiKey: number, apiVersion: number, correlationId: number, clientId: string | null, flexible: boolean) {
	const w = new Writer();
	w.i16(apiKey).i16(apiVersion).i32(correlationId).legacyString(clientId);
	if (flexible) w.taggedFields();
	return w;
}

function apiVersionsV4(): Uint8Array {
	const w = header(18, 4, 7, 'kafka-cli', true);
	w.compactString('kafka-cli');
	w.compactString('4.1.2');
	w.taggedFields();
	return w.framed();
}

function describeTopicPartitionsV0(): Uint8Array {
	const w = header(75, 0, 42, 'adminclient-1', true);
	w.compactArrayLen(1);
	w.compactString('orders-log');
	w.taggedFields();
	w.i32(100); // response_partition_limit
	w.i8(0xff); // cursor = null
	w.taggedFields();
	return w.framed();
}

function fetchV16(): Uint8Array {
	const w = header(1, 16, 1001, 'consumer-1', true);
	w.i32(500); // max_wait_ms
	w.i32(1); // min_bytes
	w.i32(52428800); // max_bytes
	w.i8(0); // isolation_level
	w.i32(0); // session_id
	w.i32(0); // session_epoch
	w.compactArrayLen(1);
	w.uuid('00000000-0000-4000-8000-00000000000a');
	w.compactArrayLen(1);
	w.i32(0); // partition
	w.i32(-1); // current_leader_epoch
	w.i64(0n); // fetch_offset
	w.i32(-1); // last_fetched_epoch
	w.i64(0n); // log_start_offset
	w.i32(1048576); // partition_max_bytes
	w.taggedFields();
	w.taggedFields();
	w.compactArrayLen(0); // forgotten_topics_data
	w.compactString(''); // rack_id
	w.taggedFields();
	return w.framed();
}

function produceV11(): Uint8Array {
	const batch = encodeRecordBatch([
		{ key: null, value: 'hello world', headers: [{ key: 'source', value: 'lab' }] },
		{ key: 'k2', value: 'second record' }
	]);
	const w = header(0, 11, 2001, 'producer-1', true);
	w.compactString(null); // transactional_id
	w.i16(-1); // acks = all
	w.i32(1500); // timeout_ms
	w.compactArrayLen(1);
	w.compactString('orders-log');
	w.compactArrayLen(1);
	w.i32(0); // partition index
	w.compactBytes(batch.bytes);
	w.taggedFields();
	w.taggedFields();
	w.taggedFields();
	return w.framed();
}

export const samples: Sample[] = [
	{
		id: 'apiversions-v4',
		label: 'ApiVersions v4',
		api: 'ApiVersions(18) v4',
		stages: [3, 4, 5, 10, 19, 29],
		blurb:
			'The handshake every client opens with. Note the header: client_id is still a legacy int16-prefixed string, and only the body uses compact strings.',
		bytes: apiVersionsV4()
	},
	{
		id: 'describetopicpartitions-v0',
		label: 'DescribeTopicPartitions v0',
		api: 'DescribeTopicPartitions(75) v0',
		stages: [10, 11, 12, 13, 14, 15],
		blurb:
			'One topic name, a partition limit and a null cursor. The 0xff is the null cursor — not a length, not a tag.',
		bytes: describeTopicPartitionsV0()
	},
	{
		id: 'fetch-v16',
		label: 'Fetch v16',
		api: 'Fetch(1) v16',
		stages: [19, 20, 21, 22, 23, 24, 25, 26, 28],
		blurb:
			'Topics are addressed by uuid here, not by name — which is why stage 21 answers UNKNOWN_TOPIC_ID (100) rather than error 3.',
		bytes: fetchV16()
	},
	{
		id: 'produce-v11',
		label: 'Produce v11',
		api: 'Produce(0) v11',
		stages: [29, 30, 31, 32, 33, 34, 37],
		blurb:
			'Two records inside one RecordBatch v2, nested in a compact bytes field. The CRC32C is computed live below.',
		bytes: produceV11()
	}
];

export function sampleById(id: string): Sample | undefined {
	return samples.find((s) => s.id === id);
}
