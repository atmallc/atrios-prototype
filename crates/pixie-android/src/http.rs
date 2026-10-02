//! A tiny HTTP server for the chat page, so Pixie can be used from the phone's
//! own browser (or the computer's, over `adb forward tcp:8080 tcp:8080`).
//!
//! Routes:
//!   GET  /            the chat page
//!   POST /api/ask     {"text": "..."} -> {"reply": "...", "pending": {...} | null}
//!   POST /api/tts     {"text": "..."} -> audio/wav of the text spoken aloud
//!   GET  /api/state   {"pending": ..., "brain": true|false, "skills": [...], "speak": [...]}
//!
//! Hand-rolled on std so the phone binary stays small. One request per
//! connection, which is all a chat page needs.

use crate::{respond, Shared};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const INDEX: &str = include_str!("index.html");
const MAX_BODY: usize = 16 * 1024;

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: value.to_string().into_bytes(),
        }
    }
}

/// Handles one connection.
pub fn serve(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let response = match read_request(&mut stream) {
        Ok((method, path, body)) => route(shared, &method, &path, &body),
        Err(e) => Response::json(400, json!({ "error": e })),
    };
    let reason = match response.status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        502 => "Bad Gateway",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.body);
}

/// Reads the request line, headers and body. Returns (method, path, body).
fn read_request(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Err("bad request line".into());
    };
    let (method, path) = (method.to_string(), path.to_string());
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).map_err(|e| e.to_string())?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().map_err(|_| "bad content-length")?;
            }
        }
    }
    if length > MAX_BODY {
        return Err("body too large".into());
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok((method, path, body))
}

/// Maps a request to a response. Separate from the socket code so tests can call it.
pub fn route(shared: &Shared, method: &str, path: &str, body: &[u8]) -> Response {
    let path = path.split('?').next().unwrap_or("");
    match (method, path) {
        ("GET", "/" | "/index.html") => Response {
            status: 200,
            content_type: "text/html",
            body: INDEX.as_bytes().to_vec(),
        },
        ("GET", "/api/state") => Response::json(200, state(shared)),
        ("POST", "/api/ask") => {
            let text = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|v| v.get("text").and_then(Value::as_str).map(str::trim).map(String::from))
                .filter(|t| !t.is_empty());
            let Some(text) = text else {
                return Response::json(400, json!({ "error": "send {\"text\": \"...\"}" }));
            };
            let reply = {
                let mut session = shared.web.lock().unwrap_or_else(|e| e.into_inner());
                respond(shared, &mut session, &text)
            };
            let mut out = state(shared);
            out["reply"] = Value::String(reply);
            Response::json(200, out)
        }
        ("POST", "/api/tts") => {
            let text = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|v| v.get("text").and_then(Value::as_str).map(str::trim).map(String::from))
                .filter(|t| !t.is_empty() && t.len() <= 500);
            let Some(text) = text else {
                return Response::json(400, json!({ "error": "send {\"text\": \"...\"} (up to 500 characters)" }));
            };
            match crate::brain::tts(shared.bridge, &text) {
                Ok(wav) => Response { status: 200, content_type: "audio/wav", body: wav },
                Err(e) => Response::json(502, json!({ "error": e })),
            }
        }
        ("GET" | "POST", _) => Response::json(404, json!({ "error": "not found" })),
        _ => Response::json(400, json!({ "error": "unsupported method" })),
    }
}

/// What the page needs to draw itself: the waiting skill, the skills, whether the brain answers.
fn state(shared: &Shared) -> Value {
    let pending = shared
        .web
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pending
        .as_ref()
        .map(|(spec, _)| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "permissions": spec.permissions.iter().filter_map(|p| serde_json::to_value(p).ok()).collect::<Vec<_>>(),
                "code": spec.code,
            })
        });
    let skills: Vec<Value> = shared
        .agent
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .runtime()
        .manifests()
        .iter()
        .map(|m| {
            json!({
                "name": m.name,
                "description": m.description,
                "tier": m.tier,
                "permissions": m.permissions,
            })
        })
        .collect();
    // Whatever skills asked to say since the last look; the page speaks them once.
    let speak: Vec<String> = std::mem::take(&mut *shared.outbox.lock().unwrap_or_else(|e| e.into_inner()));
    json!({ "pending": pending, "skills": skills, "brain": brain_reachable(shared), "speak": speak })
}

fn brain_reachable(shared: &Shared) -> bool {
    TcpStream::connect_timeout(&shared.bridge, Duration::from_millis(400)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{library::Library, Session};
    use pixie_agent::{Agent, KeywordGuardrail, KeywordModel};
    use pixie_runtime::Runtime;
    use std::sync::Mutex;

    fn shared() -> Shared {
        let dir = std::env::temp_dir().join(format!("pixie-http-{}", std::process::id()));
        let mut runtime = Runtime::new();
        for skill in pixie_skills_basic::all(dir.join("photos")) {
            runtime.install_trusted(skill).unwrap();
        }
        Shared {
            agent: Mutex::new(Agent::new(
                runtime,
                Box::new(KeywordModel),
                Box::new(KeywordGuardrail::default()),
            )),
            library: Library::new(dir),
            // Nothing listens here, so the brain is "offline".
            bridge: "127.0.0.1:9".parse().unwrap(),
            web: Mutex::new(Session::default()),
            outbox: Default::default(),
        }
    }

    fn json_of(r: &Response) -> Value {
        serde_json::from_slice(&r.body).unwrap()
    }

    #[test]
    fn serves_the_chat_page() {
        let r = route(&shared(), "GET", "/", b"");
        assert_eq!(r.status, 200);
        assert_eq!(r.content_type, "text/html");
        assert!(String::from_utf8(r.body).unwrap().contains("<title>Pixie</title>"));
    }

    #[test]
    fn state_lists_skills_and_brain_status() {
        let r = route(&shared(), "GET", "/api/state", b"");
        let v = json_of(&r);
        assert_eq!(v["brain"], false);
        assert!(v["pending"].is_null());
        assert!(v["skills"].as_array().unwrap().iter().any(|s| s["name"] == "status.time"));
    }

    #[test]
    fn asking_runs_a_skill_and_returns_the_reply() {
        let r = route(&shared(), "POST", "/api/ask", br#"{"text": "what time is it"}"#);
        assert_eq!(r.status, 200);
        assert!(json_of(&r)["reply"].as_str().unwrap().ends_with("UTC"));
    }

    #[test]
    fn unknown_requests_without_a_brain_say_so() {
        let r = route(&shared(), "POST", "/api/ask", br#"{"text": "write me a poem about the sea"}"#);
        assert!(json_of(&r)["reply"].as_str().unwrap().contains("out of reach"));
    }

    #[test]
    fn spoken_words_are_handed_to_the_page_once() {
        let s = shared();
        s.outbox.lock().unwrap().push("hello".into());
        assert_eq!(json_of(&route(&s, "GET", "/api/state", b""))["speak"], json!(["hello"]));
        assert_eq!(json_of(&route(&s, "GET", "/api/state", b""))["speak"], json!([]));
    }

    #[test]
    fn tts_needs_text_and_a_reachable_bridge() {
        let s = shared();
        assert_eq!(route(&s, "POST", "/api/tts", br#"{"text": ""}"#).status, 400);
        // Nothing listens on the test bridge address.
        assert_eq!(route(&s, "POST", "/api/tts", br#"{"text": "hi"}"#).status, 502);
    }

    #[test]
    fn bad_requests_are_refused() {
        let s = shared();
        assert_eq!(route(&s, "POST", "/api/ask", b"not json").status, 400);
        assert_eq!(route(&s, "POST", "/api/ask", br#"{"text": "  "}"#).status, 400);
        assert_eq!(route(&s, "GET", "/nope", b"").status, 404);
        assert_eq!(route(&s, "DELETE", "/", b"").status, 400);
    }
}
