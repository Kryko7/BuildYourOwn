//! Helpers every stage file leans on: turning an etcd error into a failure, waiting for a
//! condition, and the handful of assertions that would otherwise be written fifty times.

use crate::assert::{Check, Failure, FailureKind};
use crate::etcd::{Client, EtcdError};
use serde_json::Value;
use std::fmt::Debug;
use std::future::Future;
use std::time::{Duration, Instant};

/// Turn a request that must succeed into a result the test can use.
///
/// The failure carries the status line and the body, because the first thing anyone wants
/// to know about a refused request is what the server actually said.
pub fn ok<T>(result: Result<T, EtcdError>, what: &str) -> Result<T, Failure> {
    result.map_err(|e| describe(e, what))
}

/// Turn an [`EtcdError`] into a [`Failure`] with everything it knows attached.
pub fn describe(e: EtcdError, what: &str) -> Failure {
    let kind = match &e {
        EtcdError::Transport(crate::etcd::HttpError::Timeout(_)) => FailureKind::Timeout,
        EtcdError::Transport(_) => FailureKind::Connection,
        EtcdError::Decode { .. } => FailureKind::Protocol,
        EtcdError::Status { .. } => FailureKind::Assertion,
    };
    let body = e.body();
    let mut f = Failure::new(kind, format!("{what}: {e}"));
    if !body.trim().is_empty() {
        f = f.block("the server answered", body);
    }
    f
}

/// Demand that a request failed, and hand back the error so the test can inspect it.
pub fn expect_error<T: Debug>(
    result: Result<T, EtcdError>,
    what: &str,
) -> Result<EtcdError, Failure> {
    match result {
        Err(e) => Ok(e),
        Ok(v) => Err(Failure::new(
            FailureKind::Assertion,
            format!("{what}: the request was expected to fail, but it succeeded"),
        )
        .block("what came back", format!("{v:?}"))),
    }
}

/// Demand that a request failed with a particular gRPC code.
pub fn expect_code<T: Debug>(
    result: Result<T, EtcdError>,
    code: i64,
    what: &str,
) -> Result<EtcdError, Failure> {
    let e = expect_error(result, what)?;
    let mut c = Check::new(what);
    c.eq("error.code", Some(code), e.code());
    if let Some(status) = e.status() {
        c.that(
            "error.http_status",
            "a 4xx or 5xx status",
            !(200..300).contains(&status),
            status,
        );
    }
    c.block("the server answered", e.body());
    c.finish()?;
    Ok(e)
}

/// Poll `f` until it answers `true`, or give up with a failure naming what was waited for.
pub async fn wait_until<F, Fut>(what: &str, within: Duration, mut f: F) -> Result<Duration, Failure>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let start = Instant::now();
    loop {
        if f().await {
            return Ok(start.elapsed());
        }
        if start.elapsed() >= within {
            return Err(Failure::new(
                FailureKind::Assertion,
                format!("{what} did not happen within {} ms", within.as_millis()),
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Write `n` keys `<prefix>/00`, `<prefix>/01`, ... with values `v0`, `v1`, ...
pub async fn put_series(client: &Client, prefix: &[u8], n: usize) -> Result<Vec<i64>, Failure> {
    let mut revisions = Vec::new();
    for i in 0..n {
        let key = [prefix, format!("/{i:02}").as_bytes()].concat();
        let value = format!("v{i}");
        let r = ok(
            client.put(&key, value.as_bytes()).await,
            &format!("put {}", String::from_utf8_lossy(&key)),
        )?;
        revisions.push(r.header.revision);
    }
    Ok(revisions)
}

/// Bytes as text, for a report line.
pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// A pretty-printed JSON block for a failure.
pub fn json_block(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// Assert that a sequence of revisions strictly increases.
pub fn check_strictly_increasing(c: &mut Check, path: &str, values: &[i64]) {
    for (i, w) in values.windows(2).enumerate() {
        c.that(
            &format!("{path}[{}] < {path}[{}]", i, i + 1),
            "a strictly larger revision",
            w[1] > w[0],
            (w[0], w[1]),
        );
    }
}

/// Assert that a sequence never goes backwards.
pub fn check_non_decreasing(c: &mut Check, path: &str, values: &[i64]) {
    for (i, w) in values.windows(2).enumerate() {
        c.that(
            &format!("{path}[{}] <= {path}[{}]", i, i + 1),
            "a revision that does not go backwards",
            w[1] >= w[0],
            (w[0], w[1]),
        );
    }
}

/// The lowest number of members that can still decide anything.
pub fn majority(members: usize) -> usize {
    members / 2 + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::etcd::HttpError;

    fn status_error(code: i64) -> EtcdError {
        EtcdError::Status {
            status: 400,
            code,
            message: "etcdserver: mvcc: required revision has been compacted".into(),
            body: r#"{"code":11,"message":"..."}"#.into(),
        }
    }

    #[test]
    fn ok_attaches_what_the_server_said() {
        let f = ok::<()>(Err(status_error(11)), "range at revision 2").expect_err("must fail");
        assert_eq!(f.kind, FailureKind::Assertion);
        assert!(
            f.messages[0].starts_with("range at revision 2:"),
            "{:?}",
            f.messages
        );
        assert!(f.blocks.iter().any(|(t, _)| t == "the server answered"));
    }

    #[test]
    fn transport_failures_are_not_assertions() {
        let f = ok::<()>(
            Err(EtcdError::Transport(HttpError::Timeout("x".into()))),
            "put",
        )
        .expect_err("must fail");
        assert_eq!(f.kind, FailureKind::Timeout);
        let f = ok::<()>(
            Err(EtcdError::Transport(HttpError::Connect("x".into()))),
            "put",
        )
        .expect_err("must fail");
        assert_eq!(f.kind, FailureKind::Connection);
        let f = ok::<()>(
            Err(EtcdError::Decode {
                what: "x".into(),
                body: "y".into(),
            }),
            "put",
        )
        .expect_err("must fail");
        assert_eq!(f.kind, FailureKind::Protocol);
    }

    #[test]
    fn expect_code_checks_the_code_and_the_status() {
        expect_code(Err::<(), _>(status_error(11)), 11, "a compacted read").expect("must match");
        let f = expect_code(Err::<(), _>(status_error(3)), 11, "a compacted read")
            .expect_err("the wrong code must fail");
        assert!(f.messages[0].contains("error.code"), "{:?}", f.messages);
        let f = expect_code(Ok::<_, EtcdError>(7), 11, "a compacted read")
            .expect_err("a success must fail");
        assert!(
            f.messages[0].contains("expected to fail"),
            "{:?}",
            f.messages
        );
    }

    #[tokio::test]
    async fn wait_until_gives_up_with_a_useful_message() {
        let f = wait_until(
            "the leader to change",
            Duration::from_millis(120),
            || async { false },
        )
        .await
        .expect_err("must give up");
        assert!(
            f.messages[0].contains("the leader to change did not happen within 120 ms"),
            "{:?}",
            f.messages
        );
        let mut n = 0;
        let took = wait_until("a counter to reach 2", Duration::from_secs(2), || {
            n += 1;
            async move { n >= 2 }
        })
        .await
        .expect("must succeed");
        assert!(took < Duration::from_secs(2));
    }

    #[test]
    fn monotonicity_checks_name_the_pair_that_broke() {
        let mut c = Check::new("revisions");
        check_strictly_increasing(&mut c, "revisions", &[1, 2, 2, 3]);
        let f = c.finish().expect_err("must fail");
        assert!(
            f.messages[0].contains("revisions[1] < revisions[2]"),
            "{:?}",
            f.messages
        );

        let mut c = Check::new("revisions");
        check_non_decreasing(&mut c, "revisions", &[1, 2, 2, 3]);
        assert!(c.finish().is_ok(), "equal revisions are allowed here");

        let mut c = Check::new("revisions");
        check_non_decreasing(&mut c, "revisions", &[3, 2]);
        assert!(c.finish().is_err());
    }

    #[test]
    fn majority_is_more_than_half() {
        assert_eq!(majority(1), 1);
        assert_eq!(majority(3), 2);
        assert_eq!(majority(5), 3);
    }
}
