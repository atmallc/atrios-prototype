//! Core skill types for Pixie.
//!
//! A skill is Pixie's unit of capability: the AI invokes skills to act on the
//! device (take a photo, show text, read the battery) or on the world (Gmail).
//! Every skill declares a [`Manifest`]; the runtime uses it to decide whether
//! the skill may be installed and what it is allowed to touch.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Who built a skill, which decides how much the runtime trusts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Locked-down OS skills (guardrails, payments, messaging). Only the
    /// business can build these; they run natively.
    Critical,
    /// Basic OS skills shipped with Pixie (camera, display, status bar).
    System,
    /// Third-party skills from the skill store. Run sandboxed.
    Store,
    /// Skills written on the device by the local model. Run sandboxed.
    AiAuthored,
}

impl Tier {
    /// Store and AI-authored skills run in the locked sandbox.
    pub fn sandboxed(self) -> bool {
        matches!(self, Tier::Store | Tier::AiAuthored)
    }
}

/// What a skill may touch. Declared up front in the manifest; the runtime
/// refuses a call if the skill was not granted every permission it declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Camera,
    Display,
    Microphone,
    /// Playing sound through the speaker.
    Audio,
    Telephony,
    Network,
    Storage,
    PowerStatus,
    Clock,
}

/// A skill's declaration: its name, what it does, the JSON shape of its
/// arguments, and the permissions it needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Dotted name, e.g. `camera.take_photo`.
    pub name: String,
    /// One line the model reads when choosing skills.
    pub description: String,
    pub tier: Tier,
    pub permissions: Vec<Permission>,
    /// JSON Schema for the arguments. MCP tools map onto this directly.
    #[serde(default)]
    pub input_schema: Value,
}

/// What a skill hands back to the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Output {
    /// Text to show or speak to the user.
    Text { text: String },
    /// A file the skill produced, such as a photo.
    File { path: String, mime: String },
    /// Structured data for the agent to use in a later step.
    Data { value: Value },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SkillError {
    InvalidArgs(String),
    Unavailable(String),
    Failed(String),
}

impl fmt::Display for SkillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkillError::InvalidArgs(m) => write!(f, "invalid arguments: {m}"),
            SkillError::Unavailable(m) => write!(f, "unavailable: {m}"),
            SkillError::Failed(m) => write!(f, "failed: {m}"),
        }
    }
}

impl std::error::Error for SkillError {}

/// A skill the runtime can invoke.
pub trait Skill: Send + Sync {
    fn manifest(&self) -> &Manifest;
    fn invoke(&self, args: &Value) -> Result<Output, SkillError>;
}
