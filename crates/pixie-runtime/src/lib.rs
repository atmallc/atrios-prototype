//! The skill runtime: installs skills, checks permissions, and invokes them.
//!
//! Store and AI-authored skills are marked sandboxed; today they run
//! in-process like the rest; the isolation backend is still to be built.

use pixie_skill::{Manifest, Output, Permission, Skill, SkillError, Tier};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fmt;

/// Who is installing a skill. Only the business may install critical skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Publisher {
    Business,
    ThirdParty,
    OnDeviceModel,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeError {
    UnknownSkill(String),
    AlreadyInstalled(String),
    /// The publisher may not install a skill of this tier.
    TierNotAllowed {
        skill: String,
        tier: Tier,
    },
    /// The skill declares a permission it was not granted.
    PermissionDenied {
        skill: String,
        permission: Permission,
    },
    Skill(SkillError),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::UnknownSkill(n) => write!(f, "no skill named {n}"),
            RuntimeError::AlreadyInstalled(n) => write!(f, "{n} is already installed"),
            RuntimeError::TierNotAllowed { skill, tier } => {
                write!(f, "{skill}: this publisher cannot install {tier:?} skills")
            }
            RuntimeError::PermissionDenied { skill, permission } => {
                write!(f, "{skill} was not granted {permission:?}")
            }
            RuntimeError::Skill(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

struct Installed {
    skill: Box<dyn Skill>,
    granted: HashSet<Permission>,
}

#[derive(Default)]
pub struct Runtime {
    skills: BTreeMap<String, Installed>,
}

impl Runtime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs a skill and grants it the listed permissions.
    pub fn install(
        &mut self,
        publisher: Publisher,
        skill: Box<dyn Skill>,
        granted: &[Permission],
    ) -> Result<(), RuntimeError> {
        let manifest = skill.manifest();
        let allowed = match manifest.tier {
            Tier::Critical | Tier::System => publisher == Publisher::Business,
            Tier::Store => publisher != Publisher::OnDeviceModel,
            Tier::AiAuthored => true,
        };
        if !allowed {
            return Err(RuntimeError::TierNotAllowed {
                skill: manifest.name.clone(),
                tier: manifest.tier,
            });
        }
        if self.skills.contains_key(&manifest.name) {
            return Err(RuntimeError::AlreadyInstalled(manifest.name.clone()));
        }
        let name = manifest.name.clone();
        self.skills.insert(
            name,
            Installed {
                skill,
                granted: granted.iter().copied().collect(),
            },
        );
        Ok(())
    }

    /// Installs a business skill with every permission it declares.
    pub fn install_trusted(&mut self, skill: Box<dyn Skill>) -> Result<(), RuntimeError> {
        let granted = skill.manifest().permissions.clone();
        self.install(Publisher::Business, skill, &granted)
    }

    /// Manifests of every installed skill, for the model to choose from.
    pub fn manifests(&self) -> Vec<&Manifest> {
        self.skills.values().map(|i| i.skill.manifest()).collect()
    }

    pub fn invoke(&self, name: &str, args: &Value) -> Result<Output, RuntimeError> {
        let installed = self
            .skills
            .get(name)
            .ok_or_else(|| RuntimeError::UnknownSkill(name.to_string()))?;
        for permission in &installed.skill.manifest().permissions {
            if !installed.granted.contains(permission) {
                return Err(RuntimeError::PermissionDenied {
                    skill: name.to_string(),
                    permission: *permission,
                });
            }
        }
        installed.skill.invoke(args).map_err(RuntimeError::Skill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Echo(Manifest);

    impl Skill for Echo {
        fn manifest(&self) -> &Manifest {
            &self.0
        }
        fn invoke(&self, args: &Value) -> Result<Output, SkillError> {
            Ok(Output::Data {
                value: args.clone(),
            })
        }
    }

    fn echo(tier: Tier, permissions: Vec<Permission>) -> Box<dyn Skill> {
        Box::new(Echo(Manifest {
            name: "test.echo".into(),
            description: "echoes its arguments".into(),
            tier,
            permissions,
            input_schema: Value::Null,
        }))
    }

    #[test]
    fn only_the_business_installs_critical_skills() {
        let mut rt = Runtime::new();
        for publisher in [Publisher::ThirdParty, Publisher::OnDeviceModel] {
            let err = rt
                .install(publisher, echo(Tier::Critical, vec![]), &[])
                .unwrap_err();
            assert!(matches!(err, RuntimeError::TierNotAllowed { .. }));
        }
        rt.install(Publisher::Business, echo(Tier::Critical, vec![]), &[])
            .unwrap();
    }

    #[test]
    fn model_cannot_install_store_skills_as_its_own() {
        let mut rt = Runtime::new();
        let err = rt
            .install(Publisher::OnDeviceModel, echo(Tier::Store, vec![]), &[])
            .unwrap_err();
        assert!(matches!(err, RuntimeError::TierNotAllowed { .. }));
    }

    #[test]
    fn ungranted_permission_blocks_the_call() {
        let mut rt = Runtime::new();
        rt.install(
            Publisher::ThirdParty,
            echo(Tier::Store, vec![Permission::Camera]),
            &[],
        )
        .unwrap();
        let err = rt.invoke("test.echo", &json!({})).unwrap_err();
        assert_eq!(
            err,
            RuntimeError::PermissionDenied {
                skill: "test.echo".into(),
                permission: Permission::Camera
            }
        );
    }

    #[test]
    fn granted_skill_runs() {
        let mut rt = Runtime::new();
        rt.install(
            Publisher::ThirdParty,
            echo(Tier::Store, vec![Permission::Camera]),
            &[Permission::Camera],
        )
        .unwrap();
        let out = rt.invoke("test.echo", &json!({"a": 1})).unwrap();
        assert_eq!(
            out,
            Output::Data {
                value: json!({"a": 1})
            }
        );
    }
}
