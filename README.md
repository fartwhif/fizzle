# fizzle — Asheron's Call TUI client

A terminal client for the [ACE](https://asheron.us) (Asheron's Call) server
protocol, written in Rust. It logs in, creates/selects characters, enters the
world, and supports movement (with terrain and door collision); melee,
missile, and magic combat (with attack height/power and spell slots/tabs);
stances and soul emotes; chat, tells, channels, and emote chat; targeting,
appraise, and live target health; fellowship, allegiance, and friends;
books (read/write pages); a radar; vitals (including XP and Vitae); death and
respawn; a 3D world view (with the local character rendered as an actor); an
inventory paper-doll with side packs; the Dereth continent map; vendor/trade/
loot; and logout — verified against a live ACE server.

## Workspace

| Crate | Purpose |
|-------|---------|
| `acproto` | The protocol: packet framing, ISAAC stream crypto, the login/connect handshake, and encode/decode for the S2C/C2S game messages. No I/O. |
| `acgeo`  | The game geometry: the retail DAT reader, the land height/terrain and solid (door/wall) collision, the Dereth landmass, and the software 3D world renderer. No I/O. |
| `fizzle`  | The `ratatui`/`crossterm` terminal UI and session driver on top of `acproto` + `acgeo`. |

`acproto` and `acgeo` are self-contained (no network I/O); `fizzle` owns the
UDP socket, the reader thread, input, and rendering.

## Build & run

```sh
cargo build            # dev profile
cargo run -p fizzle -- --account <user> --password <pass>
```

Options:

```
--host <HOST>       server host            (default 127.0.0.1)
--port <PORT>       UDP game port          (default 9000; connect port is +1)
--server <NAME>     name a saved profile entry to look up its host
--account <USER>    account name           (or the active profile account)
--password <PASS>   account password       (or the saved profile password)
--character <NAME>  auto-enter this character once the list arrives
--ui <MODE>         UI: `tui` (default) or `plain` (plain stdout, no TUI)
```

Profiles (servers + accounts) are persisted under the user config dir, so a
second run can just `cargo run -p fizzle` and reuse the active entry.

## Icons (Nerd Fonts)

Item icons (pack grid, paper doll, vendor/trade/loot, ground, drag marker)
and the right-column one-row widget selector use Nerd Font *Material Design
Icon* glyphs (`nf-md-*`, code points 0xF0001-0xF1AF0). Run fizzle in a
terminal whose font is a [Nerd Font](https://www.nerdfonts.com/) to see them
as icons. Every glyph occupies exactly one cell, so the pack grid, paper
doll, and lists stay aligned in any terminal — in a non-Nerd-Font terminal
they simply render as empty/tofu boxes.

## Controls

The in-game `F1` help screen is the definitive key map. The essentials:

**Movement** (client-side simulation; the server corrects if it disagrees):

| Key | Action |
|-----|--------|
| `w`/`↑` `x`/`↓` | move forward / backward |
| `a`/`←` `d`/`→` | turn left / right |
| `z` `c` | strafe left / right |
| `s` | stop |
| `Space` | jump |
| `Q` | run lock on / off (default on) |
| `Shift` (with any move key) | walk (else the run lock decides run / walk) |

Hold several move keys to combine (W+D arcs, W+C is diagonal). A tap is a
small nudge; a held key pauses until the terminal's first auto-repeat, then
continues.

**Targeting and objects** (the retail key map):

| Key | Action |
|-----|--------|
| `+` / `-` | next / prev compass item (creature) |
| `'` | nearest creature |
| `L` / `;` | prev / next creature |
| `[` / `]` | prev / next object (door, ground item) |
| `\` | nearest object |
| `,` / `.` | prev / next person |
| `/` | nearest person |
| `F` | pick up the target (ground container; pack item → main pack) |
| `R` | use the target (container → pick up; NPC → talk dialog) |
| `E` | examine the target (works from any widget) |
| `T` | attack the target in the current combat mode (melee or missile) |
| `` ` `` | toggle combat / peace mode (sent to the server) |

**Combat hotkeys** — the `` ` `` toggle cycles between Peace and your
character's default combat mode (melee, missile, or magic by equipped
weapon). The numeric / edit keys are scoped to the active mode:

| Key | Melee / Missile | Magic |
|-----|-----------------|-------|
| `1-9,0` | item quickslot (not used) | select spell slots 1-10 |
| `Ins` | power − | previous spell slot |
| `PgUp` | power + | previous spell tab |
| `Del` | attack height: low | next spell slot |
| `End` | attack height: medium | cast the selected spell |
| `PgDn` | attack height: high | next spell tab |

Attack power is quantized to 7 steps (0-100%, default 50%); height is
low/medium/high (default medium).

**Stances:** `Y` ready, `G` sit, `H` crouch, `B` sleep (sent to the server
as a movement substate).

**Emotes** (soul emotes): `K` point, `I` laugh, `J` wave, `O` cheer, `U` cry.

**Social actions** — the right-column panel selects which keys are active
(the actions act on the current target, which must be a player):

| Panel | Key | Action |
|-------|-----|--------|
| Allegiance | `v` / `m` | swear to / break allegiance with the target |
| Fellowship | `v` / `m` / `n` / `p` | recruit / dismiss / make new leader / toggle openness (target) |
| Friends | `m` | remove the target as a friend |
| Spellbook | `p` / `m` / `v` | add to / remove from / forget the selected spell |

Fellowship create/quit and friend add take a name via chat: `/fellowship
<name>`, `/fellowship quit`, `/friend <name>`.

**Books** (modal) — `R` on a book opens it. While open: `↑`/`j` and
`↓`/`k` change page, `e` edit (then type; `Enter` saves, `Esc` cancels),
`a` add a page, `d` delete the page, `r` refetch the page, `Esc` close.

**UI:** `Enter`/`Tab` open chat (Enter sends, Tab or Esc closes it); `F1`
help; `Esc` menu; `Shift+Esc` log out of the world.

Chat commands (typed while chat is focused, sent with `Enter`): `/t <name>
<text>` tell a player; `:text` (or `;text`) emote chat; `/ch <channel>
<text>` a channel; every other `/` or `@` command (e.g. `/lifestone`,
`@acehelp`) is passed to the server as-is.

## Mouse

Click to act; the pointer is drawn as a marker. The inventory, vendor, trade,
and loot widgets are driven entirely by the mouse:

- **left-click** — target the actor/item under the cursor (in combat stance,
  a click on a creature attacks it).
- **left double-click** — use the target under the cursor (a ground item is
  picked up, a chest/corpse opens to loot, a player opens a trade).
- **scroll wheel** — scroll the active list; over the pack grid scroll its
  items; over the vendor box scroll its list; over the 3D view zoom the
  camera in (scroll up) / out (scroll down).
- **right-click** — target + examine the item/object under the cursor.
- **glyph row / Spells / Keys bar** — open that multi widget / select the slot.
- **drag & drop** — onto the pack icon column, trade, vendor, paper doll
  (equip), pack, player, or the ground.
- **inventory icon column** — the rightmost column of the inventory widget
  holds one icon per side pack you own; clicking an icon opens that pack as a
  loot view (its contents are already merged into the main pack list).
- **menu / list rows** — click moves the selection, double-click activates.

## Menu, themes, world view

`Esc` opens a menu (`j`/`k` move, `Enter` open, `Esc` back) with seven items:
Themes, Widgets, World view, Actor glow, Help, Log out, and Quit.

- **Themes** — four: *Custom* (default, your terminal's colors + ASCII
  borders), *Utf8* (unicode rounded borders), *Mono* (strip color, keep
  modifiers), *Green* (unicode borders, all green). Applied live.
- **Widgets** — pick the right-column multi widget (14: Allegiance,
  Fellowship, Friends, Squelch, Spellbook, Attributes, Skills, Titles,
  Contracts, Map, House, Inventory, You, Examine).
- **World view** — the world pane's render mode: *tactical* (north-up
  top-down map) or *3d* (realtime 3D, needs the retail DATs). In 3D, the
  scroll wheel dollies the camera (closer/lower to farther/higher); the
  distance is clamped to a readable range and never passes through walls.
- **Actor glow** — a halo around characters in the 3D view (on/off).

Theme, world view, and actor-glow choices apply live and are written back to
the profile on exit, so the next launch picks them up (alongside the saved
servers and accounts).

## DAT files (3D, collision, Dereth map)

The 3D view, terrain/door collision, and the Dereth continent map read the
retail DAT files. Point `fizzle` at them with the `AC_DAT_DIR` environment
variable (or the profile's `dat_dir`, or a standard install path). Without the
DATs the client still runs: the world pane falls back to the tactical radar
only, movement uses a flat plane, and the Dereth map is unavailable.

## How it works

The transport is UDP. The client opens the **game port** (default `9000`) and
the **connect port** (`9001`). The login handshake is `LoginRequest` →
`ConnectRequest`/`ConnectResponse`, after which the server seeds an ISAAC
stream (S2C) and the client seeds its own (C2S); all subsequent payloads are
encrypted with those streams. A reader thread receives, decrypts, reassembles
fragments, decodes messages, and feeds them to the event loop; movement is
emitted on a fixed 50 ms tick (`MoveToState` on signature change,
`AutonomousPosition` at ≤1 Hz, `Jump` on takeoff).

The session keepalive is a PAK (`AckSequence`, flag `0x4000`, carrying the
highest S2C packet sequence) sent at most once every 2.0 s — the retail
`EnqueuePak` cadence. Any C2S packet resets the server's timeout (15 s while
authenticating, 60 s after), which is what keeps an idle character-select
session alive. The PAK reuses the current C2S packet sequence without
consuming a number: the server advances its `lastReceivedPacketSequence`
only for non-PAK packets, and a PAK that did would push the next game
packet out of order (PROTOCOL.md, Step 4).

The rest of the session lifecycle mirrors the retail client's timers:

| Timer | Behavior |
|---|---|
| Login | retransmit `LoginRequest` every 2.0 s, up to 20 tries |
| Cookie | retransmit `ConnectResponse` every 333 ms while the handshake is outstanding |
| PAK | keepalive every 2.0 s |
| Silence | checked every 4 s: no S2C data for 40 s = dead server |
| Enter world | no world within 110 s = dead server |
| Logout | no `CharacterLogOff` response within 25 s = dead server |

When a session dies — server drop, "account in use" (`CharacterError 1`), or
timeout — fizzle stops and shows a **Disconnected** screen with the reason;
`Enter` starts a fresh login, `Esc` quits. Nothing reconnects on its own,
matching retail, where every reconnect is a user-initiated action.

On a clean exit fizzle sends the retail `LogOffServer` Disconnect broadcast
(a bare 20-byte header, flag `0x8000`), so the server drops the session
immediately (`Reason: PacketHeader Disconnect`) instead of waiting out its
timeout. Note that the server holds an account for ~2 s after *any* session
end (staged termination): a restart inside that window is rejected with
"account in use" exactly as retail gets — retry a moment later.

## Death & respawn

Death and respawn are server-authoritative, not tracked from local damage.
The `PlayerKilled` (0x019E) broadcast and the `PrivateUpdateAttribute2ndLevel`
(0x02E9) vitals drive the client: `Health == 0` sets the dead state (red
"YOU ARE DEAD" overlay, motion packets stop, input gated as in the retail
`PlayerIsDead`), and the revive vitals — health restored to 75% of max on the
death teleport, then to full at the lifestone/sanctuary — clear it. Like the
retail client, the client swallows its own `PlayerKilled` (no chat line when
the local player is victim or killer) and scrolls other players' deaths to
chat. The death teleport itself syncs through the existing
`PlayerTeleport`/`UpdatePosition` flow. Both opcodes are verified
byte-for-byte against the live server (capture regression tests in `acproto`).

## Tests

```sh
cargo test
```

`acproto` has unit tests covering the message layouts, including a byte-exact
`GameEventIdentifyObjectResponse` (appraise) regression test. The appraise
parser is bounds-checked so a truncated or malformed network message ends
parsing cleanly instead of panicking the client.

`acgeo` has geometry tests plus an integration test (`render3d_check`) that
renders a real landblock against the retail DATs (skipped when the DATs are
absent) and asserts the 3D output is shaded, not a uniform fill.

## Known gaps

Things the retail client does that fizzle intentionally does not, because a
terminal TUI has no place for them:

- **Rendering beyond the 3D world pane** — no D3D device, no full-screen UI
  windows, no UI window layout persistence, no screenshot capture.
- **Audio** — no sound.
- **Input devices** — no joystick/DirectInput support (keyboard + mouse only).
- **Internationalization** — no DBCS/IME (single-byte ASCII input).
- **Plugin host** — no `theapdll` plugin loading.
- **Retail infrastructure** — no Keystone billing, no self-patcher.
- **Dedicated retail UIs** — ProfilerUI, the character appearance editor, the
  house property GUI, chess, the consent list, and PK arenas are not
  implemented.

Gaps driven by the protocol or the reference client's scope:

- In a trade, the other player's item names are not shown (the server does
  not send them).
- Some parsed item metadata (per-item coin value, capacity, sell price, icon
  id) is decoded but not displayed — matching the reference client's scope.

Documented deviations from retail (behavioral choices, not omissions):

- **Cast targeting without a SpellTable** — with no retail `SpellTable.dat`
  on hand, a cast is aimed at the current combat target if one is selected,
  otherwise sent untargeted; the server validates either way.
- **Game ping cadence** — a game ping is sent every 10 s (retail pings on a
  slightly different timer).
- **Stances** — a stance change is a `MoveToState` with a forward command at
  0.0 speed (the same motion mechanism retail uses).
- **Soul emotes** — sent via the soul-emote motion body with the sequence
  masked to the low 15 bits (`0x7FFF`), as the retail motion path does.
- **Attack power** — defaults to 0.5 (medium) and quantizes to the retail 7
  power levels.
- **Book UI** — the book reader is a modal overlay over the world pane rather
  than a separate window.
- **Social actions** — member-directed actions (fellowship, friends) act on
  the current target and are gated by the active social panel, since a TUI
  has no per-member list cursor.
- **Idle logout** — the client logs out after 1200 s of no input.

## Resources

External Asheron's Call references:

- [aclogview](https://github.com/ACEmulator/aclogview) — a viewer for ACE server log files.
- [ACCPP source search](https://search.accpp.net/#b=ACCPP) — full-text search over the Asheron's Call C++ source.
