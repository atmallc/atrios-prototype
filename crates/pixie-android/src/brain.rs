//! The brain: requests Pixie has no skill for go to a bridge program on the
//! computer, which asks a model and sends one JSON line back. On a phone
//! connected with `adb reverse tcp:2324 tcp:2324`, the bridge is at
//! 127.0.0.1:2324.

use pixie_script::BrainReply;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Asks the brain. `catalog` lists every installed skill (name, description,
/// input schema) so it can choose one to call or see what is missing.
pub fn ask(bridge: SocketAddr, request: &str, catalog: Value) -> Result<BrainReply, String> {
    let mut stream = TcpStream::connect_timeout(&bridge, Duration::from_secs(3))
        .map_err(|e| format!("can't reach the bridge: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(240)))
        .map_err(|e| e.to_string())?;
    let message = json!({ "op": "ask", "request": request, "skills": catalog });
    writeln!(stream, "{message}").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| format!("no answer: {e}"))?;
    serde_json::from_str(&line).map_err(|e| format!("bad answer from the bridge: {e}"))
}

/// Asks the bridge to search the web and returns its short answer.
pub fn search(bridge: SocketAddr, query: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(&bridge, Duration::from_secs(3))
        .map_err(|e| format!("can't reach the bridge: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|e| e.to_string())?;
    writeln!(stream, "{}", json!({ "op": "search", "query": query })).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| format!("no answer: {e}"))?;
    let reply: Value = serde_json::from_str(&line).map_err(|e| format!("bad answer: {e}"))?;
    reply
        .get("text")
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| "the bridge sent no text".to_string())
}

/// Asks the bridge to synthesize `text` and returns the audio as a WAV file.
pub fn tts(bridge: SocketAddr, text: &str) -> Result<Vec<u8>, String> {
    let mut stream = TcpStream::connect_timeout(&bridge, Duration::from_secs(3))
        .map_err(|e| format!("can't reach the bridge: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;
    writeln!(stream, "{}", json!({ "op": "tts", "text": text })).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| format!("no answer: {e}"))?;
    let reply: Value = serde_json::from_str(&line).map_err(|e| format!("bad answer: {e}"))?;
    let hex = reply
        .get("wav_hex")
        .and_then(Value::as_str)
        .ok_or_else(|| reply.get("error").and_then(Value::as_str).unwrap_or("no audio").to_string())?;
    from_hex(hex)
}

fn from_hex(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("bad audio data".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| "bad audio data".to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::from_hex;

    #[test]
    fn decodes_hex() {
        assert_eq!(from_hex("00ff10").unwrap(), vec![0, 255, 16]);
        assert!(from_hex("abc").is_err());
        assert!(from_hex("zz").is_err());
    }
}
