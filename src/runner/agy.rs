use std::fs;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::runner::{AgentRequest, AgentResponse, AgentRunner, ExecutionMode, extract_json_value};

#[derive(Debug, Clone)]
pub struct AgyRunner {
    binary: String,
}

impl AgyRunner {
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    pub fn build_args(&self, req: &AgentRequest, schema_path: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "--disable-slash-commands".to_string(),
            "--output-format".to_string(),
        ];
        if req.json_schema.is_some() {
            args.push("json".to_string());
        } else {
            args.push("text".to_string());
        }

        // Note: Do NOT pass `--mode plan` for structured JSON calls because `plan` triggers
        // Antigravity's planning workflow instead of answering the JSON schema directly.
        match req.mode {
            ExecutionMode::ReadOnly => {
                args.push("--sandbox".to_string());
            }
            ExecutionMode::Verify => {
                args.push("--mode".to_string());
                args.push("auto".to_string());
            }
            ExecutionMode::WorkspaceWrite => {
                args.push("--mode".to_string());
                args.push("accept-edits".to_string());
            }
        }

        if req.timeout_secs > 0 {
            args.push("--print-timeout".to_string());
            args.push(format!("{}s", req.timeout_secs));
        }

        if let Some(path) = schema_path {
            args.push("--json-schema".to_string());
            args.push(path.to_string());
        }

        // Attach prompt directly to `--print=<prompt>` so agy never misinterprets
        // subsequent flags or leading dashes as the prompt value.
        args.push(format!("--print={}", req.prompt));
        args
    }
}

impl AgentRunner for AgyRunner {
    fn name(&self) -> &str {
        "agy"
    }

    fn run(&self, req: &AgentRequest) -> Result<AgentResponse> {
        let mut temp_schema = None;
        let schema_path = if let Some(ref schema) = req.json_schema {
            let path = std::env::temp_dir().join(format!(
                "daihon-agy-schema-{}.json",
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ));
            fs::write(&path, serde_json::to_string_pretty(schema)?)
                .context("agy 用 JSON Schema の書き出しに失敗しました")?;
            let path_str = path.to_string_lossy().to_string();
            temp_schema = Some(path);
            Some(path_str)
        } else {
            None
        };

        let args = self.build_args(req, schema_path.as_deref());
        let output = Command::new(&self.binary)
            .args(&args)
            .current_dir(&req.working_dir)
            .envs(&req.env_vars)
            .output()
            .with_context(|| format!("`{}` の起動に失敗しました", self.binary));

        if let Some(path) = temp_schema {
            let _ = fs::remove_file(path);
        }

        let output = output?;
        let raw_stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            bail!(
                "agy が失敗しました (exit {:?}):\nstderr: {}\nstdout: {}",
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
