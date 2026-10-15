# fizzle — Asheron's Call TUI client

A terminal client for the [ACE](https://asheron.us) (Asheron's Call) server
protocol, written in Rust. It logs in, enters the world, and supports movement,
melee combat, chat, a radar, vitals, and target appraisal — verified against a
live ACE server.

## Workspace

| Crate | Purpose |
|-------|---------|
| `acproto` | The protocol: packet framing, ISAAC stream crypto, the login/connect handshake, and encode/decode for the S2C/C2S game messages. |
| `acetui`  | The `ratatui`/`crossterm` terminal UI and session driver on top of `acproto`. |

`acproto` is self-contained (no I/O); `acetui` owns the UDP socket, the reader
thread, input, and rendering.

## Build & run

```sh
cargo build            # dev profile
cargo run -p acetui -- --account <user> --password <pass>
```

Options:

```
--host <HOST>       server host            (default 127.0.0.1)
--port <PORT>       UDP game port          (default 9000; connect port is +1)
--server <NAME>     name a saved profile entry to look up its host
--account <USER>    account name           (or the active profile account)
--password <PASS>   account password       (or the saved profile password)
--character <NAME>  auto-enter this character once the list arrives
```

Profiles (servers + accounts) are persisted under the user config dir, so a
second run can just `cargo run -p acetui` and reuse the active entry.

## Controls

| Key | Action |
|-----|--------|
| `w`/`↑` `x`/`↓` `a`/`←` `d`/`→` | move forward / back / turn left / right |
| `z` `c` | strafe left / right |
| `s` | stop |
| `Space` | jump |
| `q` | toggle run / walk |
| `` ` `` | toggle combat mode (melee) |
| `t` | attack current target |
| `Tab` | cycle target (sends an IdentifyObject to appraise it) |
| `Enter` / `/` | focus chat |
| `Esc` | menu (`l` log out, `q` quit) |

Chat commands (typed while chat is focused, sent with `Enter`):

- `/t <name> <text>` — tell a player
- `/ch <channel> <text>` — channel chat; `<channel>` is `g`/`general`,
  `a`/`allegiance`, `t`/`trade`, `lfg`, `rp`/`roleplay`, or `olthoi`
- anything else is a local `TALK` message to nearby objects

## How it works

The transport is UDP. The client opens the **game port** (default `9000`) and
the **connect port** (`9001`). The login handshake is `LoginRequest` →
`ConnectRequest`/`ConnectResponse`, after which the server seeds an ISAAC
stream (S2C) and the client seeds its own (C2S); all subsequent payloads are
encrypted with those streams. A reader thread receives, decrypts, reassembles
fragments, decodes messages, and feeds them to the event loop; movement is
emitted on a fixed 50 ms tick (`MoveToState` on signature change,
`AutonomousPosition` at ≤1 Hz, `Jump` on takeoff).

An `Echo`/`EchoResponse` keep-alive runs continuously so the server's
anti-speedhack `VerifyEcho` stays happy and the session isn't dropped for
inactivity.

## Tests

```sh
cargo test
```

`acproto` has unit tests covering the message layouts, including a byte-exact
`GameEventIdentifyObjectResponse` (appraise) regression test. The appraise
parser is bounds-checked so a truncated or malformed network message ends
parsing cleanly instead of panicking the client.

## Known limitations

- **Death / respawn sync.** The client tracks its own health locally from
  damage events. If the server kills the character, the client can stay out of
  sync (still showing alive) until the server logs the session out; death and
  respawn `GameEvent`s are not yet handled.
- **Character creation** is implemented and exercised, but long-running
  gameplay features (spells, trading, housing, etc.) are out of scope.
