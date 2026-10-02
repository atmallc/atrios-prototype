# pixie

An operating system for AIs to use and humans to direct. Linux kernel, Rust userspace. First device: Pixel 2. Development runs on device. User can talk to the ai and ai will code new skills as it needs. It will connect with online services using mcp and will have default skills like access to internet, and be able to audio chat with the user.

## Crates

| Crate | What it does |
| --- | --- |
| `pixie-skill` | Skill trait, manifest, tiers (critical, system, store, AI-authored) and permissions |
| `pixie-runtime` | Installs skills, enforces who may install which tier, checks permissions on every call |
| `pixie-agent` | Agent loop: guardrail review, model decision, skill call. `Model` and `Guardrail` are swappable traits |
| `pixie-skills-basic` | Camera (fake backend for now), show text, time, battery |
| `pixie-shell` | `pixie` binary: a text stand-in for the voice and text interface |
| `pixie-init` | The phone's first process: mounts, framebuffer UI, buttons, USB serial, agent |
| `pixie-image` | Builds a Pixel 2 `boot.img` from Google's stock kernel or a source-built one |

The model and guardrail are keyword stand-ins until Gemma 4 E2B is wired in.

## Install on a Pixel 2

See [docs/install-pixel-2.md](docs/install-pixel-2.md). In short:

```sh
rustup target add aarch64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl -p pixie-init
cargo run --release -p pixie-image -- repack --stock boot.img --out pixie-boot.img
fastboot boot pixie-boot.img
```

## Run on a computer

```sh
cargo test
cargo run -p pixie-shell
> take a photo
```
