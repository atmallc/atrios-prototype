//! `pixie`: a text shell for the agent. Type a request, get a response.
//! This stands in for Pixie's voice and text interface until the UI exists.

use pixie_agent::{Agent, KeywordGuardrail, KeywordModel, Response};
use pixie_runtime::Runtime;
use pixie_skill::Output;
use std::io::{self, BufRead, Write};

fn main() -> io::Result<()> {
    let photo_dir = std::env::var("PIXIE_PHOTO_DIR").unwrap_or_else(|_| "pixie-photos".into());
    let mut runtime = Runtime::new();
    for skill in pixie_skills_basic::all(photo_dir) {
        runtime
            .install_trusted(skill)
            .expect("basic skills install");
    }
    let agent = Agent::new(
        runtime,
        Box::new(KeywordModel),
        Box::new(KeywordGuardrail::default()),
    );

    let names: Vec<_> = agent
        .runtime()
        .manifests()
        .iter()
        .map(|m| m.name.clone())
        .collect();
    println!("pixie ready. skills: {}", names.join(", "));

    let stdin = io::stdin();
    let mut out = io::stdout();
    loop {
        write!(out, "> ")?;
        out.flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let request = line.trim();
        if request.is_empty() {
            continue;
        }
        if request == "exit" {
            break;
        }
        match agent.handle(request) {
            Response::Reply(text) => println!("{text}"),
            Response::Skill { skill, output } => match output {
                Output::Text { text } => println!("[{skill}] {text}"),
                Output::File { path, .. } => println!("[{skill}] saved {path}"),
                Output::Data { value } => println!("[{skill}] {value}"),
            },
            Response::Reported { prompt } => println!("{prompt}"),
            Response::Error(e) => println!("error: {e}"),
        }
    }
    Ok(())
}
