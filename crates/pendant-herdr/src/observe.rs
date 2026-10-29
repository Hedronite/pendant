use std::process::Stdio;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};

use crate::error::HerdrError;
use crate::line::{read_limited_line, MAX_HERDR_LINE};
use crate::paths::named_session;
use crate::HerdrClient;

/// One `terminal.frame` line from `herdr terminal session observe`.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalFrame {
    pub seq: u64,
    pub full: bool,
    pub encoding: String,
    pub width: u32,
    pub height: u32,
    pub bytes_b64: String,
    pub raw: Value,
}

#[derive(Debug, Deserialize)]
struct FrameLine {
    #[serde(rename = "type")]
    kind: String,
    seq: u64,
    #[serde(default)]
    encoding: String,
    width: u32,
    height: u32,
    full: bool,
    bytes: String,
}

/// `herdr terminal session observe` as a stdio child. No PTY.
pub struct ObserveSession {
    pub argv: Vec<String>,
    /// Transcript tests set this. The scratch check uses the real `herdr` binary.
    pub fake_binary: bool,
    child: Child,
    stdout: BufReader<ChildStdout>,
}

impl ObserveSession {
    pub async fn next_frame(&mut self) -> Result<TerminalFrame, HerdrError> {
        loop {
            let line = match read_limited_line(&mut self.stdout, MAX_HERDR_LINE).await {
                Ok(line) => line,
                Err(HerdrError::EmptyResponse) => {
                    let status = self.child.wait().await.ok();
                    return Err(HerdrError::CliFailed {
                        status: status.and_then(|s| s.code()).unwrap_or(1),
                        stderr: "observe stdout closed".into(),
                    });
                }
                Err(err) => return Err(err),
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let raw: Value = serde_json::from_str(trimmed)?;
            if raw.get("type").and_then(Value::as_str) == Some("terminal.closed") {
                return Err(HerdrError::EmptyResponse);
            }
            let frame: FrameLine = serde_json::from_value(raw.clone())?;
            if frame.kind != "terminal.frame" {
                continue;
            }
            return Ok(TerminalFrame {
                seq: frame.seq,
                full: frame.full,
                encoding: frame.encoding,
                width: frame.width,
                height: frame.height,
                bytes_b64: frame.bytes,
                raw,
            });
        }
    }
}

impl HerdrClient {
    /// Spawn `herdr --session <name> terminal session observe <target> --cols N --rows N`.
    ///
    /// `HERDR_SOCKET_PATH` is this client's socket. `HERDR_SESSION` is set only
    /// when the path is a named session, so a child cannot attach to the
    /// default socket by inheriting the parent environment.
    pub async fn observe(
        &self,
        target: &str,
        cols: u16,
        rows: u16,
    ) -> Result<ObserveSession, HerdrError> {
        if target.starts_with('-') {
            return Err(HerdrError::BadObserveTarget);
        }
        let cols_s = cols.to_string();
        let rows_s = rows.to_string();
        let mut argv = Vec::new();
        if let Some(name) = named_session(self.socket_path()) {
            argv.push("--session".into());
            argv.push(name);
        }
        argv.extend([
            "terminal".into(),
            "session".into(),
            "observe".into(),
            target.to_string(),
            "--cols".into(),
            cols_s,
            "--rows".into(),
            rows_s,
        ]);

        let mut cmd = Command::new(self.herdr_bin_path());
        cmd.args(&argv)
            .env("HERDR_SOCKET_PATH", self.socket_path())
            .env_remove("HERDR_SESSION")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(name) = named_session(self.socket_path()) {
            cmd.env("HERDR_SESSION", name);
        }
        let mut child = cmd.spawn().map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                HerdrError::CliMissing
            } else {
                HerdrError::Io(err)
            }
        })?;
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
        }
        let stdout = child
            .stdout
            .take()
            .map(BufReader::new)
            .ok_or_else(|| HerdrError::CliFailed {
                status: 1,
                stderr: "observe stdout was not piped".into(),
            })?;
        let fake_binary =
            self.herdr_bin_path().file_name().and_then(|n| n.to_str()) != Some("herdr");
        let mut full_argv = vec![self.herdr_bin_path().to_string_lossy().into_owned()];
        full_argv.extend(argv);
        Ok(ObserveSession {
            argv: full_argv,
            fake_binary,
            child,
            stdout,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Transcript test. This binary is a fake. It is not the real herdr 0.9.5.
    #[tokio::test]
    async fn observe_transcript_uses_fake_herdr_binary() {
        let dir = tempfile::tempdir().unwrap();
        let argv_log = dir.path().join("argv.txt");
        let script = dir.path().join("fake-herdr");
        let argv_q = shell_quote(&argv_log);
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {argv_q}\nprintf '%s\\n' '{{\"type\":\"terminal.frame\",\"seq\":1,\"encoding\":\"ansi\",\"width\":40,\"height\":12,\"full\":true,\"bytes\":\"G1tI\"}}'\nprintf '%s\\n' '{{\"type\":\"terminal.frame\",\"seq\":2,\"encoding\":\"ansi\",\"width\":40,\"height\":12,\"full\":false,\"bytes\":\"YQ==\"}}'\n"
        );
        std::fs::write(&script, body).unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let sock = dir
            .path()
            .join(".config/herdr/sessions/pendant-bridge-g0/herdr.sock");
        let client = HerdrClient::with_herdr_bin(&sock, &script);
        let mut session = client.observe("w1:p1", 40, 12).await.unwrap();
        assert!(script.is_absolute());
        assert_eq!(session.argv[0], script.to_string_lossy());
        assert!(session.fake_binary, "transcript test must say the binary is fake");
        assert!(session.argv.iter().any(|a| a == "observe"));
        assert!(session.argv.iter().any(|a| a == "--session"));
        assert!(session.argv.iter().any(|a| a == "pendant-bridge-g0"));
        assert!(session.argv.iter().any(|a| a == "--cols"));
        assert!(session.argv.iter().any(|a| a == "40"));

        let full = session.next_frame().await.unwrap();
        assert!(full.full);
        assert_eq!(full.seq, 1);
        let incremental = session.next_frame().await.unwrap();
        assert!(!incremental.full);
        assert_eq!(incremental.seq, 2);
        assert_eq!(incremental.bytes_b64, "YQ==");

        let logged = std::fs::read_to_string(&argv_log).unwrap();
        assert!(logged.contains("terminal"));
        assert!(logged.contains("observe"));
        assert!(logged.contains("w1:p1"));
    }

    #[tokio::test]
    async fn observe_target_starting_with_dash_does_not_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawned");
        let script = dir.path().join("fake-herdr");
        let marker_q = marker.to_string_lossy().replace('\'', "'\\''");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{marker_q}'\n"),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let client = HerdrClient::with_herdr_bin(dir.path().join("herdr.sock"), &script);
        let result = client.observe("-pane", 40, 12).await;
        assert!(matches!(result, Err(HerdrError::BadObserveTarget)));
        assert!(!marker.exists(), "a child was spawned");
    }

    fn shell_quote(path: &std::path::Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }
}
