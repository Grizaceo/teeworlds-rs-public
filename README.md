# teeworlds-rs

A Teeworlds-style multiplayer platformer shooter in Rust, playable in the
browser. One binary serves the WASM client and runs an authoritative 50 Hz
server over WebSocket (own binary protocol, not DDNet-compatible).

By Cristóbal Muñoz, built with Devin, Hermes and Claude Code. Separate from
Vibe Arena's judged code; it connects to Vibe Arena as an external game.

## Run

```bash
# Order matters: the server embeds the wasm
cargo build --release -p client --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/teeworlds-rs.wasm teeworlds_rs.wasm
cargo build --release -p server
BOTS=3 ./target/release/teeworlds-rs-server   # http://localhost:8303/#yourname
```

Modes (`MODE=`): `dm`, `eat`, `race`, `stomach`, `evo`, `mancha`, `koth`, `ctf`.
Controls: A/D move, Space jump (double), right mouse hook, left mouse fire,
1–5 weapons, Shift dash, Q parry.

## Stream integration (Vibe Arena)

- Protocol is versioned (`PROTO_VERSION`). Vibe Arena's current chat bridge
  keeps working unchanged as a v0 client; `tools/vibe_compat_check.mjs`
  verifies it with Vibe Arena's own codec.
- `STREAMER_KEY` (env only): the streamer joins with `/?streamer` and types
  the key; it never goes in a URL.
- `PLATFORM_KEY` (env only): lets the platform send chat effects
  (bomb, armor) on behalf of viewers who are not playing.
- `MAX_PLAYERS` caps the room; viewers join with `/?name=<login>`.

Full design notes and decisions (Spanish): [AGENTS.md](AGENTS.md).

## Tests

```bash
cargo test --release --workspace --exclude client
```
