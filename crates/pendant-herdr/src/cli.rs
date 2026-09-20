use std::path::Path;
use std::process::Stdio;

use serde_json::{json, Value};
use tokio::process::Command;

use crate::error::HerdrError;
use crate::protocol::unwrap_result;

pub async fn rpc_cli(socket: &Path, method: &str, params: Value) -> Result<Value, HerdrError> {
    let args =
        cli_args(method, &params).ok_or_else(|| HerdrError::UnsupportedMethod(method.into()))?;
    let output = run_herdr(socket, &args).await?;
    if method == "agent.read" {
        return Ok(json!({
            "type": "agent_read",
            "text": String::from_utf8_lossy(&output.stdout).to_string()
        }));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(json!({ "ok": true }));
    }
    let parsed: Value = serde_json::from_str(trimmed)?;
    if let Some(err) = parsed.get("error") {
        let code = err
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("cli_error")
            .to_string();
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(trimmed)
            .to_string();
        return Err(HerdrError::Rpc {
            code,
            message,
            body: None,
        });
    }
    Ok(unwrap_result(parsed))
}

async fn run_herdr(socket: &Path, args: &[String]) -> Result<std::process::Output, HerdrError> {
    let output = Command::new("herdr")
        .args(args)
        .env("HERDR_SOCKET_PATH", socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                HerdrError::CliMissing
            } else {
                HerdrError::Io(err)
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if let Ok(parsed) = serde_json::from_str::<Value>(&stderr) {
            if let Some(err) = parsed.get("error") {
                return Err(HerdrError::Rpc {
                    code: err
                        .get("code")
                        .and_then(Value::as_str)
                        .unwrap_or("cli_error")
                        .to_string(),
                    message: err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or(&stderr)
                        .to_string(),
                    body: None,
                });
            }
        }
        return Err(HerdrError::CliFailed {
            status: output.status.code().unwrap_or(1),
            stderr,
        });
    }
    Ok(output)
}

fn cli_args(method: &str, params: &Value) -> Option<Vec<String>> {
    let str_param = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    };
    match method {
        "ping" => Some(vec!["status".into(), "server".into()]),
        "session.snapshot" => Some(vec!["api".into(), "snapshot".into()]),
        "worktree.list" => {
            let mut args = vec!["worktree".into(), "list".into()];
            if let Some(id) = str_param("workspace_id") {
                args.extend(["--workspace".into(), id]);
            }
            if let Some(cwd) = str_param("cwd") {
                args.extend(["--cwd".into(), cwd]);
            }
            Some(args)
        }
        "worktree.create" => {
            let mut args = vec!["worktree".into(), "create".into()];
            if let Some(id) = str_param("workspace_id") {
                args.extend(["--workspace".into(), id]);
            }
            if let Some(cwd) = str_param("cwd") {
                args.extend(["--cwd".into(), cwd]);
            }
            if let Some(branch) = str_param("branch") {
                args.extend(["--branch".into(), branch]);
            }
            if let Some(base) = str_param("base") {
                args.extend(["--base".into(), base]);
            }
            if let Some(path) = str_param("path") {
                args.extend(["--path".into(), path]);
            }
            if let Some(label) = str_param("label") {
                args.extend(["--label".into(), label]);
            }
            if params.get("focus").and_then(Value::as_bool) == Some(true) {
                args.push("--focus".into());
            } else {
                args.push("--no-focus".into());
            }
            Some(args)
        }
        "agent.get" => Some(vec!["agent".into(), "get".into(), str_param("target")?]),
        "agent.read" => {
            let mut args = vec![
                "agent".into(),
                "read".into(),
                str_param("target")?,
                "--source".into(),
                str_param("source").unwrap_or_else(|| "recent".into()),
            ];
            if let Some(lines) = params.get("lines").and_then(Value::as_u64) {
                args.extend(["--lines".into(), lines.to_string()]);
            }
            Some(args)
        }
        "agent.prompt" => Some(vec![
            "agent".into(),
            "prompt".into(),
            str_param("target")?,
            str_param("text")?,
        ]),
        "agent.send_keys" => {
            let mut args = vec!["agent".into(), "send-keys".into(), str_param("target")?];
            let keys = params.get("keys").and_then(Value::as_array)?;
            if keys.is_empty() {
                return None;
            }
            for key in keys {
                args.push(key.as_str()?.to_string());
            }
            Some(args)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_snapshot_and_worktree_create() {
        assert_eq!(
            cli_args("session.snapshot", &json!({})).unwrap(),
            vec!["api", "snapshot"]
        );
        let create = cli_args(
            "worktree.create",
            &json!({"workspace_id":"w1","branch":"feat/x","focus":false}),
        )
        .unwrap();
        assert!(create.contains(&"--workspace".into()));
        assert!(create.contains(&"w1".into()));
        assert!(create.contains(&"--branch".into()));
        assert!(create.contains(&"--no-focus".into()));
    }

    #[test]
    fn maps_agent_keys() {
        let args = cli_args(
            "agent.send_keys",
            &json!({"target":"w1:p1","keys":["enter","esc"]}),
        )
        .unwrap();
        assert_eq!(args, vec!["agent", "send-keys", "w1:p1", "enter", "esc"]);
    }
}
