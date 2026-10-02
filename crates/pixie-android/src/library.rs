//! Skills the owner has approved, kept as one JSON file each so they survive
//! a restart (the phone's storage persists, unlike the bare-metal build's).

use pixie_script::SkillSpec;
use std::fs;
use std::path::PathBuf;

pub struct Library {
    dir: PathBuf,
}

impl Library {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn data_dir(&self) -> PathBuf {
        self.dir.join("data")
    }

    fn skills_dir(&self) -> PathBuf {
        self.dir.join("skills")
    }

    pub fn save(&self, spec: &SkillSpec) -> Result<(), String> {
        let dir = self.skills_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(spec).map_err(|e| e.to_string())?;
        fs::write(dir.join(format!("{}.json", spec.name)), json).map_err(|e| e.to_string())
    }

    pub fn load(&self) -> Vec<SkillSpec> {
        let mut specs: Vec<SkillSpec> = fs::read_dir(self.skills_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| fs::read_to_string(e.path()).ok())
            .filter_map(|s| serde_json::from_str(&s).ok())
            .collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn saved_skills_come_back() {
        let dir = std::env::temp_dir().join(format!("pixie-lib-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let lib = Library::new(&dir);
        assert!(lib.load().is_empty());
        let spec = SkillSpec {
            name: "user.demo".into(),
            description: "d".into(),
            permissions: vec![],
            input_schema: Value::Null,
            code: "1".into(),
        };
        lib.save(&spec).unwrap();
        assert_eq!(lib.load(), vec![spec]);
        fs::remove_dir_all(dir).unwrap();
    }
}
