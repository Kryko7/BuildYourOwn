//! The decoder, against bodies real etcd 3.7.1 actually sent.
//!
//! Every string below was copied out of a live etcd on this machine, including the parts
//! that look wrong: fields at their zero value are missing, 64-bit numbers are strings, and
//! the gateway spells its request keys in camelCase while spelling its response keys in
//! snake_case. A decoder that only ever sees its own encoder's output would not survive any
//! of that, so it is pinned here.

use disttest::etcd::{
    as_bool, as_i64, as_u64, b64, cmp_create_eq, cmp_value_eq, cmp_version_eq, field, op_delete,
    op_put, op_range, prefix_end, unb64, Header, KeyValue, PutRequest, RangeRequest, ZERO_KEY,
};
use serde_json::{json, Value};

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("the recorded body must be valid JSON")
}

const PUT: &str = r#"{"header":{"cluster_id":"12101053776118486841","member_id":"15985099378392235387","revision":"2","raft_term":"2"}}"#;

const PUT_PREV: &str = r#"{"header":{"cluster_id":"12101053776118486841","member_id":"15985099378392235387","revision":"3","raft_term":"2"},"prev_kv":{"key":"Zm9v","create_revision":"2","mod_revision":"2","version":"1","value":"YmFy"}}"#;

const RANGE: &str = r#"{"header":{"cluster_id":"12101053776118486841","member_id":"15985099378392235387","revision":"2","raft_term":"2"},"kvs":[{"key":"Zm9v","create_revision":"2","mod_revision":"2","version":"1","value":"YmFy"}],"count":"1"}"#;

const RANGE_EMPTY: &str = r#"{"header":{"cluster_id":"12101053776118486841","member_id":"15985099378392235387","revision":"3","raft_term":"2"}}"#;

const RANGE_SORTED_LIMITED: &str = r#"{"header":{"revision":"8","raft_term":"2"},"kvs":[{"key":"a2M=","create_revision":"8","mod_revision":"8","version":"1","value":"Yw=="},{"key":"a2I=","create_revision":"7","mod_revision":"7","version":"1","value":"Yg=="}],"more":true,"count":"3"}"#;

const KEYS_ONLY: &str = r#"{"header":{"revision":"8"},"kvs":[{"key":"a2E=","create_revision":"6","mod_revision":"6","version":"1"}],"count":"1"}"#;

const TXN_FAILED: &str = r#"{"header":{"revision":"13","raft_term":"2"},"responses":[{"response_range":{"header":{"revision":"12"},"kvs":[{"key":"bm9wZQ==","create_revision":"12","mod_revision":"12","version":"1","value":"bWFkZQ=="}],"count":"1"}},{"response_delete_range":{"header":{"revision":"13"},"deleted":"1"}}]}"#;

const TXN_SUCCEEDED: &str = r#"{"header":{"revision":"4","raft_term":"2"},"succeeded":true,"responses":[{"response_put":{"header":{"revision":"4"}}}]}"#;

const DELETE: &str = r#"{"header":{"revision":"5","raft_term":"2"},"deleted":"1","prev_kvs":[{"key":"b2s=","create_revision":"4","mod_revision":"4","version":"1","value":"MQ=="}]}"#;

const LEASE_GRANT: &str =
    r#"{"header":{"revision":"4","raft_term":"2"},"ID":"9041997376642916107","TTL":"5"}"#;

const KEEPALIVE: &str = r#"{"result":{"header":{"revision":"8","raft_term":"2"},"ID":"9041997376642916121","TTL":"5"}}"#;

const WATCH_CREATED: &str =
    r#"{"result":{"header":{"revision":"9","raft_term":"2"},"watch_id":"7","created":true}}"#;

const WATCH_PUT: &str = r#"{"result":{"header":{"revision":"9","raft_term":"2"},"watch_id":"7","events":[{"kv":{"key":"a3c=","create_revision":"9","mod_revision":"9","version":"1","value":"djE="}}]}}"#;

const WATCH_DELETE: &str = r#"{"result":{"header":{"revision":"10","raft_term":"2"},"watch_id":"7","events":[{"type":"DELETE","kv":{"key":"a3c=","mod_revision":"10"}}]}}"#;

const STATUS: &str = r#"{"header":{"cluster_id":"12101053776118486841","member_id":"15985099378392235387","revision":"2","raft_term":"2"},"version":"3.7.1","dbSize":"24576","leader":"15985099378392235387","raftIndex":"5","raftTerm":"2","raftAppliedIndex":"5","dbSizeInUse":"24576","storageVersion":"3.7.0","dbSizeQuota":"2147483648","downgradeInfo":{}}"#;

const MEMBER_LIST: &str = r#"{"header":{"cluster_id":"12101053776118486841","member_id":"15985099378392235387","raft_term":"2"},"members":[{"ID":"15985099378392235387","name":"n1","peerURLs":["http://127.0.0.1:12380"],"clientURLs":["http://127.0.0.1:12379"]}]}"#;

const COMPACTED_ERROR: &str =
    r#"{"code":11,"message":"etcdserver: mvcc: required revision has been compacted"}"#;

#[test]
fn a_header_decodes_from_every_body_that_carries_one() {
    let h = Header::from(&parse(PUT));
    assert_eq!(h.revision, 2);
    assert_eq!(h.raft_term, 2);
    assert_eq!(h.cluster_id, 12_101_053_776_118_486_841);
    assert_eq!(h.member_id, 15_985_099_378_392_235_387);

    // The member list omits `revision` entirely, because it is zero there.
    let h = Header::from(&parse(MEMBER_LIST));
    assert_eq!(h.revision, 0);
    assert_eq!(h.raft_term, 2);
    assert_ne!(h.cluster_id, 0);
}

#[test]
fn a_kv_carries_its_three_revisions() {
    let body = parse(RANGE);
    let kvs = body["kvs"].as_array().expect("kvs");
    let kv = KeyValue::from(&kvs[0]).expect("decode");
    assert_eq!(kv.key_str(), "foo");
    assert_eq!(kv.value_str(), "bar");
    assert_eq!(kv.create_revision, 2);
    assert_eq!(kv.mod_revision, 2);
    assert_eq!(kv.version, 1);
    assert_eq!(kv.lease, 0, "a key with no lease omits the field");
}

#[test]
fn an_empty_range_omits_both_kvs_and_count() {
    let body = parse(RANGE_EMPTY);
    assert!(body.get("kvs").is_none());
    assert_eq!(as_i64(&body, "count"), 0);
    assert!(!as_bool(&body, "more"));
}

#[test]
fn a_limited_descending_range_keeps_the_full_count() {
    let body = parse(RANGE_SORTED_LIMITED);
    assert_eq!(as_i64(&body, "count"), 3, "count ignores the limit");
    assert!(as_bool(&body, "more"));
    let kvs = body["kvs"].as_array().expect("kvs");
    assert_eq!(kvs.len(), 2);
    let first = KeyValue::from(&kvs[0]).expect("decode");
    let second = KeyValue::from(&kvs[1]).expect("decode");
    assert_eq!(first.key_str(), "kc");
    assert_eq!(second.key_str(), "kb");
}

#[test]
fn keys_only_leaves_the_value_out_rather_than_sending_an_empty_one() {
    let body = parse(KEYS_ONLY);
    let kv = KeyValue::from(&body["kvs"][0]).expect("decode");
    assert!(kv.value.is_empty());
    assert_eq!(kv.key_str(), "ka");
}

#[test]
fn a_transaction_that_failed_omits_succeeded_entirely() {
    let body = parse(TXN_FAILED);
    assert!(
        body.get("succeeded").is_none(),
        "false is the zero value, so the gateway leaves it out"
    );
    assert!(!as_bool(&body, "succeeded"));
    let responses = body["responses"].as_array().expect("responses");
    assert_eq!(responses.len(), 2);
    assert!(field(&responses[0], "response_range").is_some());
    assert!(field(&responses[1], "response_delete_range").is_some());

    let body = parse(TXN_SUCCEEDED);
    assert!(as_bool(&body, "succeeded"));
    assert!(field(&body["responses"][0], "response_put").is_some());
}

#[test]
fn camel_case_spellings_resolve_too() {
    // The gateway accepts camelCase on the way in; a hand-written server may answer in it.
    let camel = json!({"responseDeleteRange": {"deleted": "2"}});
    assert!(field(&camel, "response_delete_range").is_some());
    assert_eq!(
        as_i64(
            field(&camel, "response_delete_range").unwrap_or(&Value::Null),
            "deleted"
        ),
        2
    );
}

#[test]
fn a_delete_reports_what_it_removed() {
    let body = parse(DELETE);
    assert_eq!(as_i64(&body, "deleted"), 1);
    let prev = KeyValue::from(&body["prev_kvs"][0]).expect("decode");
    assert_eq!(prev.key_str(), "ok");
    assert_eq!(prev.value_str(), "1");
}

#[test]
fn a_put_with_prev_kv_carries_the_value_that_was_there_before() {
    let body = parse(PUT_PREV);
    let prev = KeyValue::from(&body["prev_kv"]).expect("decode");
    assert_eq!(prev.value_str(), "bar");
    assert_eq!(Header::from(&body).revision, 3);
}

#[test]
fn lease_ids_are_capitalised_and_survive_being_larger_than_an_i32() {
    let body = parse(LEASE_GRANT);
    assert_eq!(as_i64(&body, "ID"), 9_041_997_376_642_916_107);
    assert_eq!(as_i64(&body, "TTL"), 5);

    // A keepalive is a stream, so its message is wrapped.
    let body = parse(KEEPALIVE);
    let inner = &body["result"];
    assert_eq!(as_i64(inner, "ID"), 9_041_997_376_642_916_121);
    assert_eq!(as_i64(inner, "TTL"), 5);
}

#[test]
fn watch_messages_are_wrapped_and_a_missing_type_is_a_put() {
    for (text, want_type) in [(WATCH_PUT, "PUT"), (WATCH_DELETE, "DELETE")] {
        let body = parse(text);
        let inner = &body["result"];
        let event = &inner["events"][0];
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("PUT");
        assert_eq!(kind, want_type);
        let kv = KeyValue::from(&event["kv"]).expect("decode");
        assert_eq!(kv.key_str(), "kw");
    }
    let created = parse(WATCH_CREATED);
    assert!(as_bool(&created["result"], "created"));
    assert_eq!(as_i64(&created["result"], "watch_id"), 7);
    assert!(
        !as_bool(&created["result"], "canceled"),
        "an absent flag is false"
    );
}

#[test]
fn status_spells_its_raft_fields_in_camel_case() {
    let body = parse(STATUS);
    assert_eq!(as_u64(&body, "raftIndex"), 5);
    assert_eq!(as_u64(&body, "raftTerm"), 2);
    assert_eq!(as_u64(&body, "raftAppliedIndex"), 5);
    assert_eq!(as_u64(&body, "leader"), 15_985_099_378_392_235_387);
    assert_eq!(as_i64(&body, "dbSize"), 24_576);
    assert_eq!(
        body["version"].as_str(),
        Some("3.7.1"),
        "the version is a plain string, not a number"
    );
}

#[test]
fn a_member_list_names_its_urls() {
    let body = parse(MEMBER_LIST);
    let member = &body["members"][0];
    assert_eq!(as_u64(member, "ID"), 15_985_099_378_392_235_387);
    assert_eq!(member["name"].as_str(), Some("n1"));
    assert_eq!(
        member["peerURLs"][0].as_str(),
        Some("http://127.0.0.1:12380")
    );
}

#[test]
fn an_error_body_carries_a_grpc_code() {
    let body = parse(COMPACTED_ERROR);
    assert_eq!(body["code"].as_i64(), Some(11));
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("compacted"));
}

#[test]
fn request_builders_produce_the_bodies_the_gateway_accepts() {
    let put = PutRequest::new(b"foo", b"bar").prev_kv().lease(7).body();
    assert_eq!(put["key"], json!("Zm9v"));
    assert_eq!(put["value"], json!("YmFy"));
    assert_eq!(put["prev_kv"], json!(true));
    assert_eq!(put["lease"], json!("7"), "64-bit values go out as strings");

    let range = RangeRequest::prefix(b"k/")
        .limit(2)
        .sort("DESCEND", "KEY")
        .at_revision(9)
        .body();
    assert_eq!(range["key"], json!(b64(b"k/")));
    assert_eq!(range["range_end"], json!(b64(b"k0")));
    assert_eq!(range["limit"], json!("2"));
    assert_eq!(range["sort_order"], json!("DESCEND"));
    assert_eq!(range["sort_target"], json!("KEY"));
    assert_eq!(range["revision"], json!("9"));

    let all = RangeRequest::all().body();
    assert_eq!(all["key"], json!(b64(ZERO_KEY)));
    assert_eq!(all["range_end"], json!(b64(ZERO_KEY)));

    assert_eq!(
        RangeRequest::key(b"x").count_only().keys_only().body()["count_only"],
        json!(true)
    );
}

#[test]
fn comparisons_and_operations_use_the_field_names_the_gateway_reads() {
    assert_eq!(cmp_value_eq(b"k", b"v")["target"], json!("VALUE"));
    assert_eq!(cmp_value_eq(b"k", b"v")["value"], json!(b64(b"v")));
    assert_eq!(cmp_version_eq(b"k", 0)["version"], json!("0"));
    assert_eq!(cmp_create_eq(b"k", 3)["create_revision"], json!("3"));

    assert!(op_put(b"k", b"v").get("requestPut").is_some());
    assert!(op_range(b"k").get("requestRange").is_some());
    assert!(op_delete(b"k").get("requestDeleteRange").is_some());
}

#[test]
fn prefix_end_matches_what_etcd_does() {
    assert_eq!(prefix_end(b"k"), b"l".to_vec());
    assert_eq!(prefix_end(b"k/"), b"k0".to_vec());
    assert_eq!(prefix_end(&[0x61, 0xff]), vec![0x62]);
    assert_eq!(prefix_end(&[0xff]), vec![0], "all-0xff means everything");
    assert_eq!(unb64(&b64(b"round trip")).expect("decode"), b"round trip");
}
