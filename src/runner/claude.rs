use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::runner::{
    AgentRequest, AgentResponse, AgentRunner, ExecutionMode, INTERNAL_RUN_ENV, extract_json_value,
};

#[derive(Debug, Clone)]
pub struct ClaudeRunner {
    binary: String,
    extra_args: Vec<String>,
}

impl ClaudeRunner {
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            extra_args: Vec::new(),
        }
    }

    pub fn with_extra_args(mut self, extra_args: Vec<String>) -> Self {
        self.extra_args = extra_args;
        self
    }

    pub fn build_args(&self, req: &AgentRequest) -> Result<Vec<String>> {
        let perm = match req.mode {
            ExecutionMode::ReadOnly => "plan",
            ExecutionMode::Verify => "default",
            ExecutionMode::WorkspaceWrite => "acceptEdits",
        };
        let has_perm_override = self.extra_args.iter().any(|a| {
            a == "--permission-mode"
                || a.starts_with("--permission-mode=")
                || a == "--dangerously-skip-permissions"
        });
        let has_output_format = self
            .extra_args
            .iter()
            .any(|a| a == "--output-format" || a.starts_with("--output-format="));

        let mut args = vec!["-p".to_string()];
        if !has_perm_override {
            args.push("--permission-mode".to_string());
            args.push(perm.to_string());
        }
        args.extend(self.extra_args.iter().cloned());

        if let Some(ref schema) = req.json_schema {
            if !has_output_format {
                args.push("--output-format".to_string());
                args.push("json".to_string());
            }
            args.push("--json-schema".to_string());
            args.push(serde_json::to_string(schema)?);
        } else if req.mode == ExecutionMode::WorkspaceWrite && !has_output_format {
            // Stream structured progress events during long-running developer tasks so users
            // never stare at a silent black box.
            args.push("--output-format".to_string());
            args.push("stream-json".to_string());
            args.push("--verbose".to_string());
        }

        Ok(args)
    }
}

/// Summarizes a single NDJSON line from `claude -p --output-format stream-json --verbose`,
/// returning `(optional_progress_label, optional_final_result_text)`.
pub fn parse_claude_stream_line(line: &str) -> (Option<String>, Option<String>) {
    let Ok(val) = serde_json::from_str::<Value>(line.trim()) else {
        return (None, None);
    };
    let kind = val.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "assistant" => {
            if let Some(content) = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_array)
            {
                let mut tools = Vec::new();
                for block in content {
                    if block.get("type").and_then(Value::as_str) == Some("tool_use")
                        && let Some(name) = block.get("name").and_then(Value::as_str)
                    {
                        tools.push(name.to_string());
                    }
                }
                if !tools.is_empty() {
                    return (Some(format!("ツール実行: {}", tools.join(", "))), None);
                }
            }
            (None, None)
        }
        "result" => {
            let final_text = val
                .get("result")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            (Some("完了".to_string()), final_text)
        }
        _ => (None, None),
    }
}

impl AgentRunner for ClaudeRunner {
    fn name(&self) -> &str {
        "claude"
    }

    fn run(&self, req: &AgentRequest) -> Result<AgentResponse> {
        let args = self.build_args(req)?;
        let streaming = args.iter().any(|a| a == "stream-json");
        let started = Instant::now();

        let mut child = Command::new(&self.binary)
            .args(&args)
            .current_dir(&req.working_dir)
            .envs(&req.env_vars)
            .env(INTERNAL_RUN_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("`{}` の起動に失敗しました", self.binary))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(req.prompt.as_bytes())?;
        }

        let (status, raw_stdout, raw_stderr) = if streaming {
            let stdout_pipe = child.stdout.take();
            let stderr_pipe = child.stderr.take();
            let stderr_handle = std::thread::spawn(move || {
                let mut buf = String::new();
                if let Some(mut s) = stderr_pipe {
                    let _ = s.read_to_string(&mut buf);
                }
                buf
            });

            let mut collected = Vec::new();
            let mut final_result: Option<String> = None;
            if let Some(stdout) = stdout_pipe {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    let (progress, result_text) = parse_claude_stream_line(&line);
                    if let Some(label) = progress {
                        eprintln!("  [claude {}s] {label}", started.elapsed().as_secs());
                    }
                    if let Some(res) = result_text {
                        final_result = Some(res);
                    }
                    collected.push(line);
                }
            }
            let status = child.wait()?;
            let raw_stderr = stderr_handle.join().unwrap_or_default();
            let raw_stdout = final_result.unwrap_or_else(|| collected.join("\n"));
            (status, raw_stdout, raw_stderr)
        } else {
            let output = child.wait_with_output()?;
            (
                output.status,
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
            )
        };

        if !status.success() {
            bail!(
                "claude -p が失敗しました (exit {:?}):\nstderr: {}\nstdout: {}",
                status.code(),
                raw_stderr.trim(),
                raw_stdout.trim()
            );
        }

        let structured_json = if req.json_schema.is_some() {
            Some(extract_json_value(&raw_stdout)?)
        } else {
            None
        };

        Ok(AgentResponse {
            agent_name: self.name().to_string(),
            raw_stdout,
            raw_stderr,
            structured_json,
        })
    }
}
