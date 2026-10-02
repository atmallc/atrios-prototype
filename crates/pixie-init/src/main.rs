//! `pixie-init`: the first process on a Pixie phone.
//!
//! It mounts the kernel filesystems, starts the agent with the basic skills,
//! draws the Pixie screen on the framebuffer, and takes requests from the
//! hardware buttons, the USB link (serial or network) and the console.
//!
//! Run as a normal process (not PID 1) it skips the mounts and reads stdin,
//! which is handy for testing on a development machine.

mod bridge;
mod fb;
mod input;
mod system;
mod ui;
mod usb;

use pixie_agent::{Agent, KeywordGuardrail, KeywordModel, Response};
use pixie_runtime::{Publisher, Runtime};
use pixie_script::{BrainReply, SkillSpec};
use pixie_skill::Output;
use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use system::Reboot;
use ui::{Screen, Who};

enum Event {
    /// A typed request, with a channel for the text reply.
    Request(String, Sender<String>),
    Key(input::KeyPress),
    Tap(input::Tap),
    /// A line for the screen from a background task.
    Say(Who, String),
    /// The brain's answer to a request, with the channel the asker is waiting on.
    Brain(Sender<String>, Result<BrainReply, String>),
    /// Skills the owner approved earlier, fetched from the computer.
    Library(Result<Vec<SkillSpec>, String>),
    Tick,
}

const HELP: &str = "Ask for: take a photo, battery, time, show <text>, open <path>. \
Anything else goes to the brain, which can also write new skills. \
System: skills, sync, reboot (back to Android), bootloader, poweroff.";

fn main() {
    let pid1 = std::process::id() == 1;
    let mut screen = Screen::new();
    screen.push(Who::Pixie, "Pixie is starting.");

    if pid1 {
        for failure in system::mount_all() {
            screen.push(Who::Alert, format!("mount {failure}"));
        }
        system::ensure_backlight();
        // Before the input readers start, so the touchscreen is found.
        let mut step = |text: &str| {
            if let Ok(mut kmsg) = OpenOptions::new().write(true).open("/dev/kmsg") {
                let _ = writeln!(kmsg, "pixie: {text}");
            }
        };
        match system::load_touchscreen(&mut step) {
            Ok(()) => screen.push(Who::Pixie, "Touchscreen driver loaded."),
            Err(e) => screen.push(Who::Alert, format!("Touchscreen: {e}")),
        }
    }

    let typing = pixie_skills_basic::TypingMonitor::default();
    let mut agent = build_agent(
        if pid1 { "/data/photos" } else { "pixie-photos" },
        typing.clone(),
    );
    let mut display = fb::Framebuffer::open_first().ok().map(|fb| {
        let canvas = fb.canvas();
        (fb, canvas)
    });
    if display.is_none() {
        eprintln!("pixie: no framebuffer, running headless");
    }

    let (tx, rx) = mpsc::channel();

    let (keys, touch_screens) = {
        let (ktx, krx) = mpsc::channel();
        let (ttx, trx) = mpsc::channel();
        let found = input::spawn_readers(ktx, ttx);
        let key_tx = tx.clone();
        std::thread::spawn(move || {
            for key in krx {
                if key_tx.send(Event::Key(key)).is_err() {
                    return;
                }
            }
        });
        let tap_tx = tx.clone();
        std::thread::spawn(move || {
            for tap in trx {
                if tap_tx.send(Event::Tap(tap)).is_err() {
                    return;
                }
            }
        });
        found
    };

    // Draw first: if the USB step hangs the kernel, the last line shows where.
    draw(&screen, &mut display);
    let mut usb_step = |text: &str| {
        screen.push(Who::Pixie, text);
        draw(&screen, &mut display);
        // Also into the kernel log, which Android saves if the next step resets the phone.
        if let Ok(mut kmsg) = OpenOptions::new().write(true).open("/dev/kmsg") {
            let _ = writeln!(kmsg, "pixie: {text}");
        }
    };
    let link = pid1.then(|| usb::start_gadget(&mut usb_step));
    screen.status.link = match link {
        Some(Ok(usb::Link::Serial)) => {
            spawn_tty_reader("/dev/ttyGS0", tx.clone());
            "usb".into()
        }
        Some(Ok(usb::Link::Network)) => {
            spawn_tcp_server(tx.clone());
            screen.push(
                Who::Pixie,
                format!(
                    "USB network up. On the computer give the new interface {} (netmask 255.255.255.0), then: nc {} {}",
                    usb::HOST_IP,
                    usb::PHONE_IP.map(|b| b.to_string()).join("."),
                    usb::NET_PORT
                ),
            );
            "usb-net".into()
        }
        Some(Err(e)) => {
            screen.push(Who::Alert, format!("USB link unavailable: {e}"));
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
            "Ready. {} skills, {} input device(s), {} touchscreen(s). Tap the keyboard and type 'help', or press a button.",
            agent.runtime().manifests().len(),
            keys,
            touch_screens
        ),
    );
    spawn_library_sync(tx.clone());
    let mut session = Session {
        pending: None,
        data_dir: if pid1 { "/data/skill-data" } else { "pixie-skill-data" },
    };
    run(&mut agent, &mut screen, &mut display, rx, &typing, &tx, &mut session);

    // PID 1 must never exit, or the kernel panics.
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn build_agent(photo_dir: &str, typing: pixie_skills_basic::TypingMonitor) -> Agent {
    let mut runtime = Runtime::new();
    for skill in pixie_skills_basic::all_with(photo_dir, typing) {
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

/// State that lives between requests.
struct Session {
    /// A skill the brain wrote, waiting for the owner to say yes or no.
    pending: Option<(SkillSpec, Option<serde_json::Value>)>,
    /// Where written skills keep their scratch files.
    data_dir: &'static str,
}

fn run(
    agent: &mut Agent,
    screen: &mut Screen,
    display: &mut Option<(fb::Framebuffer, fb::Canvas)>,
    rx: Receiver<Event>,
    typing: &pixie_skills_basic::TypingMonitor,
    tx: &Sender<Event>,
    session: &mut Session,
) {
    refresh_status(agent, screen);
    draw(screen, display);
    for event in rx {
        match event {
            Event::Tick => refresh_status(agent, screen),
            Event::Say(who, text) => screen.push(who, text),
            Event::Brain(reply, result) => handle_brain(agent, screen, session, reply, result),
            Event::Library(Ok(skills)) => {
                let mut added = Vec::new();
                for spec in &skills {
                    if install_spec(agent, spec, session.data_dir).is_ok() {
                        added.push(spec.name.clone());
                    }
                }
                if !added.is_empty() {
                    screen.push(Who::Pixie, format!("Loaded skills: {}", added.join(", ")));
                }
            }
            Event::Library(Err(e)) => screen.push(Who::Alert, format!("No skill library: {e}")),
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
            Event::Tap(tap) => {
                let Some((w, h)) = display.as_ref().map(|(_, c)| (c.width, c.height)) else {
                    continue;
                };
                let (x, y) = ((tap.x * w as f32) as usize, (tap.y * h as f32) as usize);
                let line = screen.tap(x, y, w, h);
                typing.update(&screen.input);
                if let Some(line) = line {
                    // Typed on the phone: the reply only goes to the screen.
                    let (reply, _) = mpsc::channel();
                    handle_request(agent, screen, display, line, reply, tx, session);
                }
            }
            Event::Request(request, reply) => {
                handle_request(agent, screen, display, request, reply, tx, session);
            }
        }
        draw(screen, display);
    }
}

/// Runs one typed request (from USB or the on-screen keyboard) and shows the exchange.
fn handle_request(
    agent: &mut Agent,
    screen: &mut Screen,
    display: &mut Option<(fb::Framebuffer, fb::Canvas)>,
    request: String,
    reply: Sender<String>,
    tx: &Sender<Event>,
    session: &mut Session,
) {
    screen.push(Who::User, request.clone());
    let lower = request.trim().to_lowercase();

    // A written skill is waiting: the next yes or no is the owner's decision.
    if let Some((spec, run_args)) = session.pending.take() {
        let (who, text) = match lower.as_str() {
            "yes" | "y" | "install" => match install_spec(agent, &spec, session.data_dir) {
                Ok(()) => {
                    keep_on_computer(spec.clone(), tx.clone());
                    let mut text = format!("Installed {}.", spec.name);
                    if let Some(args) = run_args {
                        // Answer the request that led to the skill right away.
                        match agent.runtime().invoke(&spec.name, &args) {
                            Ok(Output::Text { text: answer }) => text = format!("{text} {answer}"),
                            Ok(_) => {}
                            Err(e) => text = format!("{text} Running it failed: {e}"),
                        }
                    }
                    (Who::Pixie, text)
                }
                Err(e) => (Who::Alert, format!("Could not install {}: {e}", spec.name)),
            },
            "no" | "n" | "cancel" => (Who::Pixie, format!("Discarded {}.", spec.name)),
            _ => {
                // Neither: keep waiting and say so.
                let name = spec.name.clone();
                session.pending = Some((spec, run_args));
                (Who::Pixie, format!("Install {name}? Type yes or no."))
            }
        };
        let _ = reply.send(text.clone());
        screen.push(who, text);
        return;
    }

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
        None if request == "sync" => {
            spawn_library_sync(tx.clone());
            (Who::Pixie, "Fetching skills from the computer...".to_string())
        }
        None if request == "skills" => {
            let names: Vec<_> = agent
                .runtime()
                .manifests()
                .iter()
                .map(|m| m.name.clone())
                .collect();
            (Who::Pixie, names.join(", "))
        }
        None => {
            let (who, text) = answer(agent, &request);
            if text == pixie_agent::NO_SKILL {
                // No built-in phrase matches: ask the brain without freezing the screen.
                screen.push(Who::Pixie, "Thinking...");
                ask_brain(agent, request, reply, tx.clone());
                return;
            }
            (who, text)
        }
    };
    let _ = reply.send(text.clone());
    screen.push(who, text);
}

/// Every installed skill as the brain should see it.
fn catalog(agent: &Agent) -> serde_json::Value {
    serde_json::Value::Array(
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

/// Asks the bridge on its own thread; the answer comes back as `Event::Brain`.
fn ask_brain(agent: &Agent, request: String, reply: Sender<String>, tx: Sender<Event>) {
    let catalog = catalog(agent);
    std::thread::spawn(move || {
        let result = bridge::ask(&request, catalog);
        let _ = tx.send(Event::Brain(reply, result));
    });
}

/// Acts on the brain's answer: say something, run a skill, or offer a new one.
fn handle_brain(
    agent: &mut Agent,
    screen: &mut Screen,
    session: &mut Session,
    reply: Sender<String>,
    result: Result<BrainReply, String>,
) {
    let mut lines: Vec<(Who, String)> = Vec::new();
    match result {
        Err(e) => lines.push((
            Who::Alert,
            format!("I have no skill for that, and my brain is out of reach ({e}). Start the bridge on the computer."),
        )),
        Ok(brain) => {
            let run_args = brain.run_args;
            if let Some(say) = brain.say.filter(|s| !s.trim().is_empty()) {
                lines.push((Who::Pixie, say));
            }
            if let Some(call) = brain.call {
                let output = agent.runtime().invoke(&call.skill, &call.args);
                lines.push(match output {
                    Ok(Output::Text { text }) => (Who::Pixie, text),
                    Ok(Output::File { path, .. }) => (Who::Pixie, format!("Saved {path}")),
                    Ok(Output::Data { value }) => (Who::Pixie, value.to_string()),
                    Err(e) => (Who::Alert, e.to_string()),
                });
            }
            if let Some(spec) = brain.propose {
                match spec.validate() {
                    Ok(()) => {
                        let permissions = if spec.permissions.is_empty() {
                            "nothing".to_string()
                        } else {
                            spec.permissions
                                .iter()
                                .filter_map(|p| serde_json::to_value(p).ok())
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect::<Vec<_>>()
                                .join(", ")
                        };
                        lines.push((
                            Who::Pixie,
                            format!(
                                "I wrote a skill: {} - {}. It can use: {permissions}. Install it? Type yes or no.",
                                spec.name, spec.description
                            ),
                        ));
                        session.pending = Some((spec, run_args));
                    }
                    Err(e) => lines.push((Who::Alert, format!("The skill I wrote is no good: {e}"))),
                }
            }
            if lines.is_empty() {
                lines.push((Who::Pixie, "I have nothing to say about that.".into()));
            }
        }
    }
    let spoken: Vec<&str> = lines.iter().map(|(_, t)| t.as_str()).collect();
    let _ = reply.send(spoken.join("\r\n"));
    for (who, text) in lines {
        screen.push(who, text);
    }
}

/// Installs a written skill with exactly the permissions it declared.
fn install_spec(agent: &mut Agent, spec: &SkillSpec, data_dir: &str) -> Result<(), String> {
    let skill = spec.build(data_dir)?;
    agent
        .runtime_mut()
        .install(Publisher::OnDeviceModel, Box::new(skill), &spec.permissions)
        .map_err(|e| e.to_string())
}

/// Saves an approved skill on the computer, since this phone forgets on restart.
fn keep_on_computer(spec: SkillSpec, tx: Sender<Event>) {
    std::thread::spawn(move || {
        if let Err(e) = bridge::approve(&spec) {
            let _ = tx.send(Event::Say(
                Who::Alert,
                format!("Installed, but not saved on the computer ({e}); it will be gone after a restart."),
            ));
        }
    });
}

/// Fetches the approved skills, retrying for a couple of minutes: the
/// computer's end of the USB link may not be ready when the phone boots.
fn spawn_library_sync(tx: Sender<Event>) {
    std::thread::spawn(move || {
        let mut last = Err("not tried".to_string());
        for _ in 0..24 {
            last = bridge::sync();
            if last.is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_secs(5));
        }
        let _ = tx.send(Event::Library(last));
    });
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

/// Accepts connections on the USB network link; each one is a session like the serial line.
fn spawn_tcp_server(tx: Sender<Event>) {
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(("0.0.0.0", usb::NET_PORT)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("pixie: tcp {}: {e}", usb::NET_PORT);
                return;
            }
        };
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let Ok(mut writer) = stream.try_clone() else { return };
                let _ = write!(writer, "pixie ready. type 'help'.\r\n> ");
                serve(BufReader::new(stream), &mut writer, &tx);
            });
        }
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
