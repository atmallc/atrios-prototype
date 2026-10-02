//! `pixied`: Pixie's agent as a background service on a phone running Android.
//!
//! It owns the skill runtime, answers the keyword requests itself, asks the
//! brain (a bridge on the computer) for everything else, and installs the
//! skills the owner approves. People talk to it over a line-based TCP port:
//!
//!     adb forward tcp:2323 tcp:2323 && nc localhost 2323
//!
//! or through the chat page on port 8080 (in the phone's browser, or the
//! computer's after `adb forward tcp:8080 tcp:8080`). It reaches the brain
//! through `adb reverse tcp:2324 tcp:2324`.

mod brain;
mod http;
mod library;

use library::Library;
use pixie_agent::{Agent, KeywordGuardrail, KeywordModel, Response, NO_SKILL};
use pixie_runtime::{Publisher, Runtime};
use pixie_script::{BrainReply, SkillSpec};
use pixie_skill::{Output, SkillError};
use pixie_skills_basic::{Speak, SpeechBackend, WebBackend, WebSearch};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

const HELP: &str = "Ask for: take a photo, battery, time, show <text>, open <path>, search <topic>. \
Anything else goes to the brain, which can also write new skills. \
Commands: skills, help.";

struct Shared {
    agent: Mutex<Agent>,
    library: Library,
    bridge: SocketAddr,
    /// The chat page's conversation; every browser tab shares it.
    web: Mutex<Session>,
    /// Things to say aloud, waiting for the chat page to pick them up and play them.
    outbox: Arc<Mutex<Vec<String>>>,
}

/// What one connection remembers between lines.
#[derive(Default)]
struct Session {
    /// A skill the brain wrote, waiting for the owner to say yes or no, and the
    /// arguments to run it with once approved.
    pending: Option<(SkillSpec, Option<Value>)>,
}

fn main() {
    let mut listen = "127.0.0.1:2323".to_string();
    let mut web = "127.0.0.1:8080".to_string();
    let mut bridge = "127.0.0.1:2324".to_string();
    let mut dir = "/data/local/tmp/pixie".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().unwrap_or_default();
        match flag.as_str() {
            "--listen" => listen = value,
            "--http" => web = value,
            "--bridge" => bridge = value,
            "--dir" => dir = value,
            _ => {
                eprintln!("usage: pixied [--listen ADDR] [--http ADDR] [--bridge ADDR] [--dir PATH]");
                std::process::exit(2);
            }
        }
    }
    let bridge: SocketAddr = bridge.parse().unwrap_or_else(|e| {
        eprintln!("pixied: bad --bridge: {e}");
        std::process::exit(2);
    });

    let library = Library::new(&dir);
    let mut runtime = Runtime::new();
    for skill in pixie_skills_basic::all(format!("{dir}/photos")) {
        if let Err(e) = runtime.install_trusted(skill) {
            eprintln!("pixied: {e}");
        }
    }
    if let Err(e) = runtime.install_trusted(Box::new(WebSearch::new(Box::new(BridgeSearch(bridge))))) {
        eprintln!("pixied: {e}");
    }
    let outbox: Arc<Mutex<Vec<String>>> = Arc::default();
    if let Err(e) = runtime.install_trusted(Box::new(Speak::new(Box::new(PageVoice(outbox.clone()))))) {
        eprintln!("pixied: {e}");
    }
    let mut agent = Agent::new(
        runtime,
        Box::new(KeywordModel),
        Box::new(KeywordGuardrail::default()),
    );
    for spec in library.load() {
        match install(&mut agent, &spec, &library) {
            Ok(()) => eprintln!("pixied: loaded skill {}", spec.name),
            Err(e) => eprintln!("pixied: skipped {}: {e}", spec.name),
        }
    }

    let shared = Arc::new(Shared {
        agent: Mutex::new(agent),
        library,
        bridge,
        web: Mutex::new(Session::default()),
        outbox,
    });
    let listener = TcpListener::bind(&listen).unwrap_or_else(|e| {
        eprintln!("pixied: can't listen on {listen}: {e}");
        std::process::exit(1);
    });
    match TcpListener::bind(&web) {
        Ok(http_listener) => {
            eprintln!("pixied: chat page on http://{web}");
            let shared = shared.clone();
            std::thread::spawn(move || {
                for stream in http_listener.incoming().flatten() {
                    let shared = shared.clone();
                    std::thread::spawn(move || http::serve(stream, &shared));
                }
            });
        }
        Err(e) => eprintln!("pixied: no chat page, can't listen on {web}: {e}"),
    }
    eprintln!("pixied: listening on {listen}; brain at {bridge}; data in {dir}");
    for stream in listener.incoming().flatten() {
        let shared = shared.clone();
        std::thread::spawn(move || serve(stream, &shared));
    }
}

/// Web search through the brain on the computer.
struct BridgeSearch(SocketAddr);

impl WebBackend for BridgeSearch {
    fn search(&self, query: &str) -> Result<String, SkillError> {
        brain::search(self.0, query).map_err(SkillError::Unavailable)
    }
}

/// Speech: the words wait in the outbox, and the chat page fetches the audio
/// (`POST /api/tts`) and plays it through the phone's speaker.
struct PageVoice(Arc<Mutex<Vec<String>>>);

impl SpeechBackend for PageVoice {
    fn speak(&self, text: &str) -> Result<String, SkillError> {
        let mut outbox = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if outbox.len() >= 20 {
            outbox.remove(0);
        }
        outbox.push(text.to_string());
        Ok("Speaking (the Pixie page must be open on the phone to play it).".into())
    }
}

fn serve(stream: TcpStream, shared: &Shared) {
    let Ok(mut writer) = stream.try_clone() else { return };
    let _ = write!(writer, "pixie ready. type 'help'.\r\n> ");
    let mut session = Session::default();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let request = line.trim();
        if !request.is_empty() {
            let reply = respond(shared, &mut session, request);
            if write!(writer, "{}\r\n", reply.replace('\n', "\r\n")).is_err() {
                return;
            }
        }
        if write!(writer, "> ").and_then(|()| writer.flush()).is_err() {
            return;
        }
    }
}

/// Answers one request with text for the owner.
fn respond(shared: &Shared, session: &mut Session, request: &str) -> String {
    let lower = request.to_lowercase();

    // A written skill is waiting: the next yes or no is the owner's decision.
    if let Some((spec, run_args)) = session.pending.take() {
        return match lower.as_str() {
            "yes" | "y" | "install" => approve(shared, &spec, run_args),
            "no" | "n" | "cancel" => format!("Discarded {}.", spec.name),
            _ => {
                let name = spec.name.clone();
                session.pending = Some((spec, run_args));
                format!("Install {name}? Type yes or no.")
            }
        };
    }

    match lower.as_str() {
        "help" => return HELP.to_string(),
        "skills" => {
            let agent = shared.agent.lock().unwrap();
            let names: Vec<_> = agent.runtime().manifests().iter().map(|m| m.name.clone()).collect();
            return names.join(", ");
        }
        _ => {}
    }

    let (answer, catalog) = {
        let agent = shared.agent.lock().unwrap();
        (answer(&agent, request), catalog(&agent))
    };
    if answer != NO_SKILL {
        return answer;
    }
    match brain::ask(shared.bridge, request, catalog) {
        Ok(reply) => act_on(shared, session, reply),
        Err(e) => format!("I have no skill for that, and my brain is out of reach ({e})."),
    }
}

fn answer(agent: &Agent, request: &str) -> String {
    match agent.handle(request) {
        Response::Reply(text) => text,
        Response::Skill { output, .. } => describe(output),
        Response::Reported { prompt } => prompt,
        Response::Error(e) => e,
    }
}

fn describe(output: Output) -> String {
    match output {
        Output::Text { text } => text,
        Output::File { path, .. } => format!("Saved {path}"),
        Output::Data { value } => value.to_string(),
    }
}

/// Every installed skill as the brain should see it.
fn catalog(agent: &Agent) -> Value {
    Value::Array(
        agent
            .runtime()
            .manifests()
            .iter()
            .map(|m| {
                serde_json::json!({
                    "name": m.name,
                    "description": m.description,
                    "input_schema": m.input_schema,
                })
            })
            .collect(),
    )
}

/// Acts on the brain's answer: say something, run a skill, or offer a new one.
fn act_on(shared: &Shared, session: &mut Session, brain: BrainReply) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(say) = brain.say.filter(|s| !s.trim().is_empty()) {
        lines.push(say);
    }
    if let Some(call) = brain.call {
        let agent = shared.agent.lock().unwrap();
        lines.push(match agent.runtime().invoke(&call.skill, &call.args) {
            Ok(output) => describe(output),
            Err(e) => e.to_string(),
        });
    }
    if let Some(spec) = brain.propose {
        match spec.validate() {
            Ok(()) => {
                lines.push(format!(
                    "I wrote a skill: {} - {}. It can use: {}. Install it? Type yes or no.",
                    spec.name,
                    spec.description,
                    permissions_text(&spec)
                ));
                session.pending = Some((spec, brain.run_args));
            }
            Err(e) => lines.push(format!("The skill I wrote is no good: {e}")),
        }
    }
    if lines.is_empty() {
        "I have nothing to say about that.".into()
    } else {
        lines.join("\n")
    }
}

fn permissions_text(spec: &SkillSpec) -> String {
    if spec.permissions.is_empty() {
        return "nothing".into();
    }
    spec.permissions
        .iter()
        .filter_map(|p| serde_json::to_value(p).ok())
        .filter_map(|v| v.as_str().map(String::from))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Installs an approved skill, keeps it on disk, and runs it for the request that led to it.
fn approve(shared: &Shared, spec: &SkillSpec, run_args: Option<Value>) -> String {
    let mut agent = shared.agent.lock().unwrap();
    if let Err(e) = install(&mut agent, spec, &shared.library) {
        return format!("Could not install {}: {e}", spec.name);
    }
    let mut text = format!("Installed {}.", spec.name);
    if let Err(e) = shared.library.save(spec) {
        text.push_str(&format!(" (Not saved for next time: {e}.)"));
    }
    if let Some(args) = run_args {
        match agent.runtime().invoke(&spec.name, &args) {
            Ok(output) => text = format!("{text} {}", describe(output)),
            Err(e) => text = format!("{text} Running it failed: {e}"),
        }
    }
    text
}

/// Installs a written skill with exactly the permissions it declared.
fn install(agent: &mut Agent, spec: &SkillSpec, library: &Library) -> Result<(), String> {
    let skill = spec.build(library.data_dir())?;
    agent
        .runtime_mut()
        .install(Publisher::OnDeviceModel, Box::new(skill), &spec.permissions)
        .map_err(|e| e.to_string())
}
