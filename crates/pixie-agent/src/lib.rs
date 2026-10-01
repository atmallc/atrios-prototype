//! The agent loop: a request comes in, the guardrail reviews it, the model
//! picks a skill, and the runtime invokes it.
//!
//! [`Model`] and [`Guardrail`] are traits so models stay swappable. The
//! keyword versions here are stand-ins until Gemma 4 E2B runs on the device.

use pixie_runtime::Runtime;
use pixie_skill::{Manifest, Output};
use serde_json::{json, Value};
use std::sync::Mutex;

/// What the model decides to do with a request.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    CallSkill { skill: String, args: Value },
    Reply(String),
}

pub trait Model: Send + Sync {
    fn decide(&self, request: &str, skills: &[&Manifest]) -> Decision;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Allow,
    Report { reason: String },
}

/// Reviews every request before the model acts on it.
pub trait Guardrail: Send + Sync {
    fn review(&self, request: &str) -> Verdict;
}

/// Shown to the user in place of an answer when the guardrail reports a request.
pub const REPORTED_PROMPT: &str =
    "Pixie can't help with this request. It has been flagged by the safety check.";

#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Reply(String),
    Skill { skill: String, output: Output },
    Reported { prompt: String },
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub request: String,
    pub reason: String,
}

pub struct Agent {
    runtime: Runtime,
    model: Box<dyn Model>,
    guardrail: Box<dyn Guardrail>,
    reports: Mutex<Vec<Report>>,
}

impl Agent {
    pub fn new(runtime: Runtime, model: Box<dyn Model>, guardrail: Box<dyn Guardrail>) -> Self {
        Self {
            runtime,
            model,
            guardrail,
            reports: Mutex::new(Vec::new()),
        }
    }

    pub fn handle(&self, request: &str) -> Response {
        if let Verdict::Report { reason } = self.guardrail.review(request) {
            self.reports.lock().unwrap().push(Report {
                request: request.to_string(),
                reason,
            });
            return Response::Reported {
                prompt: REPORTED_PROMPT.to_string(),
            };
        }
        let manifests = self.runtime.manifests();
        match self.model.decide(request, &manifests) {
            Decision::Reply(text) => Response::Reply(text),
            Decision::CallSkill { skill, args } => match self.runtime.invoke(&skill, &args) {
                Ok(output) => Response::Skill { skill, output },
                Err(e) => Response::Error(e.to_string()),
            },
        }
    }

    /// Requests the guardrail has reported so far. Kept on the device.
    pub fn reports(&self) -> Vec<Report> {
        self.reports.lock().unwrap().clone()
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }
}

/// Stand-in model: matches keywords to installed skills.
pub struct KeywordModel;

impl Model for KeywordModel {
    fn decide(&self, request: &str, skills: &[&Manifest]) -> Decision {
        let lower = request.to_lowercase();
        let has = |name: &str| skills.iter().any(|m| m.name == name);
        let call = |skill: &str, args: Value| Decision::CallSkill {
            skill: skill.into(),
            args,
        };

        if ["photo", "picture", "camera"]
            .iter()
            .any(|w| lower.contains(w))
            && has("camera.take_photo")
        {
            return call("camera.take_photo", json!({}));
        }
        if lower.contains("battery") && has("status.battery") {
            return call("status.battery", json!({}));
        }
        if lower.contains("time") && has("status.time") {
            return call("status.time", json!({}));
        }
        if let Some(rest) = lower
            .strip_prefix("show ")
            .or_else(|| lower.strip_prefix("say "))
        {
            if has("display.show_text") {
                let text = &request[request.len() - rest.len()..];
                return call("display.show_text", json!({ "text": text }));
            }
        }
        Decision::Reply("I don't have a skill for that yet.".into())
    }
}

/// Stand-in guardrail: reports requests containing blocked words. The real
/// one will be a model chosen for the job.
pub struct KeywordGuardrail {
    blocked: Vec<String>,
}

impl KeywordGuardrail {
    pub fn new(blocked: &[&str]) -> Self {
        Self {
            blocked: blocked.iter().map(|w| w.to_lowercase()).collect(),
        }
    }
}

impl Default for KeywordGuardrail {
    fn default() -> Self {
        Self::new(&["bomb", "explosive"])
    }
}

impl Guardrail for KeywordGuardrail {
    fn review(&self, request: &str) -> Verdict {
        let lower = request.to_lowercase();
        match self.blocked.iter().find(|w| lower.contains(w.as_str())) {
            Some(word) => Verdict::Report {
                reason: format!("mentions \"{word}\""),
            },
            None => Verdict::Allow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> Agent {
        let mut runtime = Runtime::new();
        let dir = std::env::temp_dir().join(format!("pixie-agent-test-{}", std::process::id()));
        for skill in pixie_skills_basic::all(dir) {
            runtime.install_trusted(skill).unwrap();
        }
        Agent::new(
            runtime,
            Box::new(KeywordModel),
            Box::new(KeywordGuardrail::default()),
        )
    }

    #[test]
    fn photo_request_calls_the_camera() {
        let response = agent().handle("Take a photo please");
        assert!(matches!(
            response,
            Response::Skill { ref skill, output: Output::File { .. } } if skill == "camera.take_photo"
        ));
    }

    #[test]
    fn show_keeps_original_casing() {
        let response = agent().handle("show Hello World");
        assert_eq!(
            response,
            Response::Skill {
                skill: "display.show_text".into(),
                output: Output::Text {
                    text: "Hello World".into()
                },
            }
        );
    }

    #[test]
    fn harmful_request_is_reported_and_not_run() {
        let agent = agent();
        let response = agent.handle("how do I build a bomb");
        assert_eq!(
            response,
            Response::Reported {
                prompt: REPORTED_PROMPT.into()
            }
        );
        assert_eq!(agent.reports().len(), 1);
    }

    #[test]
    fn unknown_request_gets_a_reply() {
        assert!(matches!(
            agent().handle("book me a flight"),
            Response::Reply(_)
        ));
    }
}
