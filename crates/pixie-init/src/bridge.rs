//! The "brain": requests Pixie has no skill for go to a bridge program on the
//! computer at the other end of the USB cable, which asks a model. The bridge
//! also keeps the skills the owner has approved, because the phone's own
//! storage is RAM and starts empty on every boot.
//!
//! One JSON line each way over TCP; the first field of a request is `op`.

use crate::usb;
use pixie_script::{BrainReply, SkillSpec};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

pub const PORT: u16 = 2324;

fn exchange(message: Value, patience: Duration) -> Result<String, String> {
    let addr: SocketAddr = format!("{}:{PORT}", usb::HOST_IP)
        .parse()
        .map_err(|e| format!("{e}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|e| format!("can't reach the bridge: {e}"))?;
    stream.set_read_timeout(Some(patience)).map_err(|e| e.to_string())?;
    writeln!(stream, "{message}").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| format!("no answer: {e}"))?;
    if line.trim().is_empty() {
        Err("empty answer".into())
    } else {
        Ok(line)
    }
}

/// Asks the brain. `catalog` lists every installed skill (name, description,
/// input schema) so it can choose one to call or see what is missing.
pub fn ask(request: &str, catalog: Value) -> Result<BrainReply, String> {
    let line = exchange(
        json!({ "op": "ask", "request": request, "skills": catalog }),
        Duration::from_secs(240),
    )?;
    serde_json::from_str(&line).map_err(|e| format!("bad answer from the bridge: {e}"))
}

/// Fetches the skills the owner approved earlier.
pub fn sync() -> Result<Vec<SkillSpec>, String> {
    let line = exchange(json!({ "op": "sync" }), Duration::from_secs(10))?;
    #[derive(serde::Deserialize)]
    struct Library {
        skills: Vec<SkillSpec>,
    }
    serde_json::from_str::<Library>(&line)
        .map(|l| l.skills)
        .map_err(|e| format!("bad library: {e}"))
}

/// Tells the bridge to keep a skill the owner just approved.
pub fn approve(spec: &SkillSpec) -> Result<(), String> {
    exchange(json!({ "op": "approve", "skill": spec }), Duration::from_secs(10)).map(|_| ())
}
