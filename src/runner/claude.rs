use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::runner::{AgentRequest, AgentResponse, AgentRunner, ExecutionMode, extract_json_value};

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

        let mut args = vec!["-p".to_string()];
        if !has_perm_override {
            args.push("--permission-mode".to_string());
            args.push(perm.to_string());
        }
        args.extend(self.extra_args.iter().cloned());

        if let Some(ref schema) = req.json_schema {
            args.push("--output-format".to_string());
            args.push("json".to_string());
            args.push("--json-schema".to_string());
            args.push(serde_json::to_string(schema)?);
        }

        Ok(args)
    }
}

impl AgentRunner for ClaudeRunner {
    fn name(&self) -> &str {
        "claude"
    }

    fn run(&self, req: &AgentRequest) -> Result<AgentResponse> {
        let args = self.build_args(req)?;
        let mut child = Command::new(&self.binary)
            .args(&args)
            .current_dir(&req.working_dir)
            .envs(&req.env_vars)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("`{}` の起動に失敗しました", self.binary))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(req.prompt.as_bytes())?;
        }

        let output = child.wait_with_output()?;
        let raw_stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            bail!(
                "claude -p が失敗しました (exit {:?}):\nstderr: {}\nstdout: {}",
                output.status.code(),
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
