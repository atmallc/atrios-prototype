//! Basic system skills that ship with Pixie.
//!
//! The camera uses a fake backend for now: it writes a test-pattern image so
//! the whole request-to-photo path runs on the emulator or a dev machine.

use pixie_skill::{Manifest, Output, Permission, Skill, SkillError, Tier};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn manifest(
    name: &str,
    description: &str,
    permissions: Vec<Permission>,
    input_schema: Value,
) -> Manifest {
    Manifest {
        name: name.into(),
        description: description.into(),
        tier: Tier::System,
        permissions,
        input_schema,
    }
}

fn no_args() -> Value {
    json!({"type": "object", "properties": {}})
}

/// Where a photo comes from. The fake one draws a test pattern; a real one
/// will read the Pixel's camera through Linux V4L2.
pub trait CameraBackend: Send + Sync {
    /// Returns a binary PPM (P6) image.
    fn capture(&self) -> Result<Vec<u8>, SkillError>;
}

/// Colour bars, 320x240.
pub struct FakeCamera;

impl CameraBackend for FakeCamera {
    fn capture(&self) -> Result<Vec<u8>, SkillError> {
        const W: usize = 320;
        const H: usize = 240;
        const BARS: [[u8; 3]; 7] = [
            [255, 255, 255],
            [255, 255, 0],
            [0, 255, 255],
            [0, 255, 0],
            [255, 0, 255],
            [255, 0, 0],
            [0, 0, 255],
        ];
        let mut img = format!("P6\n{W} {H}\n255\n").into_bytes();
        for _ in 0..H {
            for x in 0..W {
                img.extend_from_slice(&BARS[x * BARS.len() / W]);
            }
        }
        Ok(img)
    }
}

pub struct TakePhoto {
    manifest: Manifest,
    backend: Box<dyn CameraBackend>,
    dir: PathBuf,
    count: AtomicU32,
}

impl TakePhoto {
    pub fn new(backend: Box<dyn CameraBackend>, dir: impl Into<PathBuf>) -> Self {
        Self {
            manifest: manifest(
                "camera.take_photo",
                "Take a photo with the camera and save it",
                vec![Permission::Camera, Permission::Storage],
                no_args(),
            ),
            backend,
            dir: dir.into(),
            count: AtomicU32::new(0),
        }
    }
}

impl Skill for TakePhoto {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn invoke(&self, _args: &Value) -> Result<Output, SkillError> {
        let image = self.backend.capture()?;
        fs::create_dir_all(&self.dir).map_err(|e| SkillError::Failed(e.to_string()))?;
        let n = self.count.fetch_add(1, Ordering::Relaxed) + 1;
        let path = self.dir.join(format!("photo-{n}.ppm"));
        fs::write(&path, image).map_err(|e| SkillError::Failed(e.to_string()))?;
        Ok(Output::File {
            path: path.display().to_string(),
            mime: "image/x-portable-pixmap".into(),
        })
    }
}

pub struct ShowText {
    manifest: Manifest,
}

impl ShowText {
    pub fn new() -> Self {
        Self {
            manifest: manifest(
                "display.show_text",
                "Show a line of text to the user",
                vec![Permission::Display],
                json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
            ),
        }
    }
}

impl Default for ShowText {
    fn default() -> Self {
        Self::new()
    }
}

impl Skill for ShowText {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn invoke(&self, args: &Value) -> Result<Output, SkillError> {
        let text = args
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| SkillError::InvalidArgs("`text` must be a string".into()))?;
        Ok(Output::Text { text: text.into() })
    }
}

pub struct Clock {
    manifest: Manifest,
}

impl Clock {
    pub fn new() -> Self {
        Self {
            manifest: manifest(
                "status.time",
                "Current time (UTC)",
                vec![Permission::Clock],
                no_args(),
            ),
        }
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl Skill for Clock {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn invoke(&self, _args: &Value) -> Result<Output, SkillError> {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| SkillError::Failed(e.to_string()))?
            .as_secs();
        let (h, m) = ((secs / 3600) % 24, (secs / 60) % 60);
        Ok(Output::Text {
            text: format!("{h:02}:{m:02} UTC"),
        })
    }
}

/// Reads battery level from Linux sysfs, the same path the Pixel exposes.
pub struct Battery {
    manifest: Manifest,
    power_supply: PathBuf,
}

impl Battery {
    pub fn new() -> Self {
        Self::with_sysfs("/sys/class/power_supply")
    }

    pub fn with_sysfs(power_supply: impl Into<PathBuf>) -> Self {
        Self {
            manifest: manifest(
                "status.battery",
                "Battery charge level",
                vec![Permission::PowerStatus],
                no_args(),
            ),
            power_supply: power_supply.into(),
        }
    }
}

impl Default for Battery {
    fn default() -> Self {
        Self::new()
    }
}

impl Skill for Battery {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn invoke(&self, _args: &Value) -> Result<Output, SkillError> {
        let entries = fs::read_dir(&self.power_supply)
            .map_err(|_| SkillError::Unavailable("no power supply information".into()))?;
        for entry in entries.flatten() {
            if let Ok(level) = fs::read_to_string(entry.path().join("capacity")) {
                return Ok(Output::Text {
                    text: format!("{}%", level.trim()),
                });
            }
        }
        Err(SkillError::Unavailable("no battery found".into()))
    }
}

/// Every basic skill, ready to install.
pub fn all(photo_dir: impl Into<PathBuf>) -> Vec<Box<dyn Skill>> {
    vec![
        Box::new(TakePhoto::new(Box::new(FakeCamera), photo_dir)),
        Box::new(ShowText::new()),
        Box::new(Clock::new()),
        Box::new(Battery::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pixie-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn fake_camera_saves_a_photo() {
        let dir = temp_dir("camera");
        let skill = TakePhoto::new(Box::new(FakeCamera), &dir);
        let Output::File { path, .. } = skill.invoke(&json!({})).unwrap() else {
            panic!("expected a file");
        };
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"P6\n320 240\n255\n"));
        assert_eq!(bytes.len(), 15 + 320 * 240 * 3);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn show_text_requires_text() {
        let skill = ShowText::new();
        assert!(matches!(
            skill.invoke(&json!({})),
            Err(SkillError::InvalidArgs(_))
        ));
        assert_eq!(
            skill.invoke(&json!({"text": "hi"})).unwrap(),
            Output::Text { text: "hi".into() }
        );
    }

    #[test]
    fn battery_reads_capacity() {
        let dir = temp_dir("battery");
        fs::create_dir_all(dir.join("BAT0")).unwrap();
        fs::write(dir.join("BAT0/capacity"), "87\n").unwrap();
        let skill = Battery::with_sysfs(&dir);
        assert_eq!(
            skill.invoke(&json!({})).unwrap(),
            Output::Text { text: "87%".into() }
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
