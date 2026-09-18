//! Field-by-field schemas for the coordinator APIs: FindCoordinator(10), JoinGroup(11),
//! SyncGroup(14), Heartbeat(12), LeaveGroup(13), OffsetCommit(8) and OffsetFetch(9).

use super::annotate::Walk;

// ---------------------------------------------------------------------------------------
// FindCoordinator (10)
// ---------------------------------------------------------------------------------------

/// `FindCoordinatorRequest` v3-v6. From v4 on, one request can ask about several keys.
pub fn find_coordinator_request(w: &mut Walk<'_>, version: i16) {
    if version <= 3 {
        w.compact_string("key");
    }
    if version >= 1 {
        w.i8("key_type");
    }
    if version >= 4 {
        w.compact_array("coordinator_keys", |w, _| {
            w.compact_string("");
        });
    }
    w.tags("tagged_fields");
}

/// `FindCoordinatorResponse` v3-v6.
pub fn find_coordinator_response(w: &mut Walk<'_>, version: i16) {
    w.i32("throttle_time_ms");
    if version <= 3 {
        w.error_code("error_code");
        w.compact_string("error_message");
        w.i32("node_id");
        w.compact_string("host");
        w.i32("port");
    } else {
        w.compact_array("coordinators", |w, _| {
            w.compact_string("key");
            w.i32("node_id");
            w.compact_string("host");
            w.i32("port");
            w.error_code("error_code");
            w.compact_string("error_message");
            w.tags("tagged_fields");
        });
    }
    w.tags("tagged_fields");
}

// ---------------------------------------------------------------------------------------
// JoinGroup (11)
// ---------------------------------------------------------------------------------------

/// `JoinGroupRequest` v6-v9.
pub fn join_group_request(w: &mut Walk<'_>, version: i16) {
    w.compact_string("group_id");
    w.i32("session_timeout_ms");
    if version >= 1 {
        w.i32("rebalance_timeout_ms");
    }
    w.compact_string("member_id");
    if version >= 5 {
        w.compact_string("group_instance_id");
    }
    w.compact_string("protocol_type");
    w.compact_array("protocols", |w, _| {
        w.compact_string("name");
        w.compact_bytes("metadata");
        w.tags("tagged_fields");
    });
    if version >= 8 {
        w.compact_string("reason");
    }
    w.tags("tagged_fields");
}

/// `JoinGroupResponse` v6-v9.
pub fn join_group_response(w: &mut Walk<'_>, version: i16) {
    w.i32("throttle_time_ms");
    w.error_code("error_code");
    w.i32("generation_id");
    if version >= 7 {
        w.compact_string("protocol_type");
    }
    w.compact_string("protocol_name");
    w.compact_string("leader");
    if version >= 9 {
        w.boolean("skip_assignment");
    }
    w.compact_string("member_id");
    w.compact_array("members", |w, _| {
        w.compact_string("member_id");
        if version >= 5 {
            w.compact_string("group_instance_id");
        }
        w.compact_bytes("metadata");
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}

// ---------------------------------------------------------------------------------------
// SyncGroup (14)
// ---------------------------------------------------------------------------------------

/// `SyncGroupRequest` v4-v5.
pub fn sync_group_request(w: &mut Walk<'_>, version: i16) {
    w.compact_string("group_id");
    w.i32("generation_id");
    w.compact_string("member_id");
    if version >= 3 {
        w.compact_string("group_instance_id");
    }
    if version >= 5 {
        w.compact_string("protocol_type");
        w.compact_string("protocol_name");
    }
    w.compact_array("assignments", |w, _| {
        w.compact_string("member_id");
        w.compact_bytes("assignment");
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}

/// `SyncGroupResponse` v4-v5.
pub fn sync_group_response(w: &mut Walk<'_>, version: i16) {
    w.i32("throttle_time_ms");
    w.error_code("error_code");
    if version >= 5 {
        w.compact_string("protocol_type");
        w.compact_string("protocol_name");
    }
    w.compact_bytes("assignment");
    w.tags("tagged_fields");
}

// ---------------------------------------------------------------------------------------
// Heartbeat (12)
// ---------------------------------------------------------------------------------------

/// `HeartbeatRequest` v4.
pub fn heartbeat_request(w: &mut Walk<'_>, version: i16) {
    w.compact_string("group_id");
    w.i32("generation_id");
    w.compact_string("member_id");
    if version >= 3 {
        w.compact_string("group_instance_id");
    }
    w.tags("tagged_fields");
}

/// `HeartbeatResponse` v4.
pub fn heartbeat_response(w: &mut Walk<'_>, _version: i16) {
    w.i32("throttle_time_ms");
    w.error_code("error_code");
    w.tags("tagged_fields");
}

// ---------------------------------------------------------------------------------------
// LeaveGroup (13)
// ---------------------------------------------------------------------------------------

/// `LeaveGroupRequest` v4-v5.
pub fn leave_group_request(w: &mut Walk<'_>, version: i16) {
    w.compact_string("group_id");
    w.compact_array("members", |w, _| {
        w.compact_string("member_id");
        w.compact_string("group_instance_id");
        if version >= 5 {
            w.compact_string("reason");
        }
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}

/// `LeaveGroupResponse` v4-v5.
pub fn leave_group_response(w: &mut Walk<'_>, _version: i16) {
    w.i32("throttle_time_ms");
    w.error_code("error_code");
    w.compact_array("members", |w, _| {
        w.compact_string("member_id");
        w.compact_string("group_instance_id");
        w.error_code("error_code");
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}

// ---------------------------------------------------------------------------------------
// OffsetCommit (8)
// ---------------------------------------------------------------------------------------

/// `OffsetCommitRequest` v8-v9.
pub fn offset_commit_request(w: &mut Walk<'_>, version: i16) {
    w.compact_string("group_id");
    if version >= 1 {
        w.i32("generation_id_or_member_epoch");
        w.compact_string("member_id");
    }
    if (2..=4).contains(&version) {
        w.i64("retention_time_ms");
    }
    if version >= 7 {
        w.compact_string("group_instance_id");
    }
    w.compact_array("topics", |w, _| {
        w.compact_string("name");
        w.compact_array("partitions", |w, _| {
            w.i32("partition_index");
            w.i64("committed_offset");
            if version >= 6 {
                w.i32("committed_leader_epoch");
            }
            w.compact_string("committed_metadata");
            w.tags("tagged_fields");
        });
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}

/// `OffsetCommitResponse` v8-v9.
pub fn offset_commit_response(w: &mut Walk<'_>, _version: i16) {
    w.i32("throttle_time_ms");
    w.compact_array("topics", |w, _| {
        w.compact_string("name");
        w.compact_array("partitions", |w, _| {
            w.i32("partition_index");
            w.error_code("error_code");
            w.tags("tagged_fields");
        });
        w.tags("tagged_fields");
    });
    w.tags("tagged_fields");
}

// ---------------------------------------------------------------------------------------
// OffsetFetch (9)
// ---------------------------------------------------------------------------------------

/// `OffsetFetchRequest` v8-v9: one request can ask for several groups at once.
pub fn offset_fetch_request(w: &mut Walk<'_>, version: i16) {
    if version <= 7 {
        w.compact_string("group_id");
        w.compact_array("topics", |w, _| {
            w.compact_string("name");
            w.compact_array("partition_indexes", |w, _| {
                w.i32("");
            });
            w.tags("tagged_fields");
        });
    } else {
        w.compact_array("groups", |w, _| {
            w.compact_string("group_id");
            if version >= 9 {
                w.compact_string("member_id");
                w.i32("member_epoch");
            }
            w.compact_array("topics", |w, _| {
                w.compact_string("name");
                w.compact_array("partition_indexes", |w, _| {
                    w.i32("");
                });
                w.tags("tagged_fields");
            });
            w.tags("tagged_fields");
        });
    }
    if version >= 7 {
        w.boolean("require_stable");
    }
    w.tags("tagged_fields");
}

/// `OffsetFetchResponse` v8-v9.
pub fn offset_fetch_response(w: &mut Walk<'_>, version: i16) {
    w.i32("throttle_time_ms");
    if version <= 7 {
        w.compact_array("topics", |w, _| {
            w.compact_string("name");
            w.compact_array("partitions", |w, _| {
                w.i32("partition_index");
                w.i64("committed_offset");
                w.i32("committed_leader_epoch");
                w.compact_string("metadata");
                w.error_code("error_code");
                w.tags("tagged_fields");
            });
            w.tags("tagged_fields");
        });
        w.error_code("error_code");
    } else {
        w.compact_array("groups", |w, _| {
            w.compact_string("group_id");
            w.compact_array("topics", |w, _| {
                w.compact_string("name");
                w.compact_array("partitions", |w, _| {
                    w.i32("partition_index");
                    w.i64("committed_offset");
                    w.i32("committed_leader_epoch");
                    w.compact_string("metadata");
                    w.error_code("error_code");
                    w.tags("tagged_fields");
                });
                w.tags("tagged_fields");
            });
            w.error_code("error_code");
            w.tags("tagged_fields");
        });
    }
    w.tags("tagged_fields");
}
