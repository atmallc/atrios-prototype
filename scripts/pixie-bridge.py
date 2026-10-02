#!/usr/bin/env python3
"""Pixie's brain, running on the computer at the other end of the USB cable.

The phone connects to 10.55.0.2:2324 and sends one JSON line per request:

  {"op": "ask", "request": "...", "skills": [{name, description, input_schema}]}
      -> {"say": "...", "call": {...}, "propose": {...}}   (any mix)
  {"op": "sync"}                      -> {"skills": [...]}   skills approved earlier
  {"op": "approve", "skill": {...}}   -> {"ok": true}        keep this skill

Answers come from the `claude` command-line tool, run as you with every tool
turned off, so it can only reply in text. A skill it writes is checked with the
real Pixie compiler (`pixie-check`) before the phone sees it, and is saved
under ~/.pixie/skills only after the owner approves it on the phone.

    python3 scripts/pixie-bridge.py
"""
import json
import os
import tempfile
import re
import socketserver
import subprocess
import sys
from pathlib import Path

PORT = 2324
ROOT = Path(__file__).resolve().parent.parent
LIBRARY = Path.home() / ".pixie" / "skills"
CHECKER = ROOT / "target" / "debug" / "pixie-check"
ATTEMPTS = 3

SYSTEM = """You are the brain of Pixie, an assistant that lives on a Google Pixel 2 phone. The owner types to it on a tiny on-screen keyboard; the screen is about 30 characters wide.

You answer with ONE JSON object and nothing else (no code fence, no commentary). Fields, all optional:
  "say":     a short plain sentence for the screen (under 120 characters, no markdown).
  "call":    {"skill": "<installed skill name>", "args": {...}}  run an installed skill now.
  "propose": a NEW skill for the owner to approve:
             {"name": "user.<snake_case>", "description": "<one line>", "permissions": [...],
              "input_schema": {"type": "object", "properties": {...}}, "code": "<Rhai script>"}
  "run_args": with "propose": the arguments (an object, often {}) to run the new skill with the moment
             the owner approves it, so their original request gets answered without asking again.

How to decide:
- If an installed skill already does what the owner wants, use "call" (you may add a short "say").
- NEVER answer "I don't have a skill for that" or refuse because a skill is missing: missing skills are YOUR job. If a script could do any useful part of the request with the functions below, write it: "propose" it, add "run_args", and "say" briefly what it does. A proposed skill is not installed yet, so do not "call" it in the same answer.
- Prefer a general, reusable skill with arguments (for example "user.read_proc" taking a path) over a one-off, but keep it simple.
- Only when the request truly cannot be done with the functions below (it needs the network, camera, microphone, audio, calls or messages, which written skills cannot use yet), "say" exactly which capability is missing, in one short sentence, and offer the closest thing a script CAN do.
- For chat or questions, just "say" a short answer.

Skills are Rhai scripts (https://rhai.rs: Rust/JavaScript-like). The variable `args` is a map of the call's arguments; the value of the last expression is the answer shown to the owner (make it a short string). No `return`, `import` or `eval`. Examples:
  let n = args.n; "double is " + (n * 2)
  let up = read_file("/proc/uptime"); "Up " + up.split(' ')[0] + " seconds"
Syntax notes: `let x = 1;` `if c { } else { }` `for i in 0..5 { }` `fn f(a) { a + 1 }` arrays `[1, 2]` maps `#{a: 1}` strings join with `+`, `x.to_string()`, `s.len()`, `s.trim()`, `s.split(",")`, `s.contains("a")`. Scripts that run too long are stopped (200000 operations).

Functions exist only for permissions you list in "permissions" (allowed: clock, power_status, storage, display):
  clock:        time_utc() -> "HH:MM UTC"; now_secs() -> seconds since 1970
  power_status: battery() -> "87%"; battery_percent() -> 87
  storage:      read_file(path) and list_dir(path) (read-only, first 1000 characters); save(text) and load() keep a small note private to this skill (cleared on restart)
  display:      no functions; the answer is shown on screen
Ask for the fewest permissions that work. The owner sees them and must approve. Pick a new, specific name starting with "user." that is not already installed.

To speak aloud use the installed "audio.speak" skill (args {"text": "..."}) with "call"; only do so when the owner asks you to say or read something out loud.

Use the installed "web.search" skill (args {"query": "..."}) with "call" for anything that needs current or outside information: news, weather, prices, sports, recent events, facts you are unsure of. Do not guess those.

Installed skills on the phone right now:
{catalog}
"""


def clean_json(text: str):
    text = text.strip()
    text = re.sub(r"^```(?:json)?\s*|\s*```$", "", text)
    start, end = text.find("{"), text.rfind("}")
    if start < 0 or end < start:
        return None
    try:
        return json.loads(text[start : end + 1])
    except ValueError:
        return None


def run_claude(system: str, prompt: str) -> str:
    cmd = [
        "claude", "-p", prompt,
        "--system-prompt", system,
        "--tools", "",
        "--max-turns", "3",
        "--no-session-persistence",
        "--output-format", "text",
    ]
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=200)
    return out.stdout.strip() or "(no answer: " + " ".join(out.stderr.split())[:120] + ")"


def check_skill(spec: dict, installed: set) -> str:
    """Returns "" if Pixie would accept the skill, else the reason."""
    if not str(spec.get("name", "")).startswith("user."):
        return 'the name must start with "user."'
    if spec["name"] in installed or (LIBRARY / f"{spec['name']}.json").exists():
        return f"the name {spec['name']} is already taken; choose another"
    if not CHECKER.exists():
        return ""  # no compiler to check with; the phone will check again
    out = subprocess.run([str(CHECKER)], input=json.dumps(spec), capture_output=True, text=True)
    return "" if out.returncode == 0 else out.stdout.strip()


def ask(request: str, catalog: list) -> dict:
    installed = {s.get("name") for s in catalog}
    system = SYSTEM.replace(
        "{catalog}",
        "\n".join(f'- {s["name"]}: {s["description"]}  args: {json.dumps(s.get("input_schema", {}).get("properties", {}))}' for s in catalog) or "(none)",
    )
    prompt = request
    for attempt in range(ATTEMPTS):
        text = run_claude(system, prompt)
        reply = clean_json(text)
        if reply is None:
            return {"say": " ".join(text.split())[:300]}
        spec = reply.get("propose")
        if spec:
            problem = check_skill(spec, installed) if isinstance(spec, dict) else "propose must be an object"
            if problem:
                print(f"  attempt {attempt + 1}: skill refused: {problem}", flush=True)
                prompt = (
                    f"{request}\n\nYour last answer proposed a skill that Pixie refused: {problem}\n"
                    "Fix it and answer again with the one JSON object."
                )
                continue
        return {k: reply[k] for k in ("say", "call", "propose", "run_args") if k in reply and reply[k] is not None}
    return {"say": "I could not write a working skill for that."}


SEARCH_PROMPT = (
    "Search the web for: {query}\n"
    "Answer for a phone screen: at most three short plain sentences with the key facts, no markdown. "
    "Then up to two source URLs, each on its own line."
)


def web_search(query: str) -> str:
    """Searches with Claude's WebSearch tool; the only tool this call may use."""
    cmd = [
        "claude", "-p", SEARCH_PROMPT.format(query=query[:300]),
        "--tools", "WebSearch", "--allowedTools", "WebSearch",
        "--max-turns", "6",
        "--no-session-persistence",
        "--output-format", "text",
    ]
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=150)
    text = out.stdout.strip()
    if not text:
        text = "(search failed: " + " ".join(out.stderr.split())[:120] + ")"
    return text[:900]


def tts(text: str) -> bytes:
    """Speaks `text` with the Mac's own voice and returns a 22 kHz mono WAV."""
    with tempfile.TemporaryDirectory() as tmp:
        aiff, wav = os.path.join(tmp, "s.aiff"), os.path.join(tmp, "s.wav")
        subprocess.run(["say", "-o", aiff, "--", text[:500]], check=True, timeout=30)
        subprocess.run(["afconvert", "-f", "WAVE", "-d", "LEI16@22050", aiff, wav], check=True, timeout=30)
        with open(wav, "rb") as f:
            return f.read()


def load_library() -> list:
    LIBRARY.mkdir(parents=True, exist_ok=True)
    skills = []
    for path in sorted(LIBRARY.glob("*.json")):
        try:
            skills.append(json.loads(path.read_text()))
        except ValueError:
            pass
    return skills


def approve(spec: dict) -> None:
    name = str(spec.get("name", ""))
    if not re.fullmatch(r"user\.[a-z0-9_.]{1,34}", name):
        raise ValueError("bad skill name")
    LIBRARY.mkdir(parents=True, exist_ok=True)
    (LIBRARY / f"{name}.json").write_text(json.dumps(spec, indent=2))


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        line = self.rfile.readline().decode("utf-8", "replace")
        try:
            msg = json.loads(line)
            op = msg["op"]
        except (ValueError, KeyError):
            return
        try:
            if op == "ask":
                print(f"> {msg['request']}", flush=True)
                reply = ask(str(msg["request"]), list(msg.get("skills", [])))
                print(f"< {json.dumps(reply)[:300]}", flush=True)
            elif op == "search":
                query = str(msg["query"])
                print(f"? search: {query}", flush=True)
                reply = {"text": web_search(query)}
                print(f"< {reply['text'][:200]!r}", flush=True)
            elif op == "tts":
                print(f"~ say: {str(msg['text'])[:80]}", flush=True)
                reply = {"wav_hex": tts(str(msg["text"])).hex()}
            elif op == "sync":
                reply = {"skills": load_library()}
                print(f"= sync: {len(reply['skills'])} skills", flush=True)
            elif op == "approve":
                approve(msg["skill"])
                print(f"+ approved {msg['skill'].get('name')}", flush=True)
                reply = {"ok": True}
            else:
                return
        except Exception as e:  # tell the phone instead of leaving it waiting
            reply = {"say": f"(bridge error: {e})"}
        self.wfile.write((json.dumps(reply) + "\n").encode("utf-8"))


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


if __name__ == "__main__":
    # Only the phone's USB link should reach this, so bind to its address.
    host = sys.argv[1] if len(sys.argv) > 1 else "10.55.0.2"
    with Server((host, PORT), Handler) as server:
        print(f"Pixie bridge listening on {host}:{PORT}; skills in {LIBRARY}", flush=True)
        server.serve_forever()
