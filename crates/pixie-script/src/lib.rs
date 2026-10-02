//! Scripted skills: skills written as [Rhai](https://rhai.rs) scripts, so a
//! model can write a new one while the phone is running.
//!
//! The sandbox is the language itself. Rhai has no file, network or process
//! access of its own; a script can only call the functions registered here,
//! and each of those exists only if the skill's manifest asked for the
//! matching permission *and* the runtime granted it. Operation, string and
//! recursion limits stop a runaway script from hanging the phone.
//!
//! What a script sees:
//!
//! | Permission    | Functions |
//! | ------------- | --------- |
//! | `clock`       | `time_utc()` -> "HH:MM UTC", `now_secs()` -> seconds since 1970 |
//! | `power_status`| `battery()` -> "87%", `battery_percent()` -> 87 |
//! | `storage`     | `read_file(path)`, `list_dir(path)` (read-only, 1000 characters), `save(text)`, `load()` (this skill's own scratch file) |
//! | `display`     | none; the script's result is shown on screen |
//!
//! The variable `args` holds the call's arguments. The value of the last
//! expression is the skill's answer.

use pixie_skill::{Manifest, Output, Permission, Skill, SkillError, Tier};
use pixie_skills_basic::{Battery, Clock, OpenFile};
use rhai::{Dynamic, Engine, EvalAltResult, Scope, AST};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Permissions a written-on-the-device skill may ask for. Camera, microphone,
/// telephony and network have no functions yet, so they are refused.
pub const ALLOWED: [Permission; 4] = [
    Permission::Clock,
    Permission::PowerStatus,
    Permission::Storage,
    Permission::Display,
];

const MAX_SAVED: usize = 4096;

/// A skill as the brain writes it and as it is stored on the computer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillSpec {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub input_schema: Value,
    pub code: String,
}

/// A call the brain wants the phone to make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub skill: String,
    #[serde(default)]
    pub args: Value,
}

/// The brain's answer to a request. Any mix of fields may be set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BrainReply {
    #[serde(default)]
    pub say: Option<String>,
    #[serde(default)]
    pub call: Option<Call>,
    #[serde(default)]
    pub propose: Option<SkillSpec>,
    /// Arguments to run the proposed skill with as soon as it is approved, so
    /// the owner's original request gets answered without asking again.
    #[serde(default)]
    pub run_args: Option<Value>,
}

impl SkillSpec {
    /// Checks the name and permissions, and that the script compiles.
    pub fn validate(&self) -> Result<(), String> {
        let ok_name = !self.name.is_empty()
            && self.name.len() <= 40
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
        if !ok_name {
            return Err("skill name must be 1-40 characters of a-z, 0-9, _ and .".into());
        }
        if let Some(p) = self.permissions.iter().find(|p| !ALLOWED.contains(p)) {
            return Err(format!("permission {p:?} is not available to written skills"));
        }
        if self.code.len() > 8000 {
            return Err("script is longer than 8000 characters".into());
        }
        compile(&self.code).map(|_| ())
    }

    /// Builds the runnable skill. `data_dir` holds each skill's scratch file.
    pub fn build(&self, data_dir: impl Into<PathBuf>) -> Result<ScriptSkill, String> {
        self.validate()?;
        ScriptSkill::new(self, data_dir.into())
    }
}

fn compile(code: &str) -> Result<AST, String> {
    engine(&[], PathBuf::new(), "check")
        .compile(code)
        .map_err(|e| format!("script error: {e}"))
}

/// A skill backed by a script.
pub struct ScriptSkill {
    manifest: Manifest,
    engine: Engine,
    ast: AST,
}

impl ScriptSkill {
    fn new(spec: &SkillSpec, data_dir: PathBuf) -> Result<Self, String> {
        let engine = engine(&spec.permissions, data_dir, &spec.name);
        let ast = engine
            .compile(&spec.code)
            .map_err(|e| format!("script error: {e}"))?;
        let input_schema = if spec.input_schema.is_null() {
            json!({"type": "object", "properties": {}})
        } else {
            spec.input_schema.clone()
        };
        Ok(Self {
            manifest: Manifest {
                name: spec.name.clone(),
                description: spec.description.clone(),
                tier: Tier::AiAuthored,
                permissions: spec.permissions.clone(),
                input_schema,
            },
            engine,
            ast,
        })
    }
}

impl Skill for ScriptSkill {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn invoke(&self, args: &Value) -> Result<Output, SkillError> {
        let mut scope = Scope::new();
        let args = rhai::serde::to_dynamic(args)
            .map_err(|e| SkillError::InvalidArgs(e.to_string()))?;
        scope.push_constant("args", args);
        let result: Dynamic = self
            .engine
            .eval_ast_with_scope(&mut scope, &self.ast)
            .map_err(|e| SkillError::Failed(format!("{e}")))?;
        let text = if result.is_unit() {
            "done".to_string()
        } else if result.is_string() {
            result.into_string().unwrap_or_default()
        } else {
            result.to_string()
        };
        Ok(Output::Text { text })
    }
}

type Fail = Box<EvalAltResult>;

fn fail(message: impl Into<String>) -> Fail {
    message.into().into()
}

/// A skill's own scratch file, under `data_dir`, named after the skill.
fn scratch_path(data_dir: &std::path::Path, skill: &str) -> PathBuf {
    data_dir.join(format!("{skill}.txt"))
}

fn text_of(output: Result<Output, SkillError>) -> Result<String, Fail> {
    match output {
        Ok(Output::Text { text }) => Ok(text),
        Ok(_) => Err(fail("unexpected output")),
        Err(e) => Err(fail(e.to_string())),
    }
}

/// An engine with the limits applied and only the functions `granted` allows.
fn engine(granted: &[Permission], data_dir: PathBuf, skill: &str) -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(200_000);
    e.set_max_string_size(4096);
    e.set_max_array_size(256);
    e.set_max_map_size(64);
    e.set_max_call_levels(32);
    e.set_max_expr_depths(64, 32);
    e.disable_symbol("eval");
    // Scripts answer through their value, so printing goes nowhere.
    e.on_print(|_| {});
    e.on_debug(|_, _, _| {});

    let has = |p: Permission| granted.contains(&p);
    if has(Permission::Clock) {
        let clock = Arc::new(Clock::new());
        let c = clock.clone();
        e.register_fn("time_utc", move || text_of(c.invoke(&Value::Null)));
        e.register_fn("now_secs", || -> i64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });
    }
    if has(Permission::PowerStatus) {
        let battery = Arc::new(Battery::new());
        let b = battery.clone();
        e.register_fn("battery", move || text_of(b.invoke(&Value::Null)));
        let b = battery;
        e.register_fn("battery_percent", move || -> Result<i64, Fail> {
            text_of(b.invoke(&Value::Null))?
                .trim_end_matches('%')
                .parse()
                .map_err(|_| fail("no battery reading"))
        });
    }
    if has(Permission::Storage) {
        let open = Arc::new(OpenFile::new());
        let o = open.clone();
        e.register_fn("read_file", move |path: &str| {
            text_of(o.invoke(&json!({ "path": path })))
        });
        let o = open;
        e.register_fn("list_dir", move |path: &str| {
            text_of(o.invoke(&json!({ "path": path })))
        });
        let file = scratch_path(&data_dir, skill);
        let f = file.clone();
        e.register_fn("save", move |text: &str| -> Result<(), Fail> {
            if text.len() > MAX_SAVED {
                return Err(fail("save: text too long"));
            }
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(&f, text).map_err(|e| fail(format!("save: {e}")))
        });
        e.register_fn("load", move || -> String {
            std::fs::read_to_string(&file).unwrap_or_default()
        });
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(code: &str, permissions: Vec<Permission>) -> SkillSpec {
        SkillSpec {
            name: "user.test".into(),
            description: "a test skill".into(),
            permissions,
            input_schema: Value::Null,
            code: code.into(),
        }
    }

    fn run(spec: &SkillSpec, args: Value) -> Result<String, SkillError> {
        let skill = spec.build(std::env::temp_dir().join("pixie-script-test")).unwrap();
        match skill.invoke(&args)? {
            Output::Text { text } => Ok(text),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn script_computes_with_its_arguments() {
        let s = spec(r#"let n = args.n; "double is " + (n * 2)"#, vec![]);
        assert_eq!(run(&s, json!({"n": 21})).unwrap(), "double is 42");
    }

    #[test]
    fn scripts_get_only_the_functions_they_were_granted() {
        // No clock permission: the function does not exist.
        let s = spec("time_utc()", vec![]);
        assert!(matches!(run(&s, json!({})), Err(SkillError::Failed(_))));
        let s = spec("time_utc()", vec![Permission::Clock]);
        assert!(run(&s, json!({})).unwrap().ends_with("UTC"));
    }

    #[test]
    fn storage_reads_files_and_keeps_a_scratch_note() {
        let s = spec(
            r#"save("hi " + args.who); load() + "|" + list_dir("/")"#,
            vec![Permission::Storage],
        );
        let out = run(&s, json!({"who": "pixie"})).unwrap();
        assert!(out.starts_with("hi pixie|"), "{out}");
    }

    #[test]
    fn runaway_scripts_are_stopped() {
        let s = spec("let i = 0; while true { i += 1; }", vec![]);
        let err = run(&s, json!({})).unwrap_err();
        assert!(err.to_string().contains("perations"), "{err}");
        let s = spec(r#"let s = "x"; loop { s += s; }"#, vec![]);
        assert!(run(&s, json!({})).is_err());
    }

    #[test]
    fn eval_is_disabled() {
        let s = spec(r#"eval("1 + 1")"#, vec![]);
        assert!(s.validate().is_err());
    }

    #[test]
    fn bad_specs_are_refused() {
        assert!(spec("1 +", vec![]).validate().unwrap_err().contains("script error"));
        let mut s = spec("1", vec![Permission::Network]);
        assert!(s.validate().unwrap_err().contains("Network"));
        s.permissions = vec![];
        s.name = "Bad Name!".into();
        assert!(s.validate().unwrap_err().contains("name"));
    }

    #[test]
    fn replies_parse_from_json() {
        let r: BrainReply = serde_json::from_str(
            r#"{"say":"hi","propose":{"name":"user.a","description":"d","permissions":["clock"],"code":"1"}}"#,
        )
        .unwrap();
        assert_eq!(r.say.as_deref(), Some("hi"));
        assert_eq!(r.propose.unwrap().permissions, vec![Permission::Clock]);
        let r: BrainReply = serde_json::from_str(r#"{"call":{"skill":"status.time"}}"#).unwrap();
        assert_eq!(r.call.unwrap().skill, "status.time");
    }
}
