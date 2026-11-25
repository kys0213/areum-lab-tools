//! Running a model command (brief, triage proposal): the input goes in on
//! stdin, one JSON object comes back on stdout. Only the mechanics live
//! here; each caller checks its own contract on the object.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;

/// How much of the command's stderr a failure keeps.
const STDERR_TAIL: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelCommand {
    pub(crate) argv: Vec<String>,
    pub(crate) timeout: Duration,
}

impl ModelCommand {
    /// Splits on whitespace — no shell, no quoting. `None` for a blank
    /// command.
    pub(crate) fn from_command(command: &str, timeout: Duration) -> Option<ModelCommand> {
        let argv: Vec<String> = command.split_whitespace().map(str::to_owned).collect();
        (!argv.is_empty()).then_some(ModelCommand { argv, timeout })
    }

    /// Runs the command with `input` on stdin and answers its stdout.
    /// `DOPPEL_DISABLE` keeps the command's own session from connecting back
    /// to doppel; the parent's `CLAUDE_CODE_SESSION_ID` is dropped because a
    /// nested `claude` takes it over as its own id. `what` names the answer
    /// in errors.
    pub(super) async fn run(
        &self,
        input: &Value,
        cwd: Option<&str>,
        what: &str,
    ) -> Result<String, String> {
        let mut command = tokio::process::Command::new(&self.argv[0]);
        command
            .args(&self.argv[1..])
            .env("DOPPEL_DISABLE", "1")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CLAUDE_CODE_REMOTE_SESSION_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = cwd.filter(|d| Path::new(d).is_dir()) {
            command.current_dir(dir);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", self.argv[0]))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input.to_string().as_bytes())
                .await
                .map_err(|e| format!("cannot write the bundle: {e}"))?;
        }
        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| format!("timed out after {}s", self.timeout.as_secs()))?
            .map_err(|e| format!("cannot read the {what}: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr
                .chars()
                .rev()
                .take(STDERR_TAIL)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            return Err(format!(
                "the {what} command exited with {}: {}",
                output.status,
                tail.trim()
            ));
        }
        String::from_utf8(output.stdout).map_err(|_| format!("the {what} is not UTF-8"))
    }
}

/// The one JSON object the command answered. `claude -p --output-format
/// json` wraps the model's text in `{"type":"result", "result": …}`; another
/// command may print the object directly. One layer of code fence around the
/// object is removed — models add it unasked — anything else that is not one
/// JSON object is a contract violation.
pub(super) fn answer_object(stdout: &str, what: &str) -> Result<Map<String, Value>, String> {
    let outer: Option<Value> = serde_json::from_str(stdout.trim()).ok();
    let text = match &outer {
        Some(Value::Object(map)) if map.get("type").and_then(Value::as_str) == Some("result") => {
            if map.get("is_error").and_then(Value::as_bool) == Some(true) {
                return Err(format!(
                    "the {what} command reported an error: {}",
                    map.get("result").and_then(Value::as_str).unwrap_or("")
                ));
            }
            map.get("result")
                .and_then(Value::as_str)
                .ok_or("the result envelope has no text")?
                .to_owned()
        }
        _ => stdout.to_owned(),
    };
    let body = strip_fence(text.trim());
    match serde_json::from_str(body) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("the {what} is not a JSON object")),
        Err(e) => Err(format!("the {what} is not one JSON object: {e}")),
    }
}

/// A non-blank string field; blank and `null` read as absent.
pub(super) fn optional_string(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
        Some(Value::String(_)) => Ok(None),
        Some(_) => Err(format!("\"{key}\" must be a string")),
    }
}

/// `low`, `medium` or `high`, required.
pub(super) fn confidence(map: &Map<String, Value>) -> Result<String, String> {
    let confidence = optional_string(map, "confidence")?.ok_or("\"confidence\" is missing")?;
    if !matches!(confidence.as_str(), "low" | "medium" | "high") {
        return Err(format!(
            "\"confidence\" must be low, medium or high, not {confidence}"
        ));
    }
    Ok(confidence)
}

/// The precedent ids the answer cites; each must be one the input carried.
pub(super) fn cited(map: &Map<String, Value>, carried: &[String]) -> Result<Vec<String>, String> {
    let cited: Vec<String> = match map.get("cited") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| {
                i.as_str()
                    .map(str::to_owned)
                    .ok_or("\"cited\" must list ids")
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("\"cited\" must be a list".to_owned()),
    };
    if let Some(unknown) = cited.iter().find(|id| !carried.contains(id)) {
        return Err(format!("cites {unknown}, which the bundle did not carry"));
    }
    Ok(cited)
}

fn strip_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let after_info = rest.split_once('\n').map_or("", |(_, body)| body);
    after_info
        .trim_end()
        .strip_suffix("```")
        .unwrap_or(after_info)
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn blank_and_whitespace_commands_are_none() {
        assert_eq!(
            ModelCommand::from_command("  claude -p  /doppel:brief ", Duration::from_secs(1))
                .unwrap()
                .argv,
            ["claude", "-p", "/doppel:brief"]
        );
        assert_eq!(
            ModelCommand::from_command("   ", Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn the_object_is_read_through_envelope_and_fence() {
        let wrapped = json!({"type": "result", "result": "```json\n{\"a\": 1}\n```"}).to_string();
        assert_eq!(answer_object(&wrapped, "x").unwrap()["a"], 1);
        assert_eq!(answer_object("{\"a\": 2}", "x").unwrap()["a"], 2);
        let failed = json!({"type": "result", "is_error": true, "result": "boom"}).to_string();
        assert!(
            answer_object(&failed, "proposal")
                .unwrap_err()
                .contains("proposal command reported an error")
        );
        assert!(
            answer_object("[1]", "x")
                .unwrap_err()
                .contains("not a JSON object")
        );
    }
}
