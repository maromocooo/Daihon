use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::runner::{
    AgentRequest, AgentResponse, AgentRunner, ExecutionMode, INTERNAL_RUN_ENV, extract_json_value,
};

#[derive(Debug, Clone)]
pub struct CodexRunner {
    binary: String,
    extra_args: Vec<String>,
}

impl CodexRunner {
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

    pub fn build_args(
        &self,
        req: &AgentRequest,
        schema_path: Option<&str>,
        output_path: &str,
    ) -> Vec<String> {
        let sandbox = match (&req.sandbox_override, req.mode) {
            (Some(s), _) if !s.trim().is_empty() => s.trim().to_string(),
            (_, ExecutionMode::ReadOnly) => "read-only".to_string(),
            (_, ExecutionMode::Verify | ExecutionMode::WorkspaceWrite) => {
                "workspace-write".to_string()
            }
        };
        let has_sandbox_override = self.extra_args.iter().any(|a| {
            a == "-s" || a == "--sandbox" || a.starts_with("--sandbox=")
        });

        let mut args = vec![
            "exec".to_string(),
            "-C".to_string(),
            req.working_dir.to_string_lossy().to_string(),
        ];
        if !has_sandbox_override {
            args.push("-s".to_string());
            args.push(sandbox);
        }
        args.extend([
            "--ephemeral".to_string(),
            "--skip-git-repo-check".to_string(),
            "-o".to_string(),
            output_path.to_string(),
        ]);

        if let Some(path) = schema_path {
            args.push("--output-schema".to_string());
            args.push(path.to_string());
        }

        args.extend(self.extra_args.iter().cloned());
        args.push("-".to_string());
        args
    }
}

impl AgentRunner for CodexRunner {
    fn name(&self) -> &str {
        "codex"
    }

    fn run(&self, req: &AgentRequest) -> Result<AgentResponse> {
        let nonce = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let out_path = std::env::temp_dir().join(format!("daihon-codex-out-{nonce}.txt"));

        let mut temp_schema = None;
        let schema_path = if let Some(ref schema) = req.json_schema {
            let path = std::env::temp_dir()
                .join(format!("daihon-codex-schema-{nonce}.json"));
            fs::write(&path, serde_json::to_string_pretty(schema)?)
                .context("codex 用 JSON Schema の書き出しに失敗しました")?;
            let path_str = path.to_string_lossy().to_string();
            temp_schema = Some(path);
            Some(path_str)
        } else {
            None
        };

        let args = self.build_args(req, schema_path.as_deref(), &out_path.to_string_lossy());
        let mut child = Command::new(&self.binary)
            .args(&args)
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

        let output = child.wait_with_output()?;
        let last_message = fs::read_to_string(&out_path).ok();
        let _ = fs::remove_file(&out_path);
        if let Some(path) = temp_schema {
            let _ = fs::remove_file(path);
        }

        let raw_stdout = last_message.unwrap_or_else(|| {
            String::from_utf8_lossy(&output.stdout).to_string()
        });
        let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            bail!(
                "codex exec が失敗しました (exit {:?}):\nstderr: {}\nstdout: {}",
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
