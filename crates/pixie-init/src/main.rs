//! `pixie-init`: the first process on a Pixie phone.
//!
//! It mounts the kernel filesystems, starts the agent with the basic skills,
//! draws the Pixie screen on the framebuffer, and takes requests from the
//! hardware buttons, the USB serial gadget and the console.
//!
//! Run as a normal process (not PID 1) it skips the mounts and reads stdin,
//! which is handy for testing on a development machine.

mod fb;
mod input;
mod system;
mod ui;
mod usb;

use pixie_agent::{Agent, KeywordGuardrail, KeywordModel, Response};
use pixie_runtime::Runtime;
use pixie_skill::Output;
use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use system::Reboot;
use ui::{Screen, Who};

enum Event {
    /// A typed request, with a channel for the text reply.
    Request(String, Sender<String>),
    Key(input::KeyPress),
    Tick,
}

const HELP: &str = "Ask for: take a photo, battery, time, show <text>. \
System: skills, reboot (back to Android), bootloader, poweroff.";

fn main() {
    let pid1 = std::process::id() == 1;
    let mut screen = Screen::new();
    screen.push(Who::Pixie, "Pixie is starting.");

    if pid1 {
        for failure in system::mount_all() {
            screen.push(Who::Alert, format!("mount {failure}"));
        }
        system::ensure_backlight();
    }

    let agent = build_agent(if pid1 { "/data/photos" } else { "pixie-photos" });
    let mut display = fb::Framebuffer::open_first().ok().map(|fb| {
        let canvas = fb.canvas();
        (fb, canvas)
    });
    if display.is_none() {
        eprintln!("pixie: no framebuffer, running headless");
    }

    let (tx, rx) = mpsc::channel();

    let keys = {
        let (ktx, krx) = mpsc::channel();
        let n = input::spawn_readers(ktx);
        let tx = tx.clone();
        std::thread::spawn(move || {
            for key in krx {
                if tx.send(Event::Key(key)).is_err() {
                    return;
                }
            }
        });
        n
    };

    screen.status.link = match pid1.then(usb::start_serial_gadget) {
        Some(Ok(_)) => {
            spawn_tty_reader("/dev/ttyGS0", tx.clone());
            "usb".into()
        }
        Some(Err(e)) => {
            screen.push(Who::Alert, format!("USB serial unavailable: {e}"));
            String::new()
        }
        None => String::new(),
    };

    if pid1 {
        spawn_tty_reader("/dev/console", tx.clone());
    } else {
        spawn_stdin_reader(tx.clone());
    }

    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(20));
            if tx.send(Event::Tick).is_err() {
                return;
            }
        });
    }

    screen.push(
        Who::Pixie,
        format!(
            "Ready. {} skills, {} input device(s). Press a button, or connect USB and type 'help'.",
            agent.runtime().manifests().len(),
            keys
        ),
    );
    run(&agent, &mut screen, &mut display, rx);

    // PID 1 must never exit, or the kernel panics.
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn build_agent(photo_dir: &str) -> Agent {
    let mut runtime = Runtime::new();
    for skill in pixie_skills_basic::all(photo_dir) {
        if let Err(e) = runtime.install_trusted(skill) {
            eprintln!("pixie: {e}");
        }
    }
    Agent::new(
        runtime,
        Box::new(KeywordModel),
        Box::new(KeywordGuardrail::default()),
    )
}

fn run(
    agent: &Agent,
    screen: &mut Screen,
    display: &mut Option<(fb::Framebuffer, fb::Canvas)>,
    rx: Receiver<Event>,
) {
    refresh_status(agent, screen);
    draw(screen, display);
    for event in rx {
        match event {
            Event::Tick => refresh_status(agent, screen),
            Event::Key(input::KeyPress(code)) => {
                let request = match code {
                    input::KEY_VOLUMEUP => "take a photo",
                    input::KEY_VOLUMEDOWN => "battery",
                    input::KEY_POWER => "time",
                    _ => continue,
                };
                screen.push(Who::User, request);
                let (who, text) = answer(agent, request);
                screen.push(who, text);
            }
            Event::Request(request, reply) => {
                screen.push(Who::User, request.clone());
                let (who, text) = match system_command(&request) {
                    Some(r) => {
                        let note = match r {
                            Reboot::PowerOff => "Powering off...",
                            Reboot::Bootloader => "Restarting to the bootloader...",
                            Reboot::Restart => "Restarting...",
                        };
                        screen.push(Who::Pixie, note);
                        draw(screen, display);
                        let _ = reply.send(note.into());
                        std::thread::sleep(Duration::from_millis(300));
                        let e = system::reboot(r);
                        (Who::Alert, format!("reboot failed: {e}"))
                    }
                    None if request == "help" => (Who::Pixie, HELP.to_string()),
                    None if request == "skills" => {
                        let names: Vec<_> = agent
                            .runtime()
                            .manifests()
                            .iter()
                            .map(|m| m.name.clone())
                            .collect();
                        (Who::Pixie, names.join(", "))
                    }
                    None => answer(agent, &request),
                };
                let _ = reply.send(text.clone());
                screen.push(who, text);
            }
        }
        draw(screen, display);
    }
}

fn system_command(request: &str) -> Option<Reboot> {
    match request {
        "reboot" => Some(Reboot::Restart),
        "bootloader" => Some(Reboot::Bootloader),
        "poweroff" => Some(Reboot::PowerOff),
        _ => None,
    }
}

fn answer(agent: &Agent, request: &str) -> (Who, String) {
    match agent.handle(request) {
        Response::Reply(text) => (Who::Pixie, text),
        Response::Skill { output, .. } => (
            Who::Pixie,
            match output {
                Output::Text { text } => text,
                Output::File { path, .. } => format!("Saved {path}"),
                Output::Data { value } => value.to_string(),
            },
        ),
        Response::Reported { prompt } => (Who::Alert, prompt),
        Response::Error(e) => (Who::Alert, e),
    }
}

fn refresh_status(agent: &Agent, screen: &mut Screen) {
    let read = |name: &str| match agent.runtime().invoke(name, &serde_json::Value::Null) {
        Ok(Output::Text { text }) => text,
        _ => "--".into(),
    };
    screen.status.time = read("status.time");
    screen.status.battery = read("status.battery");
}

fn draw(screen: &Screen, display: &mut Option<(fb::Framebuffer, fb::Canvas)>) {
    if let Some((fb, canvas)) = display {
        screen.render(canvas);
        fb.present(canvas);
    }
}

/// Reads lines from a terminal and writes each reply back to it.
fn spawn_tty_reader(path: &'static str, tx: Sender<Event>) {
    std::thread::spawn(move || {
        // The USB tty appears a moment after the gadget binds.
        let mut tries = 0;
        let file = loop {
            match OpenOptions::new().read(true).write(true).open(path) {
                Ok(f) => break f,
                Err(_) if tries < 50 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(e) => {
                    eprintln!("pixie: {path}: {e}");
                    return;
                }
            }
        };
        let writer = file.try_clone();
        let Ok(mut writer) = writer else { return };
        let _ = write!(writer, "\r\npixie ready. type 'help'.\r\n> ");
        serve(BufReader::new(file), &mut writer, &tx);
    });
}

fn spawn_stdin_reader(tx: Sender<Event>) {
    std::thread::spawn(move || {
        let mut out = io::stdout();
        let _ = write!(out, "pixie ready. type 'help'.\n> ");
        let _ = out.flush();
        serve(io::stdin().lock(), &mut out, &tx);
    });
}

fn serve(reader: impl BufRead, writer: &mut impl Write, tx: &Sender<Event>) {
    for line in reader.lines() {
        let Ok(line) = line else { return };
        let request = line.trim().to_string();
        if !request.is_empty() {
            let (rtx, rrx) = mpsc::channel();
            if tx.send(Event::Request(request, rtx)).is_err() {
                return;
            }
            if let Ok(reply) = rrx.recv() {
                let _ = write!(writer, "{reply}\r\n");
            }
        }
        let _ = write!(writer, "> ");
        let _ = writer.flush();
    }
}
