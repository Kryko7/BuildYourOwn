//! Field-by-field schemas for the data-plane APIs: Fetch(1), Produce(0), ListOffsets(2)
//! and InitProducerId(22).
//!
//! `records` fields are handed to [`Walk::record_batches`], which annotates every batch
//! header and the first record of the first batch.

use super::annotate::Walk;

// ---------------------------------------------------------------------------------------
// Fetch (1)
// ---------------------------------------------------------------------------------------

/// `FetchRequest` v12-v17. From v15 on `replica_id` lives in a tagged field, so the body
/// starts at `max_wait_ms`.
pub fn fetch_request(w: &mut Walk<'_>, version: i16) {
    if version < 15 {
        w.i32("replica_id");
    }
    w.i32("max_wait_ms");
    w.i32("min_bytes");
    if version >= 3 {
        w.i32("max_bytes");
    }
    if version >= 4 {
        w.i8("isolation_level");
    }
    if version >= 7 {
        w.i32("session_id");
        w.i32("session_epoch");
    }
    w.compact_array("topics", |w, _| {
        if version >= 13 {
            w.uuid("topic_id");
        } else {
            w.compact_string("topic");
        }
        w.compact_array("partitions", |w, _| {
            w.i32("partition");
            if version >= 9 {
                w.i32("current_leader_epoch");
            }
            w.i64("fetch_offset");
            if version >= 12 {
                w.i32("last_fetched_epoch");
            }
            if version >= 5 {
                w.i64("log_start_offset");
            }
            w.i32("partition_max_bytes");
            w.tags("tagged_fields");
        });
        w.tags("tagged_fields");
    });
    if version >= 7 {
        w.compact_array("forgotten_topics_data", |w, _| {
            if version >= 13 {
                w.uuid("topic_id");
            } else {
                w.compact_string("topic");
            }
            w.compact_array("partitions", |w, _| {
                w.i32("");
            });
            w.tags("tagged_fields");
        });
    }
    if version >= 11 {
        w.compact_string("rack_id");
    }
    w.tags("tagged_fields");
}

/// `FetchResponse` v12-v17.
pub fn fetch_response(w: &mut Walk<'_>, version: i16) {
    w.i32("throttle_time_ms");
    if version >= 7 {
        w.error_code("error_code");
        w.i32("session_id");
    }
    w.compact_array("responses", |w, _| {
        if version >= 13 {
            w.uuid("topic_id");
        } else {
            w.compact_string("topic");
        }
        w.compact_array("partitions", |w, _| {
            w.i32("partition_index");
            w.error_code("error_code");
            w.i64("high_watermark");
            if version >= 4 {
                w.i64("last_stable_offset");
            }
            if version >= 5 {
                w.i64("log_start_offset");
            }
            if version >= 4 {
                w.compact_array("aborted_transactions", |w, _| {
                    w.i64("producer_id");
                    w.i64("first_offset");
                    w.tags("tagged_fields");
                });
            }
            if version >= 11 {
                w.i32("preferred_read_replica");
            }
            records(w, "records");
            w.tags("tagged_fields");
        });
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}

/// A `records` field: compact nullable bytes holding one or more v2 record batches.
fn records(w: &mut Walk<'_>, name: &str) {
    if let Some((start, len)) = w.compact_bytes(name) {
        w.nested(name, |w| w.record_batches("batch", start, len));
    }
}

// ---------------------------------------------------------------------------------------
// Produce (0)
// ---------------------------------------------------------------------------------------

/// `ProduceRequest` v9-v12.
pub fn produce_request(w: &mut Walk<'_>, version: i16) {
    if version >= 3 {
        w.compact_string("transactional_id");
    }
    w.i16("acks");
    w.i32("timeout_ms");
    w.compact_array("topic_data", |w, _| {
        w.compact_string("name");
        w.compact_array("partition_data", |w, _| {
            w.i32("index");
            records(w, "records");
            w.tags("tagged_fields");
        });
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}

/// `ProduceResponse` v9-v12. The array comes first and `throttle_time_ms` last, which is
/// the other way round from most responses.
pub fn produce_response(w: &mut Walk<'_>, version: i16) {
    w.compact_array("responses", |w, _| {
        w.compact_string("name");
        if version >= 13 {
            w.uuid("topic_id");
        }
        w.compact_array("partition_responses", |w, _| {
            w.i32("index");
            w.error_code("error_code");
            w.i64("base_offset");
            if version >= 2 {
                w.i64("log_append_time_ms");
            }
            if version >= 5 {
                w.i64("log_start_offset");
            }
            if version >= 8 {
                w.compact_array("record_errors", |w, _| {
                    w.i32("batch_index");
                    w.compact_string("batch_index_error_message");
                    w.tags("tagged_fields");
                });
                w.compact_string("error_message");
            }
            w.tags("tagged_fields");
        });
        w.tags("tagged_fields");
    });
    if version >= 1 {
        w.i32("throttle_time_ms");
    }
    w.tags("tagged_fields");
}

// ---------------------------------------------------------------------------------------
// ListOffsets (2)
// ---------------------------------------------------------------------------------------

/// `ListOffsetsRequest` v6-v9.
pub fn list_offsets_request(w: &mut Walk<'_>, version: i16) {
    w.i32("replica_id");
    if version >= 2 {
        w.i8("isolation_level");
    }
    w.compact_array("topics", |w, _| {
        w.compact_string("name");
        w.compact_array("partitions", |w, _| {
            w.i32("partition_index");
            if version >= 4 {
                w.i32("current_leader_epoch");
            }
            w.i64("timestamp");
            w.tags("tagged_fields");
        });
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}

/// `ListOffsetsResponse` v6-v9.
pub fn list_offsets_response(w: &mut Walk<'_>, version: i16) {
    if version >= 2 {
        w.i32("throttle_time_ms");
    }
    w.compact_array("topics", |w, _| {
        w.compact_string("name");
        w.compact_array("partitions", |w, _| {
            w.i32("partition_index");
            w.error_code("error_code");
            w.i64("timestamp");
            w.i64("offset");
            if version >= 4 {
                w.i32("leader_epoch");
            }
            w.tags("tagged_fields");
        });
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}

// ---------------------------------------------------------------------------------------
// InitProducerId (22)
// ---------------------------------------------------------------------------------------

/// `InitProducerIdRequest` v2-v5.
pub fn init_producer_id_request(w: &mut Walk<'_>, version: i16) {
    w.compact_string("transactional_id");
    w.i32("transaction_timeout_ms");
    if version >= 3 {
        w.i64("producer_id");
        w.i16("producer_epoch");
    }
    w.tags("tagged_fields");
}

/// `InitProducerIdResponse` v2-v5.
pub fn init_producer_id_response(w: &mut Walk<'_>, _version: i16) {
    w.i32("throttle_time_ms");
    w.error_code("error_code");
    w.i64("producer_id");
    w.i16("producer_epoch");
    w.tags("tagged_fields");
}
