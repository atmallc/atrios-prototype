//! Basic system skills that ship with Pixie.
//!
//! The camera uses a fake backend for now: it writes a test-pattern image so
//! the whole request-to-photo path runs on the emulator or a dev machine.

use pixie_skill::{Manifest, Output, Permission, Skill, SkillError, Tier};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

/// Longest text `files.open` returns, so a big file cannot flood the screen.
const OPEN_LIMIT: usize = 1000;

/// Opens a file and returns its text, or lists a directory.
pub struct OpenFile {
    manifest: Manifest,
}

impl OpenFile {
    pub fn new() -> Self {
        Self {
            manifest: manifest(
                "files.open",
                "Open a file on the device and show its contents, or list a directory",
                vec![Permission::Storage],
                json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
            ),
        }
    }
}

impl Default for OpenFile {
    fn default() -> Self {
        Self::new()
    }
}

impl Skill for OpenFile {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn invoke(&self, args: &Value) -> Result<Output, SkillError> {
        use std::io::Read;
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| SkillError::InvalidArgs("`path` must be a string".into()))?;
        let meta = fs::metadata(path).map_err(|e| SkillError::Failed(format!("{path}: {e}")))?;
        if meta.is_dir() {
            let mut names: Vec<String> = fs::read_dir(path)
                .map_err(|e| SkillError::Failed(format!("{path}: {e}")))?
                .flatten()
                .map(|e| {
                    let dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    format!("{}{}", e.file_name().to_string_lossy(), if dir { "/" } else { "" })
                })
                .collect();
            names.sort();
            return Ok(Output::Text {
                text: truncate(names.join(" ")),
            });
        }
        // Special files such as /proc entries report size 0, so read up to the limit.
        let mut bytes = Vec::new();
        fs::File::open(path)
            .and_then(|f| f.take(OPEN_LIMIT as u64 + 1).read_to_end(&mut bytes))
            .map_err(|e| SkillError::Failed(format!("{path}: {e}")))?;
        if bytes.contains(&0) {
            return Ok(Output::Text {
                text: format!("{path}: binary file, {} bytes", meta.len()),
            });
        }
        Ok(Output::Text {
            text: truncate(String::from_utf8_lossy(&bytes).trim_end().to_string()),
        })
    }
}

fn truncate(mut text: String) -> String {
    if text.len() > OPEN_LIMIT {
        let mut end = OPEN_LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("...");
    }
    text
}

/// How long after the last key tap someone still counts as typing.
const TYPING_WINDOW: Duration = Duration::from_secs(5);

#[derive(Default)]
struct TypingState {
    draft: String,
    last_key: Option<Instant>,
}

/// Shared between the on-screen keyboard, which reports each change, and the
/// `keyboard.typing` skill, which reads it.
#[derive(Clone, Default)]
pub struct TypingMonitor(Arc<Mutex<TypingState>>);

impl TypingMonitor {
    /// The keyboard calls this whenever the draft text changes.
    pub fn update(&self, draft: &str) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.draft = draft.to_string();
        state.last_key = Some(Instant::now());
    }

    fn describe(&self) -> String {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let recent = state.last_key.is_some_and(|t| t.elapsed() < TYPING_WINDOW);
        match (state.draft.is_empty(), recent) {
            (false, true) => format!("typing: {}", state.draft),
            (false, false) => format!("paused: {}", state.draft),
            (true, _) => "idle".into(),
        }
    }
}

/// Reports whether someone is typing on the on-screen keyboard, and what.
pub struct Typing {
    manifest: Manifest,
    monitor: TypingMonitor,
}

impl Typing {
    pub fn new(monitor: TypingMonitor) -> Self {
        Self {
            manifest: manifest(
                "keyboard.typing",
                "Tell whether someone is typing on the on-screen keyboard, and the text so far",
                vec![Permission::Display],
                no_args(),
            ),
            monitor,
        }
    }
}

impl Skill for Typing {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn invoke(&self, _args: &Value) -> Result<Output, SkillError> {
        Ok(Output::Text {
            text: self.monitor.describe(),
        })
    }
}

/// Where web search results come from. The phone has no search service of its
/// own, so `pixied` asks the brain on the computer; a future build can call a
/// search API directly.
pub trait WebBackend: Send + Sync {
    /// A short plain-text answer for `query`, with source links.
    fn search(&self, query: &str) -> Result<String, SkillError>;
}

/// Searches the internet. The query leaves the device, which is why the skill
/// declares the `network` permission and the owner sees it listed.
pub struct WebSearch {
    manifest: Manifest,
    backend: Box<dyn WebBackend>,
}

impl WebSearch {
    pub fn new(backend: Box<dyn WebBackend>) -> Self {
        Self {
            manifest: manifest(
                "web.search",
                "Search the internet for current information (news, weather, prices, facts) and return a short answer with sources",
                vec![Permission::Network],
                json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
            ),
            backend,
        }
    }
}

impl Skill for WebSearch {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn invoke(&self, args: &Value) -> Result<Output, SkillError> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| SkillError::InvalidArgs("`query` must be a non-empty string".into()))?;
        if query.len() > 300 {
            return Err(SkillError::InvalidArgs("query is longer than 300 characters".into()));
        }
        Ok(Output::Text {
            text: truncate(self.backend.search(query)?),
        })
    }
}

/// Where spoken audio comes from. The phone has no speech engine of its own,
/// so `pixied` has the computer synthesize it and the chat page plays it.
pub trait SpeechBackend: Send + Sync {
    /// Starts speaking `text`; returns a short status line for the owner.
    fn speak(&self, text: &str) -> Result<String, SkillError>;
}

/// Says something out loud.
pub struct Speak {
    manifest: Manifest,
    backend: Box<dyn SpeechBackend>,
}

impl Speak {
    pub fn new(backend: Box<dyn SpeechBackend>) -> Self {
        Self {
            manifest: manifest(
                "audio.speak",
                "Say something out loud through the phone's speaker",
                vec![Permission::Audio],
                json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
            ),
            backend,
        }
    }
}

impl Skill for Speak {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn invoke(&self, args: &Value) -> Result<Output, SkillError> {
        let text = args
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| SkillError::InvalidArgs("`text` must be a non-empty string".into()))?;
        if text.len() > 500 {
            return Err(SkillError::InvalidArgs("text is longer than 500 characters".into()));
        }
        Ok(Output::Text {
            text: self.backend.speak(text)?,
        })
    }
}

/// Every basic skill, ready to install.
pub fn all(photo_dir: impl Into<PathBuf>) -> Vec<Box<dyn Skill>> {
    all_with(photo_dir, TypingMonitor::default())
}

/// Like `all`, with the typing monitor the keyboard reports to.
pub fn all_with(photo_dir: impl Into<PathBuf>, typing: TypingMonitor) -> Vec<Box<dyn Skill>> {
    vec![
        Box::new(TakePhoto::new(Box::new(FakeCamera), photo_dir)),
        Box::new(ShowText::new()),
        Box::new(Clock::new()),
        Box::new(Battery::new()),
        Box::new(OpenFile::new()),
        Box::new(Typing::new(typing)),
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
    fn open_file_reads_text_and_lists_directories() {
        let dir = temp_dir("open");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("note.txt"), "hello pixie\n").unwrap();
        fs::write(dir.join("blob.bin"), [1u8, 0, 2]).unwrap();
        let skill = OpenFile::new();
        let open = |name: &str| {
            skill.invoke(&json!({"path": dir.join(name).to_str().unwrap()}))
        };
        assert_eq!(open("note.txt").unwrap(), Output::Text { text: "hello pixie".into() });
        assert!(matches!(open("blob.bin").unwrap(), Output::Text { text } if text.contains("binary")));
        assert!(matches!(open("missing"), Err(SkillError::Failed(_))));
        let Output::Text { text } = skill.invoke(&json!({"path": dir.to_str().unwrap()})).unwrap() else {
            panic!("expected text");
        };
        assert_eq!(text, "blob.bin note.txt sub/");
        assert!(matches!(skill.invoke(&json!({})), Err(SkillError::InvalidArgs(_))));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn typing_skill_reports_the_draft() {
        let monitor = TypingMonitor::default();
        let skill = Typing::new(monitor.clone());
        let say = |s: &Typing| match s.invoke(&json!({})).unwrap() {
            Output::Text { text } => text,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(say(&skill), "idle");
        monitor.update("hel");
        assert_eq!(say(&skill), "typing: hel");
        monitor.0.lock().unwrap().last_key = Some(Instant::now() - Duration::from_secs(60));
        assert_eq!(say(&skill), "paused: hel");
        monitor.update("");
        assert_eq!(say(&skill), "idle");
    }

    struct FakeWeb;
    impl WebBackend for FakeWeb {
        fn search(&self, query: &str) -> Result<String, SkillError> {
            Ok(format!("results for {query}"))
        }
    }

    #[test]
    fn web_search_passes_the_query_to_its_backend() {
        let skill = WebSearch::new(Box::new(FakeWeb));
        assert_eq!(skill.manifest().permissions, vec![Permission::Network]);
        assert_eq!(
            skill.invoke(&json!({"query": " rust lang "})).unwrap(),
            Output::Text { text: "results for rust lang".into() }
        );
        assert!(matches!(skill.invoke(&json!({})), Err(SkillError::InvalidArgs(_))));
        assert!(matches!(skill.invoke(&json!({"query": "  "})), Err(SkillError::InvalidArgs(_))));
        let long = "x".repeat(301);
        assert!(matches!(skill.invoke(&json!({"query": long})), Err(SkillError::InvalidArgs(_))));
    }

    struct FakeVoice(Mutex<Vec<String>>);
    impl SpeechBackend for std::sync::Arc<FakeVoice> {
        fn speak(&self, text: &str) -> Result<String, SkillError> {
            self.0.lock().unwrap().push(text.to_string());
            Ok("Speaking.".into())
        }
    }

    #[test]
    fn speak_hands_trimmed_text_to_its_backend() {
        let voice = std::sync::Arc::new(FakeVoice(Mutex::new(Vec::new())));
        let skill = Speak::new(Box::new(voice.clone()));
        assert_eq!(skill.manifest().permissions, vec![Permission::Audio]);
        assert_eq!(
            skill.invoke(&json!({"text": "  hello there "})).unwrap(),
            Output::Text { text: "Speaking.".into() }
        );
        assert_eq!(*voice.0.lock().unwrap(), vec!["hello there".to_string()]);
        assert!(matches!(skill.invoke(&json!({})), Err(SkillError::InvalidArgs(_))));
        let long = "x".repeat(501);
        assert!(matches!(skill.invoke(&json!({"text": long})), Err(SkillError::InvalidArgs(_))));
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
