# pixie

An operating system for AIs to use and humans to direct. Linux kernel, Rust userspace. First device: Pixel 2. Development runs on a host or emulator first.

## Crates

| Crate | What it does |
| --- | --- |
| `pixie-skill` | Skill trait, manifest, tiers (critical, system, store, AI-authored) and permissions |
| `pixie-runtime` | Installs skills, enforces who may install which tier, checks permissions on every call |
| `pixie-agent` | Agent loop: guardrail review, model decision, skill call. `Model` and `Guardrail` are swappable traits |
| `pixie-skills-basic` | Camera (fake backend for now), show text, time, battery |
| `pixie-shell` | `pixie` binary: a text stand-in for the voice and text interface |

The model and guardrail are keyword stand-ins until Gemma 4 E2B is wired in.

## Run

```sh
cargo test
cargo run -p pixie-shell
> take a photo
```
