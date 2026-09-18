//! The fault injector: a userspace TCP proxy in front of every member's peer URL.
//!
//! Each member advertises a *proxy* address as its peer URL and listens for peer traffic on
//! a private one, so every byte the members exchange passes through this file. No
//! privileges, no `iptables`, no network namespaces — which is what lets the cluster ladder
//! run anywhere the learner's laptop runs.
//!
//! **Which member dialled?** A partition has to cut a *link*, and a proxy only ever sees the
//! destination. The harness recovers the source by asking the kernel: the accepted
//! connection's source port identifies a socket in `/proc/net/tcp`, the socket's inode
//! appears in exactly one process's `/proc/<pid>/fd`, and the harness started every one of
//! those processes, so it knows which member that is. The answer is cached per source port.
//! When the lookup fails — a process that has already gone, a system without `/proc` — the
//! connection is treated as coming from "some peer" and only destination-wide faults apply,
//! which is recorded in [`ProxyStats::unidentified`].
//!
//! **What can be injected.** Everything is expressed per unordered link, because both
//! directions of a pair of members share the same TCP connections:
//!
//! | fault | what the proxy does |
//! |---|---|
//! | `cut` | refuses new connections and closes open ones, in both directions |
//! | `delay` | holds every chunk for the given time before passing it on |
//! | `drop` | refuses a new connection with the given probability, so the link flaps |
//! | `duplicate` | in message mode, sends a parsed peer message twice |
//! | `reorder` | in message mode, holds a message back and sends it after the next one |
//!
//! Whether a connection is framed is decided when it is *accepted*, because starting to
//! frame a stream halfway through would split it in the wrong place. A stage that wants
//! duplication or reordering on a cluster that is already connected therefore cuts the links
//! first, so the peers redial and the new connections come up in message mode.
//!
//! Duplication and reordering are the only two that need framing: deleting or swapping
//! *bytes* of a TCP stream would corrupt it rather than model a network. In message mode the
//! proxy parses the dialer's direction as HTTP/1.1 requests — which is what a raft transport
//! over HTTP sends — and duplicates or swaps whole messages. A stream it cannot frame is
//! passed through untouched and counted in [`ProxyStats::unframed`].

use rand::{Rng, SeedableRng};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// A link between two members, stored with the lower index first.
pub type Link = (usize, usize);

/// Normalise a pair of member indices into a link.
pub fn link(a: usize, b: usize) -> Link {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// What is wrong with one link.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkFault {
    /// Nothing crosses this link at all.
    pub cut: bool,
    /// Every chunk is held this long before it is passed on.
    pub delay: Duration,
    /// Probability that a new connection is refused outright.
    pub drop_connection: f64,
    /// Probability that a framed message is sent twice.
    pub duplicate: f64,
    /// Probability that a framed message is held back and sent after the next one.
    pub reorder: f64,
}

impl LinkFault {
    /// True when this link needs the proxy to parse messages rather than bytes.
    pub fn message_mode(&self) -> bool {
        self.duplicate > 0.0 || self.reorder > 0.0
    }

    /// True when the link is completely healthy.
    pub fn is_clean(&self) -> bool {
        *self == LinkFault::default()
    }
}

/// Counters every proxy adds to, so a test can say what the injector actually did.
#[derive(Debug, Default)]
pub struct ProxyStats {
    /// Connections accepted by any proxy.
    pub accepted: AtomicU64,
    /// Connections refused or closed because of a fault.
    pub blocked: AtomicU64,
    /// Bytes forwarded in either direction.
    pub bytes: AtomicU64,
    /// Chunks that were held back by a delay.
    pub delayed: AtomicU64,
    /// Messages sent twice.
    pub duplicated: AtomicU64,
    /// Messages sent after the one that followed them.
    pub reordered: AtomicU64,
    /// Connections whose dialer could not be identified.
    pub unidentified: AtomicU64,
    /// Connections asked to be framed that turned out not to be HTTP.
    pub unframed: AtomicU64,
}

impl ProxyStats {
    /// A one-line summary for `ctx.note`.
    pub fn summary(&self) -> String {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        format!(
            "peer proxies: {} connections ({} blocked, {} unidentified), {} KiB forwarded, \
             {} chunks delayed, {} messages duplicated, {} reordered, {} streams unframed",
            g(&self.accepted),
            g(&self.blocked),
            g(&self.unidentified),
            g(&self.bytes) / 1024,
            g(&self.delayed),
            g(&self.duplicated),
            g(&self.reordered),
            g(&self.unframed),
        )
    }

    /// How many connections were refused or cut.
    pub fn blocked(&self) -> u64 {
        self.blocked.load(Ordering::Relaxed)
    }

    /// How many messages the proxy duplicated.
    pub fn duplicated(&self) -> u64 {
        self.duplicated.load(Ordering::Relaxed)
    }

    /// How many messages the proxy delivered out of order.
    pub fn reordered(&self) -> u64 {
        self.reordered.load(Ordering::Relaxed)
    }
}

/// The table every proxy reads before it forwards anything.
#[derive(Debug, Default)]
pub struct FaultTable {
    faults: Mutex<HashMap<Link, LinkFault>>,
    /// Bumped whenever the table changes, so open connections notice a new cut.
    pub changed: Notify,
}

impl FaultTable {
    /// The fault on one link, or a clean link.
    pub fn get(&self, a: usize, b: usize) -> LinkFault {
        self.faults
            .lock()
            .map(|f| f.get(&link(a, b)).cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    /// Replace the fault on one link.
    pub fn set(&self, a: usize, b: usize, fault: LinkFault) {
        if let Ok(mut f) = self.faults.lock() {
            if fault.is_clean() {
                f.remove(&link(a, b));
            } else {
                f.insert(link(a, b), fault);
            }
        }
        self.changed.notify_waiters();
    }

    /// Forget every fault.
    pub fn clear(&self) {
        if let Ok(mut f) = self.faults.lock() {
            f.clear();
        }
        self.changed.notify_waiters();
    }

    /// Every link that currently carries a fault, for the report.
    pub fn describe(&self) -> String {
        let Ok(f) = self.faults.lock() else {
            return "the fault table is poisoned".into();
        };
        if f.is_empty() {
            return "no faults".into();
        }
        let mut parts: Vec<String> = f
            .iter()
            .map(|((a, b), fault)| {
                let mut what = Vec::new();
                if fault.cut {
                    what.push("cut".to_string());
                }
                if !fault.delay.is_zero() {
                    what.push(format!("delay {} ms", fault.delay.as_millis()));
                }
                if fault.drop_connection > 0.0 {
                    what.push(format!("drop {:.0}%", fault.drop_connection * 100.0));
                }
                if fault.duplicate > 0.0 {
                    what.push(format!("duplicate {:.0}%", fault.duplicate * 100.0));
                }
                if fault.reorder > 0.0 {
                    what.push(format!("reorder {:.0}%", fault.reorder * 100.0));
                }
                format!("m{}–m{}: {}", a + 1, b + 1, what.join(", "))
            })
            .collect();
        parts.sort();
        parts.join("; ")
    }
}

/// Maps a connection's source port back to the member that opened it.
///
/// The cache is keyed on the *pair* of ports, not on the source alone: ephemeral ports are
/// recycled within seconds, and a stale entry would attribute a fresh connection to the
/// wrong member — which, on a cut link, means letting traffic through a partition. That is
/// the kind of bug that shows up as one flaky run in ten and is never reproduced.
#[derive(Debug, Default)]
pub struct SourceMap {
    /// Member index → the pids that member may own (the wrapper and its children).
    roots: Mutex<Vec<(usize, u32)>>,
    cache: Mutex<HashMap<(u16, u16), usize>>,
}

impl SourceMap {
    /// Tell the map which process a member was started as.
    pub fn register(&self, member: usize, pid: u32) {
        if let Ok(mut r) = self.roots.lock() {
            r.retain(|(m, _)| *m != member);
            r.push((member, pid));
        }
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
    }

    /// Forget every cached source port; used when a member is restarted.
    pub fn forget(&self) {
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
    }

    /// Which member dialled from `source`, if the kernel still knows.
    pub fn owner(&self, source: SocketAddr, dest: SocketAddr) -> Option<usize> {
        let key = (source.port(), dest.port());
        if let Ok(c) = self.cache.lock() {
            if let Some(m) = c.get(&key) {
                return Some(*m);
            }
        }
        let inode = socket_inode(source, dest)?;
        let roots = self.roots.lock().ok()?.clone();
        for (member, pid) in roots {
            if process_tree_owns(pid, inode, 0) {
                if let Ok(mut c) = self.cache.lock() {
                    c.insert(key, member);
                }
                return Some(member);
            }
        }
        None
    }
}

/// Find the inode of the TCP socket with this exact local/remote address pair.
fn socket_inode(local: SocketAddr, remote: SocketAddr) -> Option<u64> {
    let want_local = hex_addr(local);
    let want_remote = hex_addr(remote);
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 {
                continue;
            }
            if ends_with_addr(f[1], &want_local) && ends_with_addr(f[2], &want_remote) {
                return f[9].parse().ok();
            }
        }
    }
    None
}

/// `/proc/net/tcp` writes IPv4 addresses little-endian; only the port half is compared for
/// IPv6-mapped rows, which is why this keeps the `ADDRESS:PORT` tail.
fn hex_addr(a: SocketAddr) -> String {
    match a {
        SocketAddr::V4(v4) => {
            let o = v4.ip().octets();
            format!(
                "{:02X}{:02X}{:02X}{:02X}:{:04X}",
                o[3],
                o[2],
                o[1],
                o[0],
                v4.port()
            )
        }
        SocketAddr::V6(v6) => format!(":{:04X}", v6.port()),
    }
}

fn ends_with_addr(field: &str, want: &str) -> bool {
    if want.starts_with(':') {
        field.ends_with(want)
    } else {
        field.eq_ignore_ascii_case(want)
    }
}

/// True when `pid` or one of its descendants holds the socket with this inode.
fn process_tree_owns(pid: u32, inode: u64, depth: usize) -> bool {
    if depth > 4 {
        return false;
    }
    let want = format!("socket:[{inode}]");
    if let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) {
        for e in entries.flatten() {
            if let Ok(target) = std::fs::read_link(e.path()) {
                if target.to_string_lossy() == want {
                    return true;
                }
            }
        }
    }
    let children =
        std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap_or_default();
    children
        .split_whitespace()
        .filter_map(|c| c.parse::<u32>().ok())
        .any(|c| process_tree_owns(c, inode, depth + 1))
}

/// One proxy: everything arriving here is peer traffic destined for `member`.
pub struct PeerProxy {
    /// Which member this proxy fronts.
    pub member: usize,
    /// The address peers are told to use.
    pub listen: SocketAddr,
    /// The address the member really listens on.
    pub forward: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl PeerProxy {
    /// Start a proxy in front of one member's peer port.
    pub async fn start(
        member: usize,
        listen_port: u16,
        forward_port: u16,
        faults: Arc<FaultTable>,
        sources: Arc<SourceMap>,
        stats: Arc<ProxyStats>,
        seed: u64,
    ) -> std::io::Result<PeerProxy> {
        let listen: SocketAddr = format!("127.0.0.1:{listen_port}")
            .parse()
            .map_err(|e| std::io::Error::other(format!("{e}")))?;
        let forward: SocketAddr = format!("127.0.0.1:{forward_port}")
            .parse()
            .map_err(|e| std::io::Error::other(format!("{e}")))?;
        let listener = TcpListener::bind(listen).await?;
        let task = tokio::spawn(async move {
            let mut conn_id: u64 = 0;
            loop {
                let Ok((client, peer)) = listener.accept().await else {
                    return;
                };
                conn_id += 1;
                stats.accepted.fetch_add(1, Ordering::Relaxed);
                let faults = faults.clone();
                let sources = sources.clone();
                let stats = stats.clone();
                let seed = seed ^ conn_id.wrapping_mul(0x9e37_79b9_7f4a_7c15);
                tokio::spawn(async move {
                    handle(
                        client, peer, listen, forward, member, faults, sources, stats, seed,
                    )
                    .await;
                });
            }
        });
        Ok(PeerProxy {
            member,
            listen,
            forward,
            task,
        })
    }

    /// Stop accepting; open connections die with the runtime.
    pub fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for PeerProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle(
    client: TcpStream,
    peer: SocketAddr,
    listen: SocketAddr,
    forward: SocketAddr,
    member: usize,
    faults: Arc<FaultTable>,
    sources: Arc<SourceMap>,
    stats: Arc<ProxyStats>,
    seed: u64,
) {
    let source = sources.owner(peer, listen);
    if source.is_none() {
        stats.unidentified.fetch_add(1, Ordering::Relaxed);
    }
    let fault_now = || match source {
        Some(s) => faults.get(s, member),
        // An unidentified dialer only inherits the destination's own self-link, which the
        // cluster sets when a member is isolated from everyone.
        None => faults.get(member, member),
    };
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let f = fault_now();
    if f.cut || (f.drop_connection > 0.0 && rng.random::<f64>() < f.drop_connection) {
        stats.blocked.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let Ok(upstream) = TcpStream::connect(forward).await else {
        stats.blocked.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let _ = client.set_nodelay(true);
    let _ = upstream.set_nodelay(true);
    let (cr, cw) = client.into_split();
    let (ur, uw) = upstream.into_split();
    let framed = f.message_mode();
    let a = tokio::spawn(pump(
        cr,
        uw,
        faults.clone(),
        source,
        member,
        stats.clone(),
        framed,
        seed ^ 1,
    ));
    let b = tokio::spawn(pump(
        ur,
        cw,
        faults.clone(),
        source,
        member,
        stats.clone(),
        false,
        seed ^ 2,
    ));
    let _ = a.await;
    b.abort();
}

/// Copy one direction, applying whatever the link's fault says.
#[allow(clippy::too_many_arguments)]
async fn pump(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    faults: Arc<FaultTable>,
    source: Option<usize>,
    member: usize,
    stats: Arc<ProxyStats>,
    framed: bool,
    seed: u64,
) {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut buf = vec![0u8; 32 * 1024];
    let mut pending: Vec<u8> = Vec::new();
    let mut held: Option<Vec<u8>> = None;
    let mut unframed_reported = false;
    loop {
        let fault = match source {
            Some(s) => faults.get(s, member),
            None => faults.get(member, member),
        };
        if fault.cut {
            stats.blocked.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let n = tokio::select! {
            r = from.read(&mut buf) => match r {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            },
            _ = faults.changed.notified() => continue,
        };
        stats.bytes.fetch_add(n as u64, Ordering::Relaxed);
        if !fault.delay.is_zero() {
            stats.delayed.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(fault.delay).await;
            if faults.get(source.unwrap_or(member), member).cut {
                return;
            }
        }
        if !framed {
            if to.write_all(&buf[..n]).await.is_err() {
                break;
            }
            continue;
        }
        pending.extend_from_slice(&buf[..n]);
        loop {
            let Some(len) = http_message_len(&pending) else {
                if pending.len() > 1 << 20 && !unframed_reported {
                    stats.unframed.fetch_add(1, Ordering::Relaxed);
                    unframed_reported = true;
                    if to.write_all(&pending).await.is_err() {
                        return;
                    }
                    pending.clear();
                }
                break;
            };
            let msg: Vec<u8> = pending.drain(..len).collect();
            if let Some(earlier) = held.take() {
                // The held message goes out *after* this one: that is the reordering.
                if to.write_all(&msg).await.is_err() {
                    return;
                }
                if to.write_all(&earlier).await.is_err() {
                    return;
                }
                continue;
            }
            if fault.reorder > 0.0 && rng.random::<f64>() < fault.reorder {
                stats.reordered.fetch_add(1, Ordering::Relaxed);
                held = Some(msg);
                continue;
            }
            if to.write_all(&msg).await.is_err() {
                return;
            }
            if fault.duplicate > 0.0 && rng.random::<f64>() < fault.duplicate {
                stats.duplicated.fetch_add(1, Ordering::Relaxed);
                if to.write_all(&msg).await.is_err() {
                    return;
                }
            }
        }
    }
    if let Some(earlier) = held.take() {
        let _ = to.write_all(&earlier).await;
    }
    if !pending.is_empty() {
        let _ = to.write_all(&pending).await;
    }
    let _ = to.flush().await;
}

/// The length of the first complete HTTP/1.1 message in `buf`, when there is one.
///
/// Only the two framings a peer transport uses are understood: a request with a
/// `Content-Length`, and a request with no body at all. A `Transfer-Encoding: chunked`
/// body has no length the proxy can know in advance, so such a stream is left unframed.
pub fn http_message_len(buf: &[u8]) -> Option<usize> {
    let head_end = find(buf, b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
    if head.contains("transfer-encoding:") {
        return None;
    }
    let len: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    let total = head_end + len;
    (buf.len() >= total).then_some(total)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Members a partition puts on the same side.
pub fn partition_links(groups: &[Vec<usize>]) -> HashSet<Link> {
    let mut cut = HashSet::new();
    for (i, g) in groups.iter().enumerate() {
        for other in groups.iter().skip(i + 1) {
            for a in g {
                for b in other {
                    cut.insert(link(*a, *b));
                }
            }
        }
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_unordered() {
        assert_eq!(link(2, 0), (0, 2));
        assert_eq!(link(0, 2), (0, 2));
    }

    #[test]
    fn a_partition_cuts_exactly_the_crossing_links() {
        let cut = partition_links(&[vec![0], vec![1, 2]]);
        assert_eq!(cut.len(), 2);
        assert!(cut.contains(&(0, 1)));
        assert!(cut.contains(&(0, 2)));
        assert!(!cut.contains(&(1, 2)), "the majority side stays connected");
    }

    #[test]
    fn the_fault_table_reports_what_is_set() {
        let t = FaultTable::default();
        assert_eq!(t.describe(), "no faults");
        t.set(
            1,
            0,
            LinkFault {
                cut: true,
                ..Default::default()
            },
        );
        assert!(t.get(0, 1).cut);
        assert!(t.get(1, 0).cut, "a link is unordered");
        assert!(!t.get(1, 2).cut);
        assert_eq!(t.describe(), "m1–m2: cut");
        t.clear();
        assert_eq!(t.describe(), "no faults");
    }

    #[test]
    fn message_mode_is_only_needed_for_duplication_and_reordering() {
        assert!(!LinkFault {
            cut: true,
            ..Default::default()
        }
        .message_mode());
        assert!(LinkFault {
            duplicate: 0.1,
            ..Default::default()
        }
        .message_mode());
        assert!(LinkFault {
            reorder: 0.1,
            ..Default::default()
        }
        .message_mode());
        assert!(LinkFault::default().is_clean());
    }

    #[test]
    fn http_framing_finds_whole_messages_only() {
        let msg = b"POST /raft HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc";
        assert_eq!(http_message_len(msg), Some(msg.len()));
        assert_eq!(http_message_len(&msg[..msg.len() - 1]), None);
        assert_eq!(http_message_len(b"POST /raft HTTP/1.1\r\n"), None);
        let no_body = b"GET /raft/stream/msgapp/1 HTTP/1.1\r\nHost: x\r\n\r\n";
        assert_eq!(http_message_len(no_body), Some(no_body.len()));
        let chunked = b"POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n";
        assert_eq!(
            http_message_len(chunked),
            None,
            "a chunked stream is left unframed rather than mis-split"
        );
        let two = b"GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\n\r\n";
        assert_eq!(http_message_len(two), Some(19));
    }

    #[test]
    fn proc_addresses_are_encoded_the_way_the_kernel_writes_them() {
        let a: SocketAddr = "127.0.0.1:34288".parse().expect("addr");
        assert_eq!(hex_addr(a), "0100007F:85F0");
        assert!(ends_with_addr("0100007F:85F0", "0100007F:85F0"));
        assert!(!ends_with_addr("0100007F:85F1", "0100007F:85F0"));
    }

    #[test]
    fn the_source_map_finds_this_very_process() {
        // A real loopback connection, looked up through /proc exactly as a peer would be.
        let server = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let dest = server.local_addr().expect("addr");
        let client = std::net::TcpStream::connect(dest).expect("connect");
        let source = client.local_addr().expect("addr");
        let (_accepted, _) = server.accept().expect("accept");
        let map = SourceMap::default();
        map.register(7, std::process::id());
        match map.owner(source, dest) {
            Some(m) => assert_eq!(m, 7),
            None => {
                // A machine without a readable /proc/net/tcp degrades to destination-wide
                // faults; that is a supported mode, not a failure.
                assert!(
                    std::fs::read_to_string("/proc/net/tcp").is_err(),
                    "/proc is readable, so the lookup should have found this process"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_proxy_forwards_bytes_and_a_cut_stops_them() {
        let faults = Arc::new(FaultTable::default());
        let sources = Arc::new(SourceMap::default());
        let stats = Arc::new(ProxyStats::default());
        // An echo server standing in for a member's peer port.
        let echo = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let echo_port = echo.local_addr().expect("addr").port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = echo.accept().await {
                tokio::spawn(async move {
                    let mut b = [0u8; 1024];
                    while let Ok(n) = s.read(&mut b).await {
                        if n == 0 || s.write_all(&b[..n]).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        let port = crate::node::free_port().expect("port");
        let proxy = PeerProxy::start(
            0,
            port,
            echo_port,
            faults.clone(),
            sources.clone(),
            stats.clone(),
            1,
        )
        .await
        .expect("proxy");
        assert_eq!(proxy.member, 0);

        let mut c = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        c.write_all(b"ping").await.expect("write");
        let mut b = [0u8; 4];
        c.read_exact(&mut b).await.expect("read");
        assert_eq!(&b, b"ping");

        // Nothing identified the dialer (it is this test, not a member), so the
        // destination's self-link is what applies.
        faults.set(
            0,
            0,
            LinkFault {
                cut: true,
                ..Default::default()
            },
        );
        let mut c2 = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        let mut b2 = [0u8; 4];
        let _ = c2.write_all(b"ping").await;
        let read = tokio::time::timeout(Duration::from_millis(400), c2.read_exact(&mut b2)).await;
        assert!(
            matches!(read, Ok(Err(_)) | Err(_)),
            "a cut link must not answer, got {read:?}"
        );
        assert!(stats.blocked() >= 1, "{}", stats.summary());
    }
}
