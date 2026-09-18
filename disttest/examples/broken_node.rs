//! `broken_node` — a node that is wrong on purpose.
//!
//! It exists so the suite's red output can be demonstrated on something real, the way
//! `reference_primitives` exists so the green output can be. It speaks enough of the etcd
//! v3 JSON subset to get through the first node stages, and then it is wrong in one
//! specific, famous way: **it acknowledges a write before the write is durable.** The store
//! lives in memory and is only ever written to disk on a lazy timer, so a `SIGKILL` between
//! the acknowledgement and the flush loses an acknowledged write — exactly what stage 35
//! looks for, and what stages 44 and 50 look for in a cluster.
//!
//! It is also not a cluster: it ignores `--initial-cluster` entirely and always claims to be
//! its own leader, so the cluster ladder fails against it too, loudly.
//!
//! ```text
//! disttest --target broken_node --stage 35
//! disttest --target broken_node --until 24     # these mostly pass
//! ```

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

/// One stored pair, with the revision bookkeeping the API reports.
#[derive(Clone, Debug, Default)]
struct Pair {
    value: Vec<u8>,
    create_revision: i64,
    mod_revision: i64,
    version: i64,
}

#[derive(Default)]
struct Store {
    kv: BTreeMap<Vec<u8>, Pair>,
    /// Everything written since the last flush. The bug lives here: the server answers
    /// before this has reached the disk.
    dirty: bool,
}

struct Server {
    store: Mutex<Store>,
    revision: AtomicI64,
    data_dir: PathBuf,
    member_id: u64,
    cluster_id: u64,
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let v = TABLE.iter().position(|t| *t == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn b64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

impl Server {
    fn header(&self) -> Value {
        json!({
            "cluster_id": self.cluster_id.to_string(),
            "member_id": self.member_id.to_string(),
            "revision": self.revision.load(Ordering::SeqCst).to_string(),
            "raft_term": "2",
        })
    }

    fn key_of(&self, body: &Value, field: &str) -> Option<Vec<u8>> {
        b64_decode(body.get(field)?.as_str()?)
    }

    fn handle(&self, path: &str, body: &Value) -> (u16, Value) {
        match path {
            "/v3/kv/put" => {
                let (Some(key), Some(value)) =
                    (self.key_of(body, "key"), self.key_of(body, "value"))
                else {
                    return (400, json!({"code": 3, "message": "bad key or value"}));
                };
                let rev = self.revision.fetch_add(1, Ordering::SeqCst) + 1;
                let mut store = match self.store.lock() {
                    Ok(s) => s,
                    Err(e) => e.into_inner(),
                };
                let prev = store.kv.get(&key).cloned();
                let pair = Pair {
                    value,
                    create_revision: prev.as_ref().map(|p| p.create_revision).unwrap_or(rev),
                    mod_revision: rev,
                    version: prev.as_ref().map(|p| p.version).unwrap_or(0) + 1,
                };
                store.kv.insert(key.clone(), pair);
                store.dirty = true;
                // …and answer. Nothing has been written to disk. This is the bug.
                let mut out = json!({ "header": self.header() });
                if body.get("prev_kv").and_then(Value::as_bool) == Some(true) {
                    if let Some(p) = prev {
                        out["prev_kv"] = kv_json(&key, &p);
                    }
                }
                (200, out)
            }
            "/v3/kv/range" => {
                let Some(key) = self.key_of(body, "key") else {
                    return (400, json!({"code": 3, "message": "bad key"}));
                };
                let end = self.key_of(body, "range_end");
                let store = match self.store.lock() {
                    Ok(s) => s,
                    Err(e) => e.into_inner(),
                };
                let mut kvs = Vec::new();
                for (k, p) in store.kv.iter() {
                    let hit = match &end {
                        None => *k == key,
                        Some(e) if e == &vec![0u8] => key == vec![0u8] || *k >= key,
                        Some(e) => *k >= key && *k < *e,
                    };
                    if hit {
                        kvs.push(kv_json(k, p));
                    }
                }
                let count = kvs.len();
                let mut out = json!({ "header": self.header(), "count": count.to_string() });
                if count > 0 {
                    out["kvs"] = Value::Array(kvs);
                }
                (200, out)
            }
            "/v3/kv/deleterange" => {
                let Some(key) = self.key_of(body, "key") else {
                    return (400, json!({"code": 3, "message": "bad key"}));
                };
                let end = self.key_of(body, "range_end");
                let mut store = match self.store.lock() {
                    Ok(s) => s,
                    Err(e) => e.into_inner(),
                };
                let doomed: Vec<Vec<u8>> = store
                    .kv
                    .keys()
                    .filter(|k| match &end {
                        None => **k == key,
                        Some(e) if e == &vec![0u8] => **k >= key,
                        Some(e) => **k >= key && **k < *e,
                    })
                    .cloned()
                    .collect();
                for k in &doomed {
                    store.kv.remove(k);
                }
                if !doomed.is_empty() {
                    self.revision.fetch_add(1, Ordering::SeqCst);
                    store.dirty = true;
                }
                (
                    200,
                    json!({ "header": self.header(), "deleted": doomed.len().to_string() }),
                )
            }
            "/v3/kv/txn" => {
                // Enough of a transaction to get past stage 28: EQUAL comparisons over the
                // four targets, and the three operations, applied one after another. There
                // is nothing atomic about it, which is a second lie this node tells.
                let empty = Vec::new();
                let compares = body
                    .get("compare")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty);
                let mut succeeded = true;
                for cmp in compares {
                    let Some(key) = self.key_of(cmp, "key") else {
                        return (400, json!({"code": 3, "message": "bad compare key"}));
                    };
                    let store = match self.store.lock() {
                        Ok(s) => s,
                        Err(e) => e.into_inner(),
                    };
                    let pair = store.kv.get(&key).cloned().unwrap_or_default();
                    let target = cmp.get("target").and_then(Value::as_str).unwrap_or("VALUE");
                    let want_num = |field: &str| -> i64 {
                        match cmp.get(field) {
                            Some(Value::String(s)) => s.parse().unwrap_or(0),
                            Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
                            _ => 0,
                        }
                    };
                    let held = match target {
                        "VERSION" => pair.version == want_num("version"),
                        "CREATE" => pair.create_revision == want_num("create_revision"),
                        "MOD" => pair.mod_revision == want_num("mod_revision"),
                        _ => {
                            let want = self.key_of(cmp, "value").unwrap_or_default();
                            store.kv.contains_key(&key) && pair.value == want
                        }
                    };
                    drop(store);
                    if !held {
                        succeeded = false;
                        break;
                    }
                }
                let branch = if succeeded { "success" } else { "failure" };
                let ops = body
                    .get(branch)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut responses = Vec::new();
                for op in ops {
                    let (path, field) = if op.get("requestPut").is_some() {
                        ("/v3/kv/put", "response_put")
                    } else if op.get("requestRange").is_some() {
                        ("/v3/kv/range", "response_range")
                    } else if op.get("requestDeleteRange").is_some() {
                        ("/v3/kv/deleterange", "response_delete_range")
                    } else {
                        continue;
                    };
                    let inner = op
                        .get("requestPut")
                        .or_else(|| op.get("requestRange"))
                        .or_else(|| op.get("requestDeleteRange"))
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let (_, answer) = self.handle(path, &inner);
                    responses.push(json!({ field: answer }));
                }
                let mut out = json!({ "header": self.header(), "responses": responses });
                if succeeded {
                    out["succeeded"] = json!(true);
                }
                (200, out)
            }
            "/v3/maintenance/status" => (
                200,
                json!({
                    "header": self.header(),
                    "version": "0.0.1-broken",
                    "leader": self.member_id.to_string(),
                    "raftTerm": "2",
                    "raftIndex": "1",
                    "raftAppliedIndex": "1",
                    "dbSize": "4096",
                }),
            ),
            "/v3/cluster/member/list" => (
                200,
                json!({
                    "header": self.header(),
                    "members": [{
                        "ID": self.member_id.to_string(),
                        "name": "m1",
                        "peerURLs": [],
                        "clientURLs": [],
                    }],
                }),
            ),
            _ => (404, json!({"code": 12, "message": "not implemented"})),
        }
    }

    /// The lazy flush that makes the acknowledgement a lie.
    fn flush(&self) {
        let mut store = match self.store.lock() {
            Ok(s) => s,
            Err(e) => e.into_inner(),
        };
        if !store.dirty {
            return;
        }
        let mut text = String::new();
        for (k, p) in store.kv.iter() {
            text.push_str(&format!(
                "{} {} {} {} {}\n",
                b64_encode(k),
                b64_encode(&p.value),
                p.create_revision,
                p.mod_revision,
                p.version
            ));
        }
        let _ = std::fs::create_dir_all(&self.data_dir);
        let _ = std::fs::write(self.data_dir.join("store.txt"), text);
        store.dirty = false;
    }

    fn load(&self) {
        let Ok(text) = std::fs::read_to_string(self.data_dir.join("store.txt")) else {
            return;
        };
        let mut store = match self.store.lock() {
            Ok(s) => s,
            Err(e) => e.into_inner(),
        };
        let mut highest = 0;
        for line in text.lines() {
            let f: Vec<&str> = line.split(' ').collect();
            if f.len() != 5 {
                continue;
            }
            let (Some(k), Some(v)) = (b64_decode(f[0]), b64_decode(f[1])) else {
                continue;
            };
            let pair = Pair {
                value: v,
                create_revision: f[2].parse().unwrap_or(1),
                mod_revision: f[3].parse().unwrap_or(1),
                version: f[4].parse().unwrap_or(1),
            };
            highest = highest.max(pair.mod_revision);
            store.kv.insert(k, pair);
        }
        self.revision.store(highest.max(1), Ordering::SeqCst);
    }
}

fn kv_json(key: &[u8], p: &Pair) -> Value {
    json!({
        "key": b64_encode(key),
        "value": b64_encode(&p.value),
        "create_revision": p.create_revision.to_string(),
        "mod_revision": p.mod_revision.to_string(),
        "version": p.version.to_string(),
    })
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn port_of(url: &str) -> Option<u16> {
    url.rsplit(':').next()?.parse().ok()
}

fn serve(server: Arc<Server>, mut stream: TcpStream) {
    let peer = stream.try_clone();
    let mut reader = BufReader::new(match peer {
        Ok(s) => s,
        Err(_) => return,
    });
    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        let mut length = 0usize;
        let mut close = false;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                return;
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            let lower = header.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                length = v.trim().parse().unwrap_or(0);
            }
            if lower.starts_with("connection:") && lower.contains("close") {
                close = true;
            }
        }
        let mut body_bytes = vec![0u8; length];
        if length > 0 && reader.read_exact(&mut body_bytes).is_err() {
            return;
        }
        let (status, body) = if method == "GET" && path == "/version" {
            (
                200,
                json!({"etcdserver": "0.0.1-broken", "etcdcluster": "3.7.0"}),
            )
        } else if method == "GET" && path == "/health" {
            (200, json!({"health": "true"}))
        } else if method == "POST" {
            match serde_json::from_slice::<Value>(&body_bytes) {
                Ok(v) => server.handle(&path, &v),
                Err(e) => (400, json!({"code": 3, "message": e.to_string()})),
            }
        } else {
            (501, json!({"code": 12, "message": "not implemented"}))
        };
        let text = body.to_string();
        let response = format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{text}",
            if status == 200 { "OK" } else { "Bad Request" },
            text.len()
        );
        if stream.write_all(response.as_bytes()).is_err() || stream.flush().is_err() {
            return;
        }
        if close {
            return;
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let client_url = flag(&args, "--listen-client-urls").unwrap_or_default();
    let Some(port) = port_of(&client_url) else {
        eprintln!("broken_node: --listen-client-urls is required");
        std::process::exit(2);
    };
    let data_dir = PathBuf::from(flag(&args, "--data-dir").unwrap_or_else(|| ".".into()));
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let server = Arc::new(Server {
        store: Mutex::new(Store::default()),
        revision: AtomicI64::new(1),
        data_dir,
        member_id: seed | 1,
        cluster_id: 0x0b00_b1e5,
    });
    server.load();
    // The lazy flush: five seconds of acknowledged writes can be lost at any moment.
    {
        let server = server.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            server.flush();
        });
    }
    let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) else {
        eprintln!("broken_node: cannot bind port {port}");
        std::process::exit(2);
    };
    eprintln!(
        "broken_node: listening on {client_url} (writes are acknowledged before they are durable)"
    );
    for stream in listener.incoming().flatten() {
        let server = server.clone();
        std::thread::spawn(move || serve(server, stream));
    }
}
