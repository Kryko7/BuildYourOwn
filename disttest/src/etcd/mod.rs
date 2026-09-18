//! The client the whole suite speaks: a subset of the etcd v3 HTTP/JSON API.
//!
//! Every endpoint is a `POST` whose body is a JSON object, and every key and value on the
//! wire is base64. Two properties of that gateway are worth stating, because the tests rely
//! on both and a learner's server has to match them:
//!
//! * **Zero values may be omitted.** protobuf's JSON mapping drops fields at their default,
//!   so `{"count":"0"}` and a missing `count` mean the same thing. Everything decoded here
//!   treats a missing field as its zero, and no test ever asserts a field is *present*.
//! * **64-bit integers are strings.** `"revision":"7"` is the canonical form; a plain number
//!   is accepted too, because that is what a hand-written server usually emits first.
//!
//! Errors arrive as `{"code":<grpc code>,"message":"..."}` with a 4xx status.

pub mod http;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
pub use http::{Http, HttpError, HttpResponse};
use serde_json::{json, Value};
use std::time::Duration;

// ---------------------------------------------------------------------------------------
// gRPC status codes the suite names
// ---------------------------------------------------------------------------------------

/// `InvalidArgument` — a malformed request (bad base64, an impossible range).
pub const CODE_INVALID_ARGUMENT: i64 = 3;
/// `NotFound` — most often a lease that does not exist.
pub const CODE_NOT_FOUND: i64 = 5;
/// `ResourceExhausted` — the request was larger than the server accepts.
pub const CODE_RESOURCE_EXHAUSTED: i64 = 8;
/// `OutOfRange` — `mvcc: required revision has been compacted`.
pub const CODE_OUT_OF_RANGE: i64 = 11;
/// `Unavailable` — no quorum, no leader, or the member is shutting down.
pub const CODE_UNAVAILABLE: i64 = 14;

/// The key that means "from the beginning" and the `range_end` that means "to the end".
pub const ZERO_KEY: &[u8] = &[0];

/// base64, the encoding every key and value uses on the wire.
pub fn b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

/// Undo [`b64`].
pub fn unb64(s: &str) -> Result<Vec<u8>, String> {
    B64.decode(s)
        .map_err(|e| format!("{s:?} is not valid base64: {e}"))
}

/// The `range_end` that turns a key into a prefix range: the key with its last byte
/// incremented, and `[0]` (everything) for an empty prefix or a prefix of all `0xff`.
pub fn prefix_end(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.pop() {
        if last < 0xff {
            end.push(last + 1);
            return end;
        }
    }
    vec![0]
}

// ---------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------

/// Everything a request can answer that is not a decoded response.
#[derive(Debug, Clone)]
pub enum EtcdError {
    /// The transport failed before any HTTP response arrived.
    Transport(HttpError),
    /// The server answered, but not with success.
    Status {
        /// HTTP status code.
        status: u16,
        /// The gRPC code in the error body, or `-1` when the body carried none.
        code: i64,
        /// The `message` field of the error body, or the body itself.
        message: String,
        /// The response as it arrived, for the failure block.
        body: String,
    },
    /// The status was fine but the body was not the documented shape.
    Decode {
        /// What the harness was trying to read.
        what: String,
        /// The body as it arrived.
        body: String,
    },
}

impl std::fmt::Display for EtcdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EtcdError::Transport(e) => write!(f, "{e}"),
            EtcdError::Status {
                status,
                code,
                message,
                ..
            } => write!(f, "HTTP {status}, code {code}: {message}"),
            EtcdError::Decode { what, body } => {
                write!(f, "cannot read {what} from {}", short(body))
            }
        }
    }
}

impl EtcdError {
    /// The gRPC code, when the server answered with an error body.
    pub fn code(&self) -> Option<i64> {
        match self {
            EtcdError::Status { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// The HTTP status, when there was one.
    pub fn status(&self) -> Option<u16> {
        match self {
            EtcdError::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// True when the node was unreachable rather than unhappy.
    pub fn is_transport(&self) -> bool {
        matches!(self, EtcdError::Transport(_))
    }

    /// The body as it arrived, for a failure block.
    pub fn body(&self) -> String {
        match self {
            EtcdError::Status { body, .. } | EtcdError::Decode { body, .. } => body.clone(),
            EtcdError::Transport(e) => e.to_string(),
        }
    }
}

fn short(s: &str) -> String {
    if s.len() <= 300 {
        s.to_string()
    } else {
        format!("{}… ({} bytes)", &s[..300], s.len())
    }
}

// ---------------------------------------------------------------------------------------
// Tolerant decoding helpers
// ---------------------------------------------------------------------------------------

/// Read a 64-bit integer that may be a JSON string, a JSON number, or absent (zero).
pub fn as_i64(v: &Value, field: &str) -> i64 {
    match v.get(field) {
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        _ => 0,
    }
}

/// The unsigned form of [`as_i64`]; ids are `uint64` and overflow `i64`.
pub fn as_u64(v: &Value, field: &str) -> u64 {
    match v.get(field) {
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_i64().map(|x| x as u64))
            .unwrap_or(0),
        _ => 0,
    }
}

/// Read a boolean that may be absent (false).
pub fn as_bool(v: &Value, field: &str) -> bool {
    v.get(field).and_then(Value::as_bool).unwrap_or(false)
}

/// Read a base64 field; absent means empty.
pub fn as_bytes(v: &Value, field: &str) -> Result<Vec<u8>, String> {
    match v.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) => unb64(s),
        Some(other) => Err(format!("{field} should be a base64 string, got {other}")),
    }
}

/// Read a string field; absent means empty.
pub fn as_str(v: &Value, field: &str) -> String {
    v.get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Look a field up under both the snake_case and the camelCase spelling.
///
/// The gateway emits `response_put` but accepts `requestPut`, and a hand-written server may
/// pick either; the suite never fails a test over the spelling of a key.
pub fn field<'a>(v: &'a Value, snake: &str) -> Option<&'a Value> {
    if let Some(x) = v.get(snake) {
        return Some(x);
    }
    v.get(camel(snake))
}

fn camel(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper = false;
    for ch in snake.chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Decoded responses
// ---------------------------------------------------------------------------------------

/// The `header` every response carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    /// Identifies the cluster; the same on every member.
    pub cluster_id: u64,
    /// Identifies the member that answered.
    pub member_id: u64,
    /// The store's revision when the request was applied.
    pub revision: i64,
    /// The raft term of the member that answered.
    pub raft_term: u64,
}

impl Header {
    /// Decode a `header` object, tolerating every field being absent.
    pub fn from(v: &Value) -> Header {
        let h = v.get("header").unwrap_or(&Value::Null);
        Header {
            cluster_id: as_u64(h, "cluster_id"),
            member_id: as_u64(h, "member_id"),
            revision: as_i64(h, "revision"),
            raft_term: as_u64(h, "raft_term"),
        }
    }
}

/// One key/value pair as the store returns it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyValue {
    /// The key.
    pub key: Vec<u8>,
    /// The revision that created this key (unchanged by later puts).
    pub create_revision: i64,
    /// The revision of the last put.
    pub mod_revision: i64,
    /// How many puts this key has seen since it was created; 1 after the first.
    pub version: i64,
    /// The value, empty when `keys_only` was asked for.
    pub value: Vec<u8>,
    /// The lease this key is attached to, 0 when none.
    pub lease: i64,
}

impl KeyValue {
    /// Decode one `kv` object.
    pub fn from(v: &Value) -> Result<KeyValue, String> {
        Ok(KeyValue {
            key: as_bytes(v, "key")?,
            create_revision: as_i64(v, "create_revision"),
            mod_revision: as_i64(v, "mod_revision"),
            version: as_i64(v, "version"),
            value: as_bytes(v, "value")?,
            lease: as_i64(v, "lease"),
        })
    }

    /// The key as text, for report lines.
    pub fn key_str(&self) -> String {
        String::from_utf8_lossy(&self.key).to_string()
    }

    /// The value as text, for report lines.
    pub fn value_str(&self) -> String {
        String::from_utf8_lossy(&self.value).to_string()
    }
}

fn kv_list(v: &Value, field_name: &str) -> Result<Vec<KeyValue>, String> {
    let Some(list) = field(v, field_name) else {
        return Ok(Vec::new());
    };
    match list {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => items.iter().map(KeyValue::from).collect(),
        other => Err(format!("{field_name} should be an array, got {other}")),
    }
}

fn kv_opt(v: &Value, field_name: &str) -> Result<Option<KeyValue>, String> {
    match field(v, field_name) {
        None | Some(Value::Null) => Ok(None),
        Some(obj) => KeyValue::from(obj).map(Some),
    }
}

/// `/v3/kv/put`.
#[derive(Debug, Clone, Default)]
pub struct PutResponse {
    /// The response header.
    pub header: Header,
    /// The value that was there before, when `prev_kv` was asked for.
    pub prev_kv: Option<KeyValue>,
}

/// `/v3/kv/range`.
#[derive(Debug, Clone, Default)]
pub struct RangeResponse {
    /// The response header.
    pub header: Header,
    /// The pairs in the range, in key order unless a sort was asked for.
    pub kvs: Vec<KeyValue>,
    /// True when `limit` cut the answer short.
    pub more: bool,
    /// How many keys the range holds, ignoring `limit`.
    pub count: i64,
}

impl RangeResponse {
    /// The value of the single key a point read asked for.
    pub fn one(&self) -> Option<&KeyValue> {
        self.kvs.first()
    }

    /// The keys, as text, in the order they arrived.
    pub fn keys(&self) -> Vec<String> {
        self.kvs.iter().map(KeyValue::key_str).collect()
    }

    /// The values, as text, in the order they arrived.
    pub fn values(&self) -> Vec<String> {
        self.kvs.iter().map(KeyValue::value_str).collect()
    }
}

/// `/v3/kv/deleterange`.
#[derive(Debug, Clone, Default)]
pub struct DeleteRangeResponse {
    /// The response header.
    pub header: Header,
    /// How many keys were removed.
    pub deleted: i64,
    /// What was removed, when `prev_kv` was asked for.
    pub prev_kvs: Vec<KeyValue>,
}

/// One entry of a transaction's `responses`.
#[derive(Debug, Clone)]
pub enum TxnOpResponse {
    /// A nested `requestPut`.
    Put(PutResponse),
    /// A nested `requestRange`.
    Range(RangeResponse),
    /// A nested `requestDeleteRange`.
    Delete(DeleteRangeResponse),
    /// A nested `requestTxn`.
    Txn(Box<TxnResponse>),
    /// Something the harness could not classify; kept so a failure can print it.
    Unknown(Value),
}

/// `/v3/kv/txn`.
#[derive(Debug, Clone, Default)]
pub struct TxnResponse {
    /// The response header.
    pub header: Header,
    /// True when every comparison held and the `success` branch ran.
    pub succeeded: bool,
    /// One entry per operation of the branch that ran.
    pub responses: Vec<TxnOpResponse>,
}

impl TxnResponse {
    /// The nested range response at `index`, when that is what ran there.
    pub fn range(&self, index: usize) -> Option<&RangeResponse> {
        match self.responses.get(index) {
            Some(TxnOpResponse::Range(r)) => Some(r),
            _ => None,
        }
    }

    /// The nested put response at `index`, when that is what ran there.
    pub fn put(&self, index: usize) -> Option<&PutResponse> {
        match self.responses.get(index) {
            Some(TxnOpResponse::Put(r)) => Some(r),
            _ => None,
        }
    }

    /// The nested delete response at `index`, when that is what ran there.
    pub fn delete(&self, index: usize) -> Option<&DeleteRangeResponse> {
        match self.responses.get(index) {
            Some(TxnOpResponse::Delete(r)) => Some(r),
            _ => None,
        }
    }
}

/// `/v3/lease/grant` and `/v3/lease/keepalive`.
#[derive(Debug, Clone, Default)]
pub struct LeaseResponse {
    /// The response header.
    pub header: Header,
    /// The lease id.
    pub id: i64,
    /// The TTL the server granted, in seconds; 0 or -1 when the lease is gone.
    pub ttl: i64,
    /// The `error` field a keepalive for a dead lease carries.
    pub error: String,
}

/// `/v3/maintenance/status`.
#[derive(Debug, Clone, Default)]
pub struct StatusResponse {
    /// The response header.
    pub header: Header,
    /// The server's own version string.
    pub version: String,
    /// The member id this member believes is the leader, 0 when there is none.
    pub leader: u64,
    /// The last raft index this member has.
    pub raft_index: u64,
    /// The raft term this member is in.
    pub raft_term: u64,
    /// The last raft index this member has applied.
    pub raft_applied_index: u64,
    /// Whatever the member wants to complain about.
    pub errors: Vec<String>,
    /// The size of the backing store in bytes.
    pub db_size: i64,
}

/// One member of `/v3/cluster/member/list`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Member {
    /// The member id.
    pub id: u64,
    /// The member name, empty until the member has joined.
    pub name: String,
    /// The URLs peers reach this member on.
    pub peer_urls: Vec<String>,
    /// The URLs clients reach this member on.
    pub client_urls: Vec<String>,
    /// True for a member that votes on nothing.
    pub is_learner: bool,
}

/// `/v3/cluster/member/*`.
#[derive(Debug, Clone, Default)]
pub struct MemberListResponse {
    /// The response header.
    pub header: Header,
    /// Every member the cluster knows about.
    pub members: Vec<Member>,
}

/// One event of a watch stream.
#[derive(Debug, Clone)]
pub struct Event {
    /// `PUT` or `DELETE`; a missing `type` is a put, because `PUT` is the zero value.
    pub kind: String,
    /// The key as it is after the event.
    pub kv: KeyValue,
    /// The key as it was before, when `prev_kv` was asked for.
    pub prev_kv: Option<KeyValue>,
}

/// One message of a watch stream.
#[derive(Debug, Clone, Default)]
pub struct WatchResponse {
    /// The response header.
    pub header: Header,
    /// The id of the watch this message belongs to.
    pub watch_id: i64,
    /// True for the message that acknowledges the create request.
    pub created: bool,
    /// True for the message that acknowledges a cancel, or reports a broken watch.
    pub canceled: bool,
    /// Set when the watch could not start because the revision was compacted away.
    pub compact_revision: i64,
    /// Why the watch was cancelled.
    pub cancel_reason: String,
    /// The events, in revision order.
    pub events: Vec<Event>,
}

// ---------------------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------------------

/// A `/v3/kv/range` request, built field by field.
#[derive(Debug, Clone, Default)]
pub struct RangeRequest(pub serde_json::Map<String, Value>);

impl RangeRequest {
    /// A point read of one key.
    pub fn key(key: &[u8]) -> RangeRequest {
        let mut m = serde_json::Map::new();
        m.insert("key".into(), json!(b64(key)));
        RangeRequest(m)
    }

    /// A half-open range `[key, end)`.
    pub fn range(key: &[u8], end: &[u8]) -> RangeRequest {
        let mut r = RangeRequest::key(key);
        r.0.insert("range_end".into(), json!(b64(end)));
        r
    }

    /// Everything under a prefix.
    pub fn prefix(prefix: &[u8]) -> RangeRequest {
        RangeRequest::range(prefix, &prefix_end(prefix))
    }

    /// Every key in the store.
    pub fn all() -> RangeRequest {
        RangeRequest::range(ZERO_KEY, ZERO_KEY)
    }

    /// Set any field by name, for the options a test wants to spell out.
    pub fn with(mut self, field: &str, value: Value) -> RangeRequest {
        self.0.insert(field.to_string(), value);
        self
    }

    /// Read the range as it was at `revision`.
    pub fn at_revision(self, revision: i64) -> RangeRequest {
        self.with("revision", json!(revision.to_string()))
    }

    /// Return at most `n` pairs.
    pub fn limit(self, n: i64) -> RangeRequest {
        self.with("limit", json!(n.to_string()))
    }

    /// Return only the count.
    pub fn count_only(self) -> RangeRequest {
        self.with("count_only", json!(true))
    }

    /// Return keys without their values.
    pub fn keys_only(self) -> RangeRequest {
        self.with("keys_only", json!(true))
    }

    /// Sort the answer, e.g. `("DESCEND", "KEY")`.
    pub fn sort(self, order: &str, target: &str) -> RangeRequest {
        self.with("sort_order", json!(order))
            .with("sort_target", json!(target))
    }

    /// The JSON body this request sends.
    pub fn body(&self) -> Value {
        Value::Object(self.0.clone())
    }
}

/// A `/v3/kv/put` request.
#[derive(Debug, Clone, Default)]
pub struct PutRequest(pub serde_json::Map<String, Value>);

impl PutRequest {
    /// Put `value` at `key`.
    pub fn new(key: &[u8], value: &[u8]) -> PutRequest {
        let mut m = serde_json::Map::new();
        m.insert("key".into(), json!(b64(key)));
        m.insert("value".into(), json!(b64(value)));
        PutRequest(m)
    }

    /// Set any field by name.
    pub fn with(mut self, field: &str, value: Value) -> PutRequest {
        self.0.insert(field.to_string(), value);
        self
    }

    /// Attach the key to a lease.
    pub fn lease(self, id: i64) -> PutRequest {
        self.with("lease", json!(id.to_string()))
    }

    /// Ask for the value that was there before.
    pub fn prev_kv(self) -> PutRequest {
        self.with("prev_kv", json!(true))
    }

    /// The JSON body this request sends.
    pub fn body(&self) -> Value {
        Value::Object(self.0.clone())
    }
}

/// Build one comparison of a transaction.
pub fn compare(key: &[u8], target: &str, result: &str, field_name: &str, value: Value) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("key".into(), json!(b64(key)));
    m.insert("target".into(), json!(target));
    m.insert("result".into(), json!(result));
    if !field_name.is_empty() {
        m.insert(field_name.to_string(), value);
    }
    Value::Object(m)
}

/// `VALUE` equal to `value`.
pub fn cmp_value_eq(key: &[u8], value: &[u8]) -> Value {
    compare(key, "VALUE", "EQUAL", "value", json!(b64(value)))
}

/// `VERSION` equal to `version`; version 0 means "the key does not exist".
pub fn cmp_version_eq(key: &[u8], version: i64) -> Value {
    compare(
        key,
        "VERSION",
        "EQUAL",
        "version",
        json!(version.to_string()),
    )
}

/// `CREATE` revision equal to `revision`; 0 means "never created".
pub fn cmp_create_eq(key: &[u8], revision: i64) -> Value {
    compare(
        key,
        "CREATE",
        "EQUAL",
        "create_revision",
        json!(revision.to_string()),
    )
}

/// `MOD` revision equal to `revision`.
pub fn cmp_mod_eq(key: &[u8], revision: i64) -> Value {
    compare(
        key,
        "MOD",
        "EQUAL",
        "mod_revision",
        json!(revision.to_string()),
    )
}

/// A `requestPut` operation inside a transaction.
pub fn op_put(key: &[u8], value: &[u8]) -> Value {
    json!({ "requestPut": PutRequest::new(key, value).body() })
}

/// A `requestRange` operation inside a transaction.
pub fn op_range(key: &[u8]) -> Value {
    json!({ "requestRange": RangeRequest::key(key).body() })
}

/// A `requestDeleteRange` operation inside a transaction.
pub fn op_delete(key: &[u8]) -> Value {
    json!({ "requestDeleteRange": { "key": b64(key) } })
}

// ---------------------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------------------

/// A client bound to one member's client URL.
pub struct Client {
    /// The base URL, `http://127.0.0.1:<port>`.
    pub url: String,
    /// The member name this client talks to, for report lines.
    pub member: String,
    http: Http,
}

impl Client {
    /// Build a client for one member.
    pub fn new(url: &str, member: &str, timeout: Duration) -> Result<Client, EtcdError> {
        Ok(Client {
            url: url.to_string(),
            member: member.to_string(),
            http: Http::new(url, timeout).map_err(EtcdError::Transport)?,
        })
    }

    /// Forget every pooled connection, after the node was killed or partitioned.
    pub async fn reset(&self) {
        self.http.reset().await;
    }

    /// The per-request deadline this client uses.
    pub fn timeout(&self) -> Duration {
        self.http.timeout
    }

    /// `POST` a body and hand back the raw response, whatever the status.
    pub async fn post_raw(&self, path: &str, body: &Value) -> Result<HttpResponse, EtcdError> {
        let text = serde_json::to_vec(body).unwrap_or_default();
        self.http
            .request("POST", path, Some(&text))
            .await
            .map_err(EtcdError::Transport)
    }

    /// `POST` a body that is not necessarily valid JSON, for the error stages.
    pub async fn post_bytes(&self, path: &str, body: &[u8]) -> Result<HttpResponse, EtcdError> {
        self.http
            .request("POST", path, Some(body))
            .await
            .map_err(EtcdError::Transport)
    }

    /// `GET` a path, for `/version` and `/health`.
    pub async fn get(&self, path: &str) -> Result<HttpResponse, EtcdError> {
        self.http
            .request("GET", path, None)
            .await
            .map_err(EtcdError::Transport)
    }

    /// `POST` a body and decode the answer as JSON, turning a non-2xx into an error.
    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, EtcdError> {
        let resp = self.post_raw(path, body).await?;
        let text = resp.text();
        if !(200..300).contains(&resp.status) {
            let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            return Err(EtcdError::Status {
                status: resp.status,
                code: parsed
                    .get("code")
                    .and_then(Value::as_i64)
                    .unwrap_or(-1),
                message: parsed
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or(&text)
                    .to_string(),
                body: text,
            });
        }
        serde_json::from_str(&text).map_err(|e| EtcdError::Decode {
            what: format!("the JSON body of {path} ({e})"),
            body: text,
        })
    }

    fn decode<T>(what: &str, body: &Value, f: impl FnOnce(&Value) -> Result<T, String>) -> Result<T, EtcdError> {
        f(body).map_err(|e| EtcdError::Decode {
            what: format!("{what}: {e}"),
            body: body.to_string(),
        })
    }

    /// `/v3/kv/put` with every option spelled out.
    pub async fn put_req(&self, req: &PutRequest) -> Result<PutResponse, EtcdError> {
        let v = self.post("/v3/kv/put", &req.body()).await?;
        Client::decode("the put response", &v, |v| {
            Ok(PutResponse {
                header: Header::from(v),
                prev_kv: kv_opt(v, "prev_kv")?,
            })
        })
    }

    /// The common case: put a value at a key.
    pub async fn put(&self, key: &[u8], value: &[u8]) -> Result<PutResponse, EtcdError> {
        self.put_req(&PutRequest::new(key, value)).await
    }

    /// `/v3/kv/range` with every option spelled out.
    pub async fn range_req(&self, req: &RangeRequest) -> Result<RangeResponse, EtcdError> {
        let v = self.post("/v3/kv/range", &req.body()).await?;
        Client::decode("the range response", &v, |v| {
            Ok(RangeResponse {
                header: Header::from(v),
                kvs: kv_list(v, "kvs")?,
                more: as_bool(v, "more"),
                count: as_i64(v, "count"),
            })
        })
    }

    /// The common case: read one key.
    pub async fn get_key(&self, key: &[u8]) -> Result<RangeResponse, EtcdError> {
        self.range_req(&RangeRequest::key(key)).await
    }

    /// Read one key and hand back its value, or `None` when it is not there.
    pub async fn value_of(&self, key: &[u8]) -> Result<Option<Vec<u8>>, EtcdError> {
        Ok(self.get_key(key).await?.one().map(|kv| kv.value.clone()))
    }

    /// `/v3/kv/deleterange`.
    pub async fn delete_req(&self, body: &Value) -> Result<DeleteRangeResponse, EtcdError> {
        let v = self.post("/v3/kv/deleterange", body).await?;
        Client::decode("the delete response", &v, |v| {
            Ok(DeleteRangeResponse {
                header: Header::from(v),
                deleted: as_i64(v, "deleted"),
                prev_kvs: kv_list(v, "prev_kvs")?,
            })
        })
    }

    /// Delete one key.
    pub async fn delete(&self, key: &[u8]) -> Result<DeleteRangeResponse, EtcdError> {
        self.delete_req(&json!({ "key": b64(key) })).await
    }

    /// `/v3/kv/txn`.
    pub async fn txn(
        &self,
        compare: Vec<Value>,
        success: Vec<Value>,
        failure: Vec<Value>,
    ) -> Result<TxnResponse, EtcdError> {
        let body = json!({ "compare": compare, "success": success, "failure": failure });
        let v = self.post("/v3/kv/txn", &body).await?;
        Client::decode("the txn response", &v, |v| decode_txn(v))
    }

    /// `/v3/kv/compaction`.
    pub async fn compact(&self, revision: i64, physical: bool) -> Result<Header, EtcdError> {
        let body = json!({ "revision": revision.to_string(), "physical": physical });
        let v = self.post("/v3/kv/compaction", &body).await?;
        Ok(Header::from(&v))
    }

    /// `/v3/lease/grant`.
    pub async fn lease_grant(&self, ttl_secs: i64) -> Result<LeaseResponse, EtcdError> {
        let v = self
            .post("/v3/lease/grant", &json!({ "TTL": ttl_secs.to_string(), "ID": "0" }))
            .await?;
        Ok(decode_lease(&v))
    }

    /// `/v3/lease/revoke`.
    pub async fn lease_revoke(&self, id: i64) -> Result<Header, EtcdError> {
        let v = self
            .post("/v3/lease/revoke", &json!({ "ID": id.to_string() }))
            .await?;
        Ok(Header::from(&v))
    }

    /// `/v3/lease/keepalive`, which is a stream carrying exactly one useful message.
    pub async fn lease_keepalive(&self, id: i64) -> Result<LeaseResponse, EtcdError> {
        let body = serde_json::to_vec(&json!({ "ID": id.to_string() })).unwrap_or_default();
        let mut stream = self
            .http
            .open_stream("/v3/lease/keepalive", &body, self.http.timeout)
            .await
            .map_err(EtcdError::Transport)?;
        let line = stream
            .next_line(self.http.timeout)
            .await
            .map_err(EtcdError::Transport)?
            .ok_or_else(|| EtcdError::Decode {
                what: "a keepalive message".into(),
                body: stream.head.head(),
            })?;
        let v: Value = serde_json::from_str(&line).map_err(|e| EtcdError::Decode {
            what: format!("the keepalive message ({e})"),
            body: line.clone(),
        })?;
        // Streaming endpoints wrap every message in `{"result": ...}`.
        let inner = v.get("result").unwrap_or(&v);
        Ok(decode_lease(inner))
    }

    /// `/v3/maintenance/status`.
    pub async fn status(&self) -> Result<StatusResponse, EtcdError> {
        let v = self.post("/v3/maintenance/status", &json!({})).await?;
        Ok(StatusResponse {
            header: Header::from(&v),
            version: as_str(&v, "version"),
            leader: as_u64(&v, "leader"),
            raft_index: as_u64(&v, "raftIndex").max(as_u64(&v, "raft_index")),
            raft_term: as_u64(&v, "raftTerm").max(as_u64(&v, "raft_term")),
            raft_applied_index: as_u64(&v, "raftAppliedIndex").max(as_u64(&v, "raft_applied_index")),
            errors: v
                .get("errors")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|x| x.to_string()).collect())
                .unwrap_or_default(),
            db_size: as_i64(&v, "dbSize").max(as_i64(&v, "db_size")),
        })
    }

    /// `/v3/cluster/member/list`.
    pub async fn member_list(&self) -> Result<MemberListResponse, EtcdError> {
        let v = self.post("/v3/cluster/member/list", &json!({})).await?;
        Ok(decode_members(&v))
    }

    /// `/v3/cluster/member/add`.
    pub async fn member_add(&self, peer_urls: &[String]) -> Result<Value, EtcdError> {
        self.post(
            "/v3/cluster/member/add",
            &json!({ "peerURLs": peer_urls, "isLearner": false }),
        )
        .await
    }

    /// `/v3/cluster/member/remove`.
    pub async fn member_remove(&self, id: u64) -> Result<MemberListResponse, EtcdError> {
        let v = self
            .post("/v3/cluster/member/remove", &json!({ "ID": id.to_string() }))
            .await?;
        Ok(decode_members(&v))
    }

    /// `GET /version`.
    pub async fn version(&self) -> Result<Value, EtcdError> {
        let resp = self.get("/version").await?;
        serde_json::from_str(&resp.text()).map_err(|e| EtcdError::Decode {
            what: format!("the /version body ({e})"),
            body: resp.text(),
        })
    }

    /// `GET /health`.
    pub async fn health(&self) -> Result<Value, EtcdError> {
        let resp = self.get("/health").await?;
        serde_json::from_str(&resp.text()).map_err(|e| EtcdError::Decode {
            what: format!("the /health body ({e})"),
            body: resp.text(),
        })
    }

    /// Open a watch. Every message of the request is sent at once; the responses stream
    /// back until the caller drops the [`Watch`].
    pub async fn watch(&self, requests: &[Value]) -> Result<Watch, EtcdError> {
        let mut body = Vec::new();
        for r in requests {
            body.extend_from_slice(serde_json::to_string(r).unwrap_or_default().as_bytes());
            body.push(b'\n');
        }
        let stream = self
            .http
            .open_stream("/v3/watch", &body, self.http.timeout)
            .await
            .map_err(EtcdError::Transport)?;
        Ok(Watch { stream })
    }
}

/// An open watch stream.
pub struct Watch {
    stream: http::Stream,
}

impl Watch {
    /// The status line and headers the watch started with.
    pub fn head(&self) -> String {
        self.stream.head.head()
    }

    /// The next watch message, or `None` when nothing arrived inside `timeout`.
    pub async fn next(&mut self, timeout: Duration) -> Result<Option<WatchResponse>, EtcdError> {
        let Some(line) = self
            .stream
            .next_line(timeout)
            .await
            .map_err(EtcdError::Transport)?
        else {
            return Ok(None);
        };
        let v: Value = serde_json::from_str(&line).map_err(|e| EtcdError::Decode {
            what: format!("a watch message ({e})"),
            body: line.clone(),
        })?;
        let inner = v.get("result").unwrap_or(&v).clone();
        decode_watch(&inner)
            .map(Some)
            .map_err(|e| EtcdError::Decode {
                what: format!("a watch message: {e}"),
                body: line,
            })
    }

    /// Collect messages until `deadline` passes or `want` events have been seen.
    pub async fn collect_events(
        &mut self,
        want: usize,
        deadline: Duration,
    ) -> Result<Vec<Event>, EtcdError> {
        let start = std::time::Instant::now();
        let mut events = Vec::new();
        while events.len() < want && start.elapsed() < deadline {
            let left = deadline.saturating_sub(start.elapsed());
            match self.next(left).await? {
                Some(msg) => events.extend(msg.events),
                None => break,
            }
        }
        Ok(events)
    }
}

fn decode_watch(v: &Value) -> Result<WatchResponse, String> {
    let events = match v.get("events") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|e| {
                Ok(Event {
                    kind: e
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("PUT")
                        .to_string(),
                    kv: KeyValue::from(e.get("kv").unwrap_or(&Value::Null))?,
                    prev_kv: kv_opt(e, "prev_kv")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
        Some(other) => return Err(format!("events should be an array, got {other}")),
    };
    Ok(WatchResponse {
        header: Header::from(v),
        watch_id: as_i64(v, "watch_id"),
        created: as_bool(v, "created"),
        canceled: as_bool(v, "canceled"),
        compact_revision: as_i64(v, "compact_revision"),
        cancel_reason: as_str(v, "cancel_reason"),
        events,
    })
}

fn decode_lease(v: &Value) -> LeaseResponse {
    LeaseResponse {
        header: Header::from(v),
        id: as_i64(v, "ID").max(as_i64(v, "id")),
        ttl: as_i64(v, "TTL").max(as_i64(v, "ttl")),
        error: as_str(v, "error"),
    }
}

fn decode_members(v: &Value) -> MemberListResponse {
    let members = v
        .get("members")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|m| Member {
                    id: as_u64(m, "ID").max(as_u64(m, "id")),
                    name: as_str(m, "name"),
                    peer_urls: str_list(m, "peerURLs", "peer_urls"),
                    client_urls: str_list(m, "clientURLs", "client_urls"),
                    is_learner: as_bool(m, "isLearner") || as_bool(m, "is_learner"),
                })
                .collect()
        })
        .unwrap_or_default();
    MemberListResponse {
        header: Header::from(v),
        members,
    }
}

fn str_list(v: &Value, a: &str, b: &str) -> Vec<String> {
    v.get(a)
        .or_else(|| v.get(b))
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn decode_txn(v: &Value) -> Result<TxnResponse, String> {
    let responses = match field(v, "responses") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|op| {
                if let Some(p) = field(op, "response_put") {
                    Ok(TxnOpResponse::Put(PutResponse {
                        header: Header::from(p),
                        prev_kv: kv_opt(p, "prev_kv")?,
                    }))
                } else if let Some(r) = field(op, "response_range") {
                    Ok(TxnOpResponse::Range(RangeResponse {
                        header: Header::from(r),
                        kvs: kv_list(r, "kvs")?,
                        more: as_bool(r, "more"),
                        count: as_i64(r, "count"),
                    }))
                } else if let Some(d) = field(op, "response_delete_range") {
                    Ok(TxnOpResponse::Delete(DeleteRangeResponse {
                        header: Header::from(d),
                        deleted: as_i64(d, "deleted"),
                        prev_kvs: kv_list(d, "prev_kvs")?,
                    }))
                } else if let Some(t) = field(op, "response_txn") {
                    Ok(TxnOpResponse::Txn(Box::new(decode_txn(t)?)))
                } else {
                    Ok(TxnOpResponse::Unknown(op.clone()))
                }
            })
            .collect::<Result<Vec<_>, String>>()?,
        Some(other) => return Err(format!("responses should be an array, got {other}")),
    };
    Ok(TxnResponse {
        header: Header::from(v),
        succeeded: as_bool(v, "succeeded"),
        responses,
    })
}

/// A blocking readiness probe: `GET /health` answering anything at all is enough.
pub fn blocking_probe(base_url: &str) -> Result<(), String> {
    match http::blocking_get(base_url, "/health", Duration::from_millis(400)) {
        Ok((status, body)) => {
            if (200..500).contains(&status) {
                Ok(())
            } else {
                Err(format!("GET /health answered {status}: {}", short(&body)))
            }
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_and_rejects_rubbish() {
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(unb64("Zm9v").expect("decode"), b"foo");
        assert!(unb64("!!!").is_err());
    }

    #[test]
    fn prefix_end_increments_the_last_byte() {
        assert_eq!(prefix_end(b"k"), b"l".to_vec());
        assert_eq!(prefix_end(b"ab"), b"ac".to_vec());
        assert_eq!(prefix_end(&[0x61, 0xff]), vec![0x62]);
        assert_eq!(prefix_end(&[0xff, 0xff]), vec![0]);
        assert_eq!(prefix_end(b""), vec![0]);
    }

    #[test]
    fn integers_decode_from_strings_numbers_and_nothing() {
        let v = json!({"a": "7", "b": 7, "c": null});
        assert_eq!(as_i64(&v, "a"), 7);
        assert_eq!(as_i64(&v, "b"), 7);
        assert_eq!(as_i64(&v, "c"), 0);
        assert_eq!(as_i64(&v, "missing"), 0);
        assert_eq!(as_u64(&json!({"id":"15985099378392235387"}), "id"), 15_985_099_378_392_235_387);
    }

    #[test]
    fn a_header_with_everything_missing_is_all_zero() {
        assert_eq!(Header::from(&json!({})), Header::default());
        let h = Header::from(&json!({"header":{"revision":"4","raft_term":"2"}}));
        assert_eq!(h.revision, 4);
        assert_eq!(h.raft_term, 2);
    }

    #[test]
    fn a_range_response_from_etcd_decodes() {
        let v = json!({
            "header": {"cluster_id":"1","member_id":"2","revision":"3","raft_term":"2"},
            "kvs": [{"key":"Zm9v","create_revision":"2","mod_revision":"3","version":"2","value":"YmF6"}],
            "count": "1"
        });
        let r = RangeResponse {
            header: Header::from(&v),
            kvs: kv_list(&v, "kvs").expect("kvs"),
            more: as_bool(&v, "more"),
            count: as_i64(&v, "count"),
        };
        assert_eq!(r.count, 1);
        assert!(!r.more);
        let kv = r.one().expect("one kv");
        assert_eq!(kv.key_str(), "foo");
        assert_eq!(kv.value_str(), "baz");
        assert_eq!(kv.version, 2);
        assert_eq!(kv.create_revision, 2);
    }

    #[test]
    fn camel_and_snake_spellings_both_resolve() {
        let snake = json!({"response_put": {"header": {"revision": "4"}}});
        let camel_case = json!({"responsePut": {"header": {"revision": "4"}}});
        assert!(field(&snake, "response_put").is_some());
        assert!(field(&camel_case, "response_put").is_some());
        assert_eq!(camel("response_delete_range"), "responseDeleteRange");
    }

    #[test]
    fn a_txn_response_decodes_every_branch() {
        let v = json!({
            "header": {"revision": "13"},
            "responses": [
                {"response_range": {"header": {"revision":"12"}, "kvs": [{"key":"bm9wZQ=="}], "count":"1"}},
                {"response_delete_range": {"header": {"revision":"13"}, "deleted":"1"}}
            ]
        });
        let t = decode_txn(&v).expect("decode");
        assert!(!t.succeeded, "a missing `succeeded` is false");
        assert_eq!(t.range(0).map(|r| r.count), Some(1));
        assert_eq!(t.delete(1).map(|d| d.deleted), Some(1));
        assert!(t.put(0).is_none());
    }

    #[test]
    fn a_watch_event_without_a_type_is_a_put() {
        let v = json!({"header":{"revision":"9"},"events":[{"kv":{"key":"a3c=","value":"djE="}}]});
        let w = decode_watch(&v).expect("decode");
        assert_eq!(w.events[0].kind, "PUT");
        assert_eq!(w.events[0].kv.key_str(), "kw");
        let v = json!({"events":[{"type":"DELETE","kv":{"key":"a3c=","mod_revision":"10"}}]});
        assert_eq!(decode_watch(&v).expect("decode").events[0].kind, "DELETE");
    }

    #[test]
    fn range_requests_build_the_documented_body() {
        let b = RangeRequest::prefix(b"k").limit(2).sort("DESCEND", "KEY").body();
        assert_eq!(b["key"], json!("aw=="));
        assert_eq!(b["range_end"], json!("bA=="));
        assert_eq!(b["limit"], json!("2"));
        assert_eq!(b["sort_order"], json!("DESCEND"));
        assert_eq!(RangeRequest::all().body()["range_end"], json!("AA=="));
    }

    #[test]
    fn comparisons_name_their_own_field() {
        let c = cmp_version_eq(b"k", 0);
        assert_eq!(c["target"], json!("VERSION"));
        assert_eq!(c["result"], json!("EQUAL"));
        assert_eq!(c["version"], json!("0"));
        assert_eq!(cmp_create_eq(b"k", 3)["create_revision"], json!("3"));
        assert_eq!(cmp_mod_eq(b"k", 3)["mod_revision"], json!("3"));
        assert_eq!(cmp_value_eq(b"k", b"v")["value"], json!("dg=="));
    }
}
