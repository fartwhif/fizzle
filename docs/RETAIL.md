# Decompiled retail `acclient.exe` — research notes

Everything in this repo that is "retail-derived" (movement physics, wire
formats, the TUI key map) traces back to the decompiled retail Asheron's Call
client binary and the retail DAT assets. This file records
what was examined, what was extracted, and how — so the numbers in
`src/movement.ts` / `src/messages.ts` are reproducible rather than folklore.
The **complete retail-client specification** — architecture, networking, world
streaming, entities/animation, physics, player data, combat, UI, input,
rendering, and security — is `docs/ACCLIENT.md`; this file remains the
authoritative source for the DAT/terrain/movement facts it cites.

The decompiled sources are **user-provided and user-authorized** for this
project. Canonical (committed) copies live in `docs/artifacts/`; the
decompiler working copies lived in `/tmp/retail` (disposable).

## 1. Binary / version inventory

Multiple copies of the retail client binary were examined (temporary copies in
`/tmp`, now gone except the decompiles):

| label  | vintage | notes |
|--------|---------|-------|
| A      | 2018    | retail `acclient.exe` |
| B      | 2019    | retail `acclient.exe` — **chosen** |
| C ≡ D  | 2024    | retail `acclient.exe` (two copies, byte-identical) |
| H1/H2  | —       | the two header/typedef sets that accompany the decompile |

All of A/B/C/D are the **same retail client** (Asheron's Call shipped no
meaningful client update; the differences are re-release packaging). The
decompiles differ only in trivial symbol churn. **B + H1 were chosen** as the
reference; the 2024 decompile (`acclient_c_2024.c` + `acclient_h2.h`) was
cross-checked and agrees on everything that matters (motion table, message
structs, opcodes).

Four files named around "Portaled" `acclient` variants were inspected and are
**red herrings** — near-empty stubs, not the real client. Ignore them.

### Canonical decompiled copies (committed)

| file | sha256 | size (B) |
|------|--------|----------|
| `docs/artifacts/acclient.c` (B) | `d120a8c2e0287bf106d9650946abe203b176b3d5e3973c51efafe594b854bbed` | 31,655,869 |
| `docs/artifacts/acclient.h` (H1) | `c704470f9c69ef669385fbc98f16797774bb9e048b0bc1f08f8daed3b30cc236` | 1,705,112 |

Cross-check (uncommitted, `/tmp/retail`):

| file | sha256 | size (B) |
|------|--------|----------|
| `acclient_c_2024.c` (C/D) | `ccc2234a73b9d8edc4566fa5502bfb8eff77476bb2a240a4b4f4fe0d31dc3581` | 31,660,260 |
| `acclient_h2.h` (H2) | `776b1f007f0b267166a9729a20b551ffe136d7dac3a1f5e358aad5b6bd3daf81` | 1,644,123 |

## 2. Retail DAT asset format (record walking)

`client_portal.dat` (885 MB; all three local copies byte-identical,
sha256 `dc6e500ba22e6b186db7171e3f3345238b6444c85d798adc85e550973b8d12e4`)
holds the motion tables. Format (reverse-engineered by scanning; consistent
with the DAT loader in the server C# `UnpackableExtensions`):

* **File header at offset `0x140`**: `u32 FileType = 21570`, then
  `u32 BlockSize = 1024` (sector size).
* **Sector** = `u32 next_offset` (0 = last sector of the record) + 1020 data
  bytes. A record is a **chain of sectors** walked via `next_offset`.
* **Record data** begins with its own `u32 Id` (the table id), then the
  payload. Multi-sector chains concatenate the 1020-byte payloads.

Example: the player motion table record sits at sector offset `0x36d1b800`,
spans 44 sectors, contains 366 motion cycles; its header carries
`defaultStyle 0x8000003d` (MotionStance.NonCombat) and 14 stance descriptors
(SDs).

### 2.1 DAT directory (B-tree)

`src/dats.ts` (`DatDatabase`) reads the whole container (any `client_*.dat`)
through its B-tree directory — the server C# `ACE.DatLoader`
(`DatDirectory`/`DatDirectoryHeader`) mirrors the retail `DiskController`:

* **Header @0x140** (u32s, LE): `FileType` (`0x5442`), `BlockSize` (1024),
  `FileSize`, `DataSet` (1 portal / 2 cell / 3 language / 4 highres), …,
  `BTree` (the root directory node's offset, the field at +0x20).
* **Directory node** = one sector chain of a fixed-size object:
  62×u32 `Branches` + u32 `EntryCount` + `EntryCount`×`DatFile`, where
  `DatFile` = 6×u32: `BitFlags`, `ObjectId`, `FileOffset`, `FileSize`,
  `Date`, `Iteration`. Branch `i` (0..EntryCount) points at a child node
  (0 = none); a node is a leaf when `Branches[0]` is 0 (the other slots may
  hold stale data).
* A record's bytes are the sector chain from `FileOffset`, beginning with
  its own `u32 Id`.

The walk reads ~805k entries in the cell dat in ~1 s and keeps them as
sorted typed arrays (binary-search lookups). Record ids are **arithmetic**,
never `id << 16`: landblock `LB`'s terrain is `LB*0x10000 + 0xffff` and its
placed objects are `LB*0x10000 + 0xfffe`.

### 2.2 Landblock geometry records (the "3d" world view)

`src/acegeom.ts` parses the records `src/world3d.ts` renders. Coordinate
system (server C# `ACE.Entity/Position.cs` + `LandblockId.cs`): **+X east,
+Y north, +Z up**; a **landblock is 192×192 u**, split 8×8 into 24×24 u
tiles, described by a **9×9 vertex grid**.

* **Landblock id = the wire cell's high 16 bits.** The wire `cell` is
  32-bit: `LandblockX = cell>>>24`, `LandblockY = cell>>>16`, subcell =
  `cell & 0x3F`. The dats landblock id is `(cell >>> 16) & 0xffff` (the low
  16 bits are the subcell, so `cell & 0xffff` is wrong); its own high/low
  bytes are `LandblockX`/`LandblockY`.
* **Block-frame wrap.** Positions are block-local `[0,192)`. On a boundary
  crossing the server re-bases its own position into `[0,192)` and rolls the
  cell, but a client keeps the **spawn block's frame** (its sim is
  authoritative; it only adopts the new cell), leaving x/y off by 192.
  `wrapBlockCoord` (world3d.ts) wraps x/y back into the current block's frame
  before rendering.

Records (little-endian; widths via `cU32`):

| record | dat | id | content |
|--------|-----|----|---------|
| RegionDesc | portal | `0x13000000` | per-region (Holtburg) **256-entry height table**: heightmap byte → world z |
| CellLandblock | cell | `LB*0x10000+0xffff` | outdoor terrain: 81 terrain codes (u16) + 81 height bytes, index `x*9+y` (x W→E, y S→N); vertex `(x,y)` at `(x*24, y*24, table[height])` |
| LandblockInfo | cell | `LB*0x10000+0xfffe` | placed objects: `stabs` `{id, frame}` + `buildings`; a GfxObj stab has `(id & 0xff000000) === 0x01000000` |
| GfxObj | portal | `0x01xxxxxx` | a mesh (Polygons) — a placed building **or a character part model** |
| Surface | portal | `0x08xxxxxx` | a material: solid `0x00RRGGBB` or a SurfaceTexture id |
| SurfaceTexture | portal | `0x05xxxxxx` | ordered texture-data ids (first = base map) |
| Texture | portal | `0x06xxxxxx` | image data (DXT/raw); the renderer samples its **average** color |
| SetupModel | portal | `0x02xxxxxx` | the base body part list: `numParts×u32` part model ids (0x01…); the record's 5 trailing u32s are `[defAnim, defScript, defMTable, defSTable, defPSTable]` |
| MotionTable | portal | `0x09xxxxxx` | animation cycles keyed `(style, motion)` → `{anims, velocity, omega}` |
| Animation | portal | `0x03xxxxxx` | `numFrames×(numParts×Frame + hooks)`; Frame = 7 f32, a **model-space absolute** part transform (the parent chain is not composed); optional `numFrames×Frame` root pos frames (Flags&1) |

`frame` = `{ox,oy,oz}` + quaternion `{qw,qx,qy,qz}`; `applyFrame` rotates +
translates. **cU32** (`ReadCompressedUInt32`): the width is decided by the
first byte's bits — top bit clear → 1 byte; top set, `0x40` clear → 2 bytes
`((b0&0x7F)<<8)|b1`; top and `0x40` set → 4 bytes
`((((b0&0x3F)<<8)|b1)<<16)|u16`.

**GfxObj stops after the Polygons.** Layout: `Id, flags, nSurfaces(cU32),
surfaceIds, VertexPositions, [flags&1: physics Polygons + physics BSP],
12B SortCenter, [flags&2: drawing Polygons + drawing BSP]`. The drawing
**Polygons are the complete geometry**; the drawing BSP after them is a
visibility acceleration structure the server never parses, so parsing stops
right after the Polygons. Polygons are fan-triangulated and **both windings
are emitted** (each with its own material) so a flat-shaded z-buffer pass
backface-culls per screen winding.
**GfxObj physics (the collision hull).** With `flags&1` set, the record
carries a second Polygons + BSP pair *before* the drawing section — the
physics hull. The physics polygons are a coarse collision mesh distinct
from the drawing mesh, but their vertex keys index **the same
VertexPositions slots** (same `keyToSlot` map; `vertexId & 0xffff`).
`parseGfxObjPhysics` fan-triangulates them and transforms each vertex
through the object's placed `frame` (model space when `frame` is null),
returning the world-space triangles + 3D bbox (`GfxPhysics`). Walkability
is the **`Solid` field of the physics-BSP leaves**: `physicsBspSolid`
walks the tree with the same layout as `skipBsp` (leaf = `u32 type, i32
LeafIndex, i32 Solid, 16B Sphere, u32 n + n×u16` poly ids) and reports any
leaf with `Solid != 0`; a GfxObj blocks movement iff its physics section
has ≥1 solid leaf. The physics BSP is the last parsed section (no trailing
bytes).

**Texture average / matColor:** `parseTextureAverage` samples ≤512 DXT blocks
(≤4096 raw pixels) and packs the result as `0xffRRGGBB` (retail `RGBAUnion`
order, R in the high byte). The exact value `0xff000000` is the "no texture"
sentinel (rendered gray); any other `0xff…` high byte is a real color, so the
unknown-material test is `=== 0xff000000`, not "high byte nonzero".

**Actors.** A positioned in-world object (player, creature, door, item) is a
*SetupModel* (0x02…, its base body part list) + a *MotionTable* (0x09…, its
animation cycles) + per-part *GfxObj* meshes (0x01…). `ObjectCreate` carries the
live `setupId`/`mtableId` and a `ModelData` diff whose `nModels×(u8 partIndex +
packed modelId)` entries hold the **raw GfxObj index** — `src/messages.ts`
restores the `0x01` model-type byte at parse time (`modelId | 0x01000000`),
since the dat keys (and the `0x010001ec` null-part sentinel) carry the full id.
The SetupModel part list fixes the slot order; a `ModelData` entry overrides a
slot's model (the equipped gear).

**Animation.** Per frame the renderer resolves the *MotionTable* cycle for
`(style, motionCmd)`, samples its *Animation* at the actor's `animTime` — self
from the local movement sim, remotes from S2C `Motion` packets (a remote actor
idle >3 s falls back to `Ready`) — applies each part's model-space frame
(`TurnLeft`/`SideStepLeft` mirror x, `WalkBackwards` reverses frame order),
composes part + pose + object frame, and z-buffers the triangles with the
terrain. An object with no motion table (or an unresolvable cycle) holds the
setup's `defAnim` (first trailing data id) at frame 0, else the parts' authored
(identity) pose.

## 3. Movement physics (retail client-side dead reckoning)

The retail client simulates its own movement locally and ships the results to
the server (the NPK server is client-authoritative — see `docs/PROTOCOL.md`
§20). The numbers below are what the client uses; the server C# port
(`MotionInterp.cs`, `/tmp/ACE-research/Source`) mirrors them, which is how they
were cross-verified.

### 3.1 Constants

| constant | value | source |
|----------|-------|--------|
| WalkAnimSpeed | 3.12 u/s | server `MotionInterp.cs` |
| RunAnimSpeed | 4.0 u/s | server `MotionInterp.cs` |
| BackwardsFactor | 0.65 | server `MotionInterp.cs` |
| SidestepAnimSpeed | 1.25 u/s | server `MotionInterp.cs` |
| SidestepFactor | 0.5 | server `MotionInterp.cs` |
| MaxSidestepAnimRate | 3.0 | server `MotionInterp.cs` |
| RunTurnFactor | 1.5 | server `MotionInterp.cs` |
| **turn base rate** | **1.5 rad/s** | **retail motion table (below)** |
| gravity | 9.8 u/s² | jump integration |
| jump vz0 (fresh char, extent 1) | 2.75 u/s | see §3.4 |
| run rate R (fresh char) | 1.25 | weenie `InqRunRate` |

**Turn omega = 1.5 rad/s — extracted from the DAT.** The player motion table
(weenie 1 "human", type 10 = Container base, motion table id `0x09000001`,
resolved from `ace_world.weenie_properties_d_i_d` type=2 value=150994945)
defines every stance's animation cycles. The **TurnRight cycle** has angular
velocity `Omega = [0, 0, −1.5]` — i.e. 1.5 rad/s — in **all 14 stances**, and
bearing *increases* for TurnRight (clockwise from north). The client
dead-reckons heading at this rate; the server never integrates turns (it
adopts the client's orientation quaternion).

### 3.2 `adjust_motion` (server-side re-encoding of held keys)

The client sends *held-key* state (command + hold key + speed 1.0); the
server's `adjust_motion` converts it to the interpreted motion:

* `WalkBackwards` → `WalkForward × −0.65`
* `TurnLeft` → `TurnRight × −1`; `SideStepLeft` → `SideStepRight × −1`
* `SideStepSpeed ×= 0.5 × (3.12/1.25)` (SidestepFactor × WalkAnimSpeed /
  SidestepAnimSpeed) → net sidestep 1.56 u/s
* hold key = **Run**: `WalkForward → RunForward × R`; `TurnRight × 1.5`;
  `SideStepRight × R` clamped to ±3.0

### 3.3 Net velocities (what the sim produces)

| action | walk | run (R = 1.25) |
|--------|------|----------------|
| forward | 3.12 | 4.0 × R = 5.0 |
| back | 2.028 | 2.028 × R = 2.535 |
| strafe | 1.56 | min(1.56 × R, 3.75) = 1.95 |
| turn | 1.5 rad/s | 2.25 rad/s |

Velocity vector is clamped to length `4.0 × R` (retail `get_state_velocity`
maxSpeed).

### 3.4 Jump

Height = `loadMod × (skill/(skill+1300) × 22.2 + 0.05) × extent`, min 0.35 u.
Fresh character (Jump skill 20, loadMod 1, extent 1) ≈ **0.386 u**. Takeoff
`vz = sqrt(h × 2g) ≈ 2.75 u/s`, `g = 9.8`, airtime ≈ `2·vz/g ≈ 0.561 s`.
The local sim integrates semi-implicit Euler at the 50 ms tick; its discrete
apex at dt = 0.05 is ≈ 0.32 u (Euler undershoot) — see
`tests/movement.ts` for the pinned values.

## 4. Wire formats (byte-exact; client decompile + server C# agree)

All C2S movement is a `GameAction` (§7 of PROTOCOL.md: `u32 0xF7B1` +
`u32 clientSeq` + `u32 actionType` + body).

* **Position (32 B)**: `u32 cell` + `f32 x, y, z` + `f32 qw, qx, qy, qz`.
* **RawMotionState (48 B here)**: `u32 PackedFlags` (bits 0–10 = field
  present, bits 11+ = command count, sent **0**) then, in order:
  `0x1 currentHoldKey u32 · 0x2 currentStyle u32 · 0x4 forwardCommand u32 ·
  0x8 forwardHoldKey u32 · 0x10 forwardSpeed f32 · 0x20 sidestepCommand u32 ·
  0x40 sidestepHoldKey u32 · 0x80 sidestepSpeed f32 · 0x100 turnCommand u32 ·
  0x200 turnHoldKey u32 · 0x400 turnSpeed f32`.
  **All 11 fields are always sent** (flags = `0x7ff`): the server's C#
  defaults are 0/0.0 (not the retail defaults) and `apply_raw_movement`
  copies raw→interpreted verbatim, so an omitted speed field reads 0.0.
* **MoveToState `0xF61C` body (89 B)**: RawMotionState + Position +
  `u16 ×4` (instance/serverControl/teleport/forcePosition; zeros) +
  `u8 flags` (bit 0 contact, bit 1 longJump). Server `reader.Align()` after
  the byte (no-op at the end).
* **AutonomousPosition `0xF753` body (41 B)**: Position + `u16 ×4` +
  `u8 contact`.
* **Jump `0xF61B` body (32 B)**: `f32 extent` + `f32 vx, vy, vz` + `u16 ×4`
  (zeros) + `u32 objectGuid` + `u32 spellId` (sent 0/0). This is the
  **server's** 32-byte JumpPack, not the retail 56-byte form.
* **PlayerTeleport S2C `0xF751`**: `u32` teleport sequence (C#
  `Writer.Write(uint)`) + align. The new position arrives in the subsequent
  `UpdatePosition` / `ObjectCreate`.

HoldKey wire values: Invalid = 0, None = 1, Run = 2.
`currentHoldKey = running ? 2 : 1`; per-axis hold key 0 when unheld, else
running ? 2 : 1; speeds always 1.0; style always `0x8000003d`.

**Run encoding**: "running" is sent as `WalkForward` + `forwardHoldKey =
Run(2)`; the server's `adjust_motion` turns that into `RunForward × R`. The
local sim mirrors this exactly, so local and server agree.

**Motion command list: count 0, always.** The server logs an error for
non-SoulEmote command-list entries; locomotion is fully described by the
command/hold-key fields above.

## 5. Server correction policy (client side)

The NPK server is **client-authoritative** for movement (server C# verified:
`GameActionMoveToState` → `SetRequestedLocation` + `BroadcastMovement`
unconditionally; `GameActionAutonomousPosition` → `SetRequestedLocation` +
`LastGroundPos` on contact). But it **echoes the position we sent** back via
self `UpdatePosition` — which *lags the live sim while moving*. Snapping to
the echo would cause backward jitter. Policy (in `session.ts`
`_selfPositionUpdate`):

* `PlayerTeleport` seen → next self `UpdatePosition` **snaps unconditionally**.
* else if |received − last-sent| > `MOVE_SNAP_EPS` (0.25 u) → **server
  correction** (collision/bounds/forced move) → snap the sim.
* else → it is our own **echo** → ignore (adopt the landblock `cell` only).

After any snap the motion signature is cleared to force a fresh
MoveToState broadcast.

**Send cadence** (`session.ts`): sim tick 50 ms; `MoveToState` on every
motion-state change (8-bit input signature incl. airborne);
`AutonomousPosition` at most once per second while moving. Held keys expire
120 ms after their last re-assertion (raw terminals have no keyup).

## 6. TUI key map (retail mirror)

`src/ui.tsx` world view: `w`/↑ forward · `x`/↓ back · `a`/← turn left ·
`d`/→ turn right · `z`/`c` strafe · `s` stop · space jump. Shift (or any
uppercase letter) forces **walk** for that press; otherwise the **run lock**
(`q`/`Q`, default **on** = run) decides — retail's default is run.

## 7. Verification

* `tests/movement.ts` — deterministic sim + wire-encoding assertions (no
  server). Pins every number in §3/§4.
* `tests/movement_live.ts` — real pty vs. a live server: held keys move the
  radar readout (local stack) **and** a partner session observes the position
  change on the wire (server acceptance + broadcast).
* `docs/PROTOCOL.md` §20 — the wire formats as they belong in the protocol
  spec.

## 8. Combat (melee) — retail decompile evidence

The retail combat loop is three `CM_Combat` order builders, each packed via
`OrderHdr` + `Proto_UI::SendToWeenie` (the `GameAction` §7 wrapper in
PROTOCOL.md) — the opcodes and bodies match the server's C# writers exactly:

* **`CM_Combat::Event_ChangeCombatMode(COMBAT_MODE i_mode)`** — writes
  `u32 83` (`0x53`) + `u32 mode`.
* **`CM_Combat::Event_TargetedMeleeAttack(u32 i_targetID, ATTACK_HEIGHT
  i_ah, float i_power_level)`** — writes `u32 8` (`0x0008`) + `u32 target`,
  a 4-byte align, then `u32 attackHeight` + `f32 power` (24-byte body).
* **`CM_Combat::Event_QueryHealth(u32 i_target)`** — writes `u32 447`
  (`0x1BF`) + `u32 target`.
* **`CM_Combat::Event_CancelAttack()`** — exists in retail (fired on stance
  change / lost target), but the TUI never sends it: the server auto-cancels
  an in-flight swing when a `ChangeCombatMode` away from melee is accepted,
  so a dedicated cancel is not needed client-side.

**Default swing values** (the retail left-click, non-advanced mode). The
`ClientCombatSystem` constructor sets `requestedAttackHeight = 2`
(`MEDIUM_ATTACK_HEIGHT`); `StartAttackRequest` sets `requestedAttackPower`
to `1065353216` = `0x3F800000` = **float 1.0** (full power).
`ExecuteAttack(ATTACK_HEIGHT, bool)` routes to
`Event_TargetedMeleeAttack(target, height, power)` when `combatMode == 2`
(melee). So a default swing is **Medium height + full power** — exactly what
`buildTargetedMeleeAttack` sends (`power 1.0`, `attackHeight 2`). The
`ATTACK_HEIGHT` enum: `UNDEF 0, HIGH 1, MEDIUM 2, LOW 3`.

**Key map** (`docs/artifacts/acclient default.keymap`):

* **`CombatToggleCombat` = `DIK_GRAVE` (the backtick `` ` ``)** — retail's
  real stance toggle; the TUI uses the same key and (unlike retail's local
  UI toggle) also sends `ChangeCombatMode` so the server is authoritative.
* `CombatMediumAttack` = `DIK_END`, `CombatHighAttack` = `DIK_PGDN`,
  `CombatLowAttack` = `DIK_DELETE` — explicit attack-height selection.
* `CombatDecreaseAttackPower` = `DIK_INSERT`, `CombatIncreaseAttackPower` =
  `DIK_PGUP` — the power bar. The TUI has no power-bar UI; a swing is always
  the full-power default.

**Attack path** (retail left-click on a targeted creature while in combat):
`StartAttackRequest` → `AttemptStartBuildingAttack` (power-bar build in
advanced mode, immediate otherwise) → `ExecuteAttack(requestedAttackHeight,
true)` → `Event_TargetedMeleeAttack`. With no valid combat target selected it
prints "You must select a valid combat target before attacking" — the gate
the TUI mirrors with its `no creature targeted` / `target out of range`
flashes (the TUI attack key is client-specific: `T`, plus the click-to-attack
in combat stance).

Wire byte-exact details: `docs/PROTOCOL.md` §25. Client mapping (keys,
feedback lines, the status-bar `%H`): `docs/AC-UI.md` "Combat (melee, TUI)".
