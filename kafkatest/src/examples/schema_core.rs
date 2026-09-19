//! Field-by-field schemas for the metadata APIs: ApiVersions(18),
//! DescribeTopicPartitions(75), Metadata(3), CreateTopics(19), DeleteTopics(20).
//!
//! Each function walks one body with [`super::annotate::Walk`], in the exact order the
//! protocol writes the fields. A wrong order shows up immediately: the walk stops short of
//! the frame's last byte and `--capture-examples` refuses to write the example.

use super::annotate::Walk;

// ---------------------------------------------------------------------------------------
// ApiVersions (18)
// ---------------------------------------------------------------------------------------

/// `ApiVersionsRequest`: two compact strings from v3 on, nothing at all before that.
pub fn api_versions_request(w: &mut Walk<'_>, version: i16) {
    if version >= 3 {
        w.compact_string("client_software_name");
        w.compact_string("client_software_version");
        w.tags("tagged_fields");
    }
}

/// `ApiVersionsResponse`: the one response whose header is always v0.
pub fn api_versions_response(w: &mut Walk<'_>, version: i16) {
    w.error_code("error_code");
    let entry = |w: &mut Walk<'_>, _i: usize| {
        w.api_key("api_key");
        w.i16("min_version");
        w.i16("max_version");
        if version >= 3 {
            w.tags("tagged_fields");
        }
    };
    if version >= 3 {
        w.compact_array("api_keys", entry);
    } else {
        w.array("api_keys", entry);
    }
    if version >= 1 {
        w.i32("throttle_time_ms");
    }
    if version >= 3 {
        // KIP-584 feature versioning. The epoch is the metadata log offset the finalized
        // features were read at, so it is different on every boot.
        w.tags_known(
            "tagged_fields",
            &[
                (0, "supported_features", false),
                (1, "finalized_features_epoch", true),
                (2, "finalized_features", false),
                (3, "zk_migration_ready", false),
            ],
        );
    }
}

// ---------------------------------------------------------------------------------------
// DescribeTopicPartitions (75)
// ---------------------------------------------------------------------------------------

/// `DescribeTopicPartitionsRequest` v0.
pub fn describe_topic_partitions_request(w: &mut Walk<'_>) {
    w.compact_array("topics", |w, _| {
        w.compact_string("name");
        w.tags("tagged_fields");
    });
    w.i32("response_partition_limit");
    cursor(w, "cursor");
    w.tags("tagged_fields");
}

/// `DescribeTopicPartitionsResponse` v0.
pub fn describe_topic_partitions_response(w: &mut Walk<'_>) {
    w.i32("throttle_time_ms");
    w.compact_array("topics", |w, _| {
        w.error_code("error_code");
        w.compact_string("name");
        w.uuid("topic_id");
        w.boolean("is_internal");
        w.compact_array("partitions", |w, _| {
            w.error_code("error_code");
            w.i32("partition_index");
            w.i32("leader_id");
            w.i32("leader_epoch");
            int32_array(w, "replica_nodes");
            int32_array(w, "isr_nodes");
            int32_array(w, "eligible_leader_replicas");
            int32_array(w, "last_known_elr");
            int32_array(w, "offline_replicas");
            w.tags("tagged_fields");
        });
        w.i32("topic_authorized_operations");
        w.tags("tagged_fields");
    });
    cursor(w, "next_cursor");
    w.tags("tagged_fields");
}

/// The nullable `Cursor` struct both DescribeTopicPartitions bodies end with.
fn cursor(w: &mut Walk<'_>, name: &str) {
    w.nullable_struct(name, |w| {
        w.compact_string("topic_name");
        w.i32("partition_index");
        w.tags("tagged_fields");
    });
}

/// A compact array of plain `int32`s, annotated as one field.
fn int32_array(w: &mut Walk<'_>, name: &str) {
    w.compact_array(name, |w, _| {
        w.i32("");
    });
}

// ---------------------------------------------------------------------------------------
// Metadata (3)
// ---------------------------------------------------------------------------------------

/// `MetadataRequest` v9-v13.
pub fn metadata_request(w: &mut Walk<'_>, version: i16) {
    w.compact_array("topics", |w, _| {
        if version >= 10 {
            w.uuid("topic_id");
        }
        w.compact_string("name");
        w.tags("tagged_fields");
    });
    if version >= 4 {
        w.boolean("allow_auto_topic_creation");
    }
    if (8..=10).contains(&version) {
        w.boolean("include_cluster_authorized_operations");
    }
    if version >= 8 {
        w.boolean("include_topic_authorized_operations");
    }
    w.tags("tagged_fields");
}

/// `MetadataResponse` v9-v13.
pub fn metadata_response(w: &mut Walk<'_>, version: i16) {
    w.i32("throttle_time_ms");
    w.compact_array("brokers", |w, _| {
        w.i32("node_id");
        w.compact_string("host");
        w.i32("port");
        w.compact_string("rack");
        w.tags("tagged_fields");
    });
    w.compact_string("cluster_id");
    w.i32("controller_id");
    w.compact_array("topics", |w, _| {
        w.error_code("error_code");
        w.compact_string("name");
        if version >= 10 {
            w.uuid("topic_id");
        }
        w.boolean("is_internal");
        w.compact_array("partitions", |w, _| {
            w.error_code("error_code");
            w.i32("partition_index");
            w.i32("leader_id");
            if version >= 7 {
                w.i32("leader_epoch");
            }
            int32_array(w, "replica_nodes");
            int32_array(w, "isr_nodes");
            if version >= 5 {
                int32_array(w, "offline_replicas");
            }
            w.tags("tagged_fields");
        });
        if version >= 8 {
            w.i32("topic_authorized_operations");
        }
        w.tags("tagged_fields");
    });
    if (8..=10).contains(&version) {
        w.i32("cluster_authorized_operations");
    }
    w.tags("tagged_fields");
}

// ---------------------------------------------------------------------------------------
// CreateTopics (19)
// ---------------------------------------------------------------------------------------

/// `CreateTopicsRequest` v5-v7.
pub fn create_topics_request(w: &mut Walk<'_>, _version: i16) {
    w.compact_array("topics", |w, _| {
        w.compact_string("name");
        w.i32("num_partitions");
        w.i16("replication_factor");
        w.compact_array("assignments", |w, _| {
            w.i32("partition_index");
            int32_array(w, "broker_ids");
            w.tags("tagged_fields");
        });
        w.compact_array("configs", |w, _| {
            w.compact_string("name");
            w.compact_string("value");
            w.tags("tagged_fields");
        });
        w.tags("tagged_fields");
    });
    w.i32("timeout_ms");
    w.boolean("validate_only");
    w.tags("tagged_fields");
}

/// `CreateTopicsResponse` v5-v7.
pub fn create_topics_response(w: &mut Walk<'_>, version: i16) {
    w.i32("throttle_time_ms");
    w.compact_array("topics", |w, _| {
        w.compact_string("name");
        if version >= 7 {
            w.uuid("topic_id");
        }
        w.error_code("error_code");
        w.compact_string("error_message");
        if version >= 5 {
            w.i32("num_partitions");
            w.i16("replication_factor");
            w.compact_array("configs", |w, _| {
                w.compact_string("name");
                w.compact_string("value");
                w.boolean("read_only");
                w.i8("config_source");
                w.boolean("is_sensitive");
                w.tags("tagged_fields");
            });
        }
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}

// ---------------------------------------------------------------------------------------
// DeleteTopics (20)
// ---------------------------------------------------------------------------------------

/// `DeleteTopicsRequest` v6.
pub fn delete_topics_request(w: &mut Walk<'_>, version: i16) {
    if version >= 6 {
        w.compact_array("topics", |w, _| {
            w.compact_string("name");
            w.uuid("topic_id");
            w.tags("tagged_fields");
        });
    } else {
        w.compact_array("topic_names", |w, _| {
            w.compact_string("");
        });
    }
    w.i32("timeout_ms");
    w.tags("tagged_fields");
}

/// `DeleteTopicsResponse` v6.
pub fn delete_topics_response(w: &mut Walk<'_>, version: i16) {
    w.i32("throttle_time_ms");
    w.compact_array("responses", |w, _| {
        w.compact_string("name");
        if version >= 6 {
            w.uuid("topic_id");
        }
        w.error_code("error_code");
        if version >= 5 {
            w.compact_string("error_message");
        }
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}
