use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::Value;

use crate::runner::{AgentRequest, AgentResponse, AgentRunner};

type HandlerFn = Box<dyn FnMut(&AgentRequest) -> Result<AgentResponse> + Send>;

#[derive(Clone)]
pub struct MockRunner {
    name: String,
    calls: Arc<Mutex<Vec<AgentRequest>>>,
    handler: Arc<Mutex<HandlerFn>>,
}

impl MockRunner {
    pub fn from_fn<F>(name: impl Into<String>, handler: F) -> Self
    where
        F: FnMut(&AgentRequest) -> Result<AgentResponse> + Send + 'static,
    {
        Self {
            name: name.into(),
            calls: Arc::new(Mutex::new(Vec::new())),
            handler: Arc::new(Mutex::new(Box::new(handler))),
        }
    }

    pub fn with_json_responses(name: impl Into<String>, responses: Vec<Value>) -> Self {
        let name_str = name.into();
        let agent_name = name_str.clone();
        let mut iter = responses.into_iter();
        Self::from_fn(name_str, move |_req| {
            let val = iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("MockRunner の応答キューが空になりました"))?;
            Ok(AgentResponse {
                agent_name: agent_name.clone(),
                raw_stdout: val.to_string(),
                raw_stderr: String::new(),
                structured_json: Some(val),
            })
        })
    }

    pub fn recorded_calls(&self) -> Vec<AgentRequest> {
        self.calls.lock().expect("lock calls").clone()
    }
}

impl AgentRunner for MockRunner {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self, req: &AgentRequest) -> Result<AgentResponse> {
        self.calls.lock().expect("lock calls").push(req.clone());
        let mut guard = self.handler.lock().expect("lock handler");
        (guard)(req)
    }
}
