//! Driving the `primitives` ladder: one process per topic, one command per line, one JSON
//! object per answer.
//!
//! The program is started as `./your_program.sh <topic>`. The harness then writes commands
//! to its stdin, one per line, and reads exactly one line of JSON back for each. Everything
//! is kept in a transcript, so a failure can show the whole conversation that led to it
//! rather than the single line that went wrong.

pub mod oracles;

use crate::assert::{Failure, FailureKind};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// A running primitives process, bound to one topic.
pub struct PrimProc {
    /// The topic it was started with.
    pub topic: String,
    /// Every command sent and every answer received, in order.
    pub transcript: Vec<(String, String)>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    stderr_path: PathBuf,
    timeout: Duration,
}

impl PrimProc {
    /// Start the program for one topic.
    pub async fn start(
        argv: &[String],
        cwd: &Path,
        env: &[(String, String)],
        topic: &str,
        tmp: &Path,
        timeout: Duration,
    ) -> Result<PrimProc, Failure> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| Failure::harness("the primitives command is empty"))?;
        std::fs::create_dir_all(tmp)
            .map_err(|e| Failure::harness(format!("cannot create {}: {e}", tmp.display())))?;
        let stderr_path = tmp.join(format!("{topic}.stderr"));
        let stderr = std::fs::File::create(&stderr_path)
            .map_err(|e| Failure::harness(format!("cannot create {}: {e}", stderr_path.display())))?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .arg(topic)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| {
            Failure::harness(format!("cannot start '{program} {topic}': {e}"))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Failure::harness("the child has no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Failure::harness("the child has no stdout"))?;
        Ok(PrimProc {
            topic: topic.to_string(),
            transcript: Vec::new(),
            child: Some(child),
            stdin: Some(stdin),
            stdout: Some(BufReader::new(stdout)),
            stderr_path,
            timeout,
        })
    }

    /// Send one command and read the one JSON object it answers with.
    pub async fn send(&mut self, command: &str) -> Result<Value, Failure> {
        let line = format!("{command}\n");
        if self.stdin.is_none() {
            return Err(self.gone("the program's stdin is already closed"));
        }
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(Failure::harness("the program's stdin vanished"));
        };
        if let Err(e) = stdin.write_all(line.as_bytes()).await {
            return Err(self.died(&format!("cannot write {command:?}: {e}")).await);
        }
        if let Err(e) = stdin.flush().await {
            return Err(self.died(&format!("cannot flush {command:?}: {e}")).await);
        }
        let stdout = self
            .stdout
            .as_mut()
            .ok_or_else(|| Failure::harness("the program's stdout is already closed"))?;
        let mut answer = String::new();
        let read = tokio::time::timeout(self.timeout, stdout.read_line(&mut answer)).await;
        match read {
            Err(_) => {
                self.transcript
                    .push((command.to_string(), "<no answer>".into()));
                Err(self
                    .fail(
                        FailureKind::Timeout,
                        format!(
                            "no answer to {command:?} within {} ms",
                            self.timeout.as_millis()
                        ),
                    )
                    .await)
            }
            Ok(Err(e)) => {
                self.transcript
                    .push((command.to_string(), format!("<read error: {e}>")));
                Err(self.died(&format!("cannot read the answer to {command:?}: {e}")).await)
            }
            Ok(Ok(0)) => {
                self.transcript
                    .push((command.to_string(), "<end of output>".into()));
                Err(self
                    .died(&format!(
                        "the program closed its stdout instead of answering {command:?}"
                    ))
                    .await)
            }
            Ok(Ok(_)) => {
                let text = answer.trim().to_string();
                self.transcript.push((command.to_string(), text.clone()));
                serde_json::from_str(&text).map_err(|e| {
                    Failure::protocol(format!(
                        "the answer to {command:?} is not one JSON object: {e}"
                    ))
                    .note(format!("the program wrote {text:?}"))
                    .block("transcript", self.transcript_block())
                })
            }
        }
    }

    /// Send several commands and collect the answers.
    pub async fn send_all<S: AsRef<str>>(&mut self, commands: &[S]) -> Result<Vec<Value>, Failure> {
        let mut out = Vec::with_capacity(commands.len());
        for c in commands {
            out.push(self.send(c.as_ref()).await?);
        }
        Ok(out)
    }

    /// Send one command and read a named number from the answer.
    pub async fn num(&mut self, command: &str, field: &str) -> Result<i64, Failure> {
        let v = self.send(command).await?;
        self.expect_i64(&v, command, field)
    }

    /// Read a named number out of an answer, or fail with the transcript.
    pub fn expect_i64(&self, v: &Value, command: &str, field: &str) -> Result<i64, Failure> {
        match v.get(field) {
            Some(Value::Number(n)) => n.as_i64().ok_or_else(|| {
                self.shape(command, &format!("{field} is not a whole number: {n}"))
            }),
            Some(Value::String(s)) => s
                .parse()
                .map_err(|_| self.shape(command, &format!("{field} is not a number: {s:?}"))),
            Some(other) => Err(self.shape(command, &format!("{field} should be a number, got {other}"))),
            None => Err(self.shape(command, &format!("the answer has no {field:?} field"))),
        }
    }

    /// Read a named string out of an answer.
    pub fn expect_str(&self, v: &Value, command: &str, field: &str) -> Result<String, Failure> {
        match v.get(field) {
            Some(Value::String(s)) => Ok(s.clone()),
            Some(other) => Err(self.shape(command, &format!("{field} should be a string, got {other}"))),
            None => Err(self.shape(command, &format!("the answer has no {field:?} field"))),
        }
    }

    /// Read a named boolean out of an answer.
    pub fn expect_bool(&self, v: &Value, command: &str, field: &str) -> Result<bool, Failure> {
        match v.get(field) {
            Some(Value::Bool(b)) => Ok(*b),
            Some(other) => Err(self.shape(command, &format!("{field} should be true or false, got {other}"))),
            None => Err(self.shape(command, &format!("the answer has no {field:?} field"))),
        }
    }

    /// Read a named array of strings out of an answer.
    pub fn expect_strs(&self, v: &Value, command: &str, field: &str) -> Result<Vec<String>, Failure> {
        match v.get(field) {
            Some(Value::Array(items)) => items
                .iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| self.shape(command, &format!("{field} holds {x}, not a string")))
                })
                .collect(),
            Some(other) => Err(self.shape(command, &format!("{field} should be an array, got {other}"))),
            None => Err(self.shape(command, &format!("the answer has no {field:?} field"))),
        }
    }

    /// Read a named number that may be fractional.
    pub fn expect_f64(&self, v: &Value, command: &str, field: &str) -> Result<f64, Failure> {
        match v.get(field) {
            Some(Value::Number(n)) => n
                .as_f64()
                .ok_or_else(|| self.shape(command, &format!("{field} is not a number: {n}"))),
            Some(Value::String(s)) => s
                .parse()
                .map_err(|_| self.shape(command, &format!("{field} is not a number: {s:?}"))),
            Some(other) => Err(self.shape(command, &format!("{field} should be a number, got {other}"))),
            None => Err(self.shape(command, &format!("the answer has no {field:?} field"))),
        }
    }

    /// A failure about the shape of one answer, carrying the transcript.
    pub fn shape(&self, command: &str, message: &str) -> Failure {
        Failure::protocol(format!("{command}: {message}"))
            .note(format!("topic {:?}", self.topic))
            .block("transcript", self.transcript_block())
    }

    /// The whole conversation so far, as a failure block.
    pub fn transcript_block(&self) -> String {
        let mut s = String::new();
        for (cmd, answer) in self.transcript.iter().rev().take(40).rev() {
            s.push_str(&format!("> {cmd}\n< {answer}\n"));
        }
        if self.transcript.len() > 40 {
            s = format!("... ({} earlier commands)\n{s}", self.transcript.len() - 40);
        }
        s.trim_end().to_string()
    }

    /// The program's stderr, when it wrote any.
    pub fn stderr(&self) -> String {
        std::fs::read_to_string(&self.stderr_path)
            .unwrap_or_default()
            .trim_end()
            .to_string()
    }

    fn gone(&self, message: &str) -> Failure {
        Failure::new(FailureKind::Connection, message.to_string())
            .block("transcript", self.transcript_block())
    }

    async fn died(&mut self, message: &str) -> Failure {
        self.fail(FailureKind::NodeCrash, message.to_string()).await
    }

    async fn fail(&mut self, kind: FailureKind, message: String) -> Failure {
        let mut f = Failure::new(kind, message)
            .note(format!("topic {:?}", self.topic))
            .block("transcript", self.transcript_block());
        if let Some(child) = self.child.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                f.messages.push(format!(
                    "the program had already exited with {}",
                    crate::node::describe_status(&status)
                ));
                f.kind = FailureKind::NodeCrash;
            }
        }
        let err = self.stderr();
        if !err.is_empty() {
            f = f.block("stderr", err);
        }
        f
    }

    /// Close stdin and wait briefly for the program to exit.
    pub async fn close(&mut self) {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = tokio::time::timeout(Duration::from_millis(500), child.wait()).await;
            let _ = child.start_kill();
        }
    }
}

impl Drop for PrimProc {
    fn drop(&mut self) {
        // `kill_on_drop` handles the child; dropping stdin also lets a well-behaved program
        // notice end of input and leave on its own.
        self.stdin = None;
    }
}

/// Format a JSON value the way a command line wants it: compact, no spaces.
pub fn arg(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn echo_proc(script: &str) -> (tempfile::TempDir, PrimProc) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("prog.sh");
        std::fs::write(&path, script).expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let proc = PrimProc::start(
            &[path.to_string_lossy().to_string()],
            dir.path(),
            &[],
            "lamport",
            dir.path(),
            Duration::from_millis(2000),
        )
        .await
        .expect("start");
        (dir, proc)
    }

    #[tokio::test]
    async fn one_command_gets_one_answer() {
        let (_d, mut p) = echo_proc(
            "#!/bin/sh\nn=0\nwhile read -r line; do n=$((n+1)); echo \"{\\\"ts\\\": $n}\"; done\n",
        )
        .await;
        assert_eq!(p.num("send a", "ts").await.expect("ts"), 1);
        assert_eq!(p.num("send a", "ts").await.expect("ts"), 2);
        assert!(p.transcript_block().contains("> send a"));
        p.close().await;
    }

    #[tokio::test]
    async fn a_program_that_says_nothing_times_out_with_its_transcript() {
        let (_d, mut p) = echo_proc("#!/bin/sh\nsleep 30\n").await;
        let f = p.send("send a").await.expect_err("must time out");
        assert_eq!(f.kind, FailureKind::Timeout);
        assert!(f.blocks.iter().any(|(t, _)| t == "transcript"));
    }

    #[tokio::test]
    async fn a_program_that_exits_is_reported_as_a_crash() {
        let (_d, mut p) = echo_proc("#!/bin/sh\necho oops >&2\nexit 3\n").await;
        let f = p.send("send a").await.expect_err("must fail");
        assert_eq!(f.kind, FailureKind::NodeCrash, "{:?}", f.messages);
        assert!(
            f.blocks.iter().any(|(t, b)| t == "stderr" && b.contains("oops")),
            "{:?}",
            f.blocks
        );
    }

    #[tokio::test]
    async fn output_that_is_not_json_is_a_protocol_failure() {
        let (_d, mut p) = echo_proc("#!/bin/sh\nwhile read -r l; do echo not json; done\n").await;
        let f = p.send("send a").await.expect_err("must fail");
        assert_eq!(f.kind, FailureKind::Protocol);
        assert!(f.messages[0].contains("not one JSON object"), "{:?}", f.messages);
    }

    #[tokio::test]
    async fn missing_fields_name_themselves() {
        let (_d, mut p) = echo_proc("#!/bin/sh\nwhile read -r l; do echo '{\"other\":1}'; done\n").await;
        let f = p.num("send a", "ts").await.expect_err("must fail");
        assert!(f.messages[0].contains("no \"ts\""), "{:?}", f.messages);
        p.close().await;
    }

    #[tokio::test]
    async fn numbers_may_arrive_as_strings() {
        let (_d, mut p) = echo_proc("#!/bin/sh\nwhile read -r l; do echo '{\"ts\":\"42\"}'; done\n").await;
        assert_eq!(p.num("send a", "ts").await.expect("ts"), 42);
        p.close().await;
    }
}
