# ACEmulator (Asheron's Call) — Client-Side Wire Protocol

Byte-exact specification for writing a from-scratch headless client that can:
connect, authenticate, list/select a character, enter the world, send/receive chat,
see other players' positions (radar), and log out — plus everything the server needs
to keep a session alive.

**Source of truth:** `ACEmulator/ACE` @ commit `47edade`, read-only at `/tmp/ACE-research`.
All C# `BinaryReader`/`BinaryWriter`/`BinaryPrimitives` calls are **little-endian**.
All integer arithmetic in checksums/ISAAC is **mod 2^32** (C# `uint`).
Every claim cites `Source/path:line`. Where the source is ambiguous or has dead code,
it is stated explicitly (see §13). No silent guessing.
**Client-side implementation** of every message described here (dispatch
handlers, order builders, login handshake, ISAAC/CryptoSystem) is documented
in `docs/ACCLIENT.md` (§3 networking, §7 player data, §13 character
creation, §16 security).

Conventions used in the layout tables below:

| token | meaning |
|-------|---------|
| `u8/u16/u32/u64` | unsigned integer, little-endian |
| `i32/i64` | signed integer, little-endian |
| `f32/f64` | IEEE-754 float/double, little-endian |
| `rawN` | N raw bytes |
| `String16L` / `String32L` | see §3 |
| `packedDword` | see §11.6 |
| `pad(n→4)` / `Align` | zero-fill so the current byte length is a multiple of 4 |

---

## 1. Transport overview

* **Protocol:** UDP. Two server ports (default `9000`): the base port for C2S and base+1
  (`9001`) for the S2C side. `Source/ACE.Common/NetworkSettings.cs:15-20`
  (`Port = 9000`; comment: "This also opens the next port up, 9001, for server to client
  communications").
* **Client socket model:** one local UDP socket bound to an arbitrary port `P`. The client
  must send **all** C2S traffic (LoginRequest, ConnectResponse, and every later game packet)
  from that same port `P`; the server binds each session to the C2S source endpoint and
  drops C2S packets arriving from any other endpoint (`NetworkManager.cs:155-158`,
  `session.EndPointC2S.Equals(endPoint)`).
* **Three-way handshake ports** (confirmed in `NetworkSession.cs:785-788` and
  `NetworkManager.cs:44-165`):
  1. `LoginRequest` is sent by the client **to `9000`**.
  2. The server sends `ConnectRequest` **from `9000`** back to the client's `P`
     (before the S2C endpoint is known the server transmits on the C2S socket —
     `NetworkSession.cs` `SendPacketRaw`: `socket = (session.EndPointS2C is null) ?
     connectionC2S.Socket : connectionS2C.Socket`).
  3. The client sends `ConnectResponse` **to `9001`**.
  4. Thereafter **all C2S → `9000`** and **all S2C ← `9001`**.
* **Port `9001` accepts only** a `ConnectResponse` (or an `Id==0` `CICMDCommand`); anything
  else is rejected with an error log (`NetworkManager.cs:47-80`).
* **Size limits:** a C2S datagram is rejected if `Header.Size` exceeds the buffer or if the
  total exceeds `ClientPacket.MaxPacketSize = 1024` (`ClientPacket.cs:14,29-40`). A single
  fragment must have `Size ≤ 464` (i.e. data ≤ 448) — `ClientPacketFragment.cs:17`,
  `PacketFragment.cs` (`MaxFragmentDataSize`).
* **Session index / `NetID`:** the server keeps `sessionMap[]` indexed by a session id.
  `FindOrCreateSession` assigns the lowest free index and stores
  `sessionMap[i] = new Session(…, clientId: i, …)` (`NetworkManager.cs:265-297`). That id is
  exposed as `session.Network.ClientId` (`Session.cs:87-99`) and is what the server later
  uses to look the session up for every non-login C2S packet:
  `session = sessionMap[packet.Header.Id]` (`NetworkManager.cs:152-154`). Therefore:
  **every C2S game packet after the handshake must have `Header.Id = NetID`**, where `NetID`
  is the `clientId` field in the `ConnectRequest` body (§5, step 2).

---

## 2. Packet framing

A datagram is: `Header (20 B)` + `optional header (0..N B)` + `fragments (0..M)`.
`Header.Size` = total bytes *after* the 20-byte header (optional header + all fragments).

### 2.1 Header (20 bytes, LE) — `Source/ACE.Server/Network/PacketHeader.cs:11-53`

| off | field | type | notes |
|----:|-------|------|-------|
| 0 | `Sequence` | u32 | see §3.5 / §5 step 4 |
| 4 | `Flags` | u32 | `PacketHeaderFlags` bitmask (§2.2) |
| 8 | `Checksum` | u32 | see §4 |
| 12 | `Id` | u16 | C2S: `0` for LoginRequest, `NetID` otherwise; S2C: `ServerId = 0x000B` (`NetworkManager.cs:26`) |
| 14 | `Time` | u16 | S2C: `(u16) PortalYearTicks` (`NetworkSession.cs:752`). C2S: parsed but not used by any C2S consumer. |
| 16 | `Size` | u16 | bytes after the header |
| 18 | `Iteration` | u16 | S2C: `0x0001` (`NetworkSession.cs:751`). C2S: ignored. |

The header hash is computed with the `Checksum` slot forced to `0xBADD70DD`
(`PacketHeader.cs:55-67`).

### 2.2 `PacketHeaderFlags` — `Source/ACE.Server/Network/PacketHeaderFlags.cs`

```
None               0x00000000     Retransmission   0x00000001
EncryptedChecksum  0x00000002     BlobFragments    0x00000004
ServerSwitch       0x00000100     LogonServerAddr  0x00000200
EmptyHeader1       0x00000400     Referral         0x00000800
RequestRetransmit  0x00001000     RejectRetransmit 0x00002000
AckSequence        0x00004000     Disconnect       0x00008000
LoginRequest       0x00010000     WorldLoginRequest 0x00020000
ConnectRequest     0x00040000     ConnectResponse  0x00080000
NetError           0x00100000     NetErrorDisconnect 0x00200000
CICMDCommand       0x00400000     TimeSync         0x01000000
EchoRequest        0x02000000     EchoResponse     0x04000000
Flow               0x08000000
```

### 2.3 Optional header — `Source/ACE.Server/Network/PacketHeaderOptional.cs:24-127`

Parsed **in this order**, consuming bytes only for fields whose flag is set. The optional
header's own checksum is `Hash32` over exactly the consumed bytes in this order
(`PacketHeaderOptional.cs:129-134`).

**Consumed (in code order):**

| flag | name | bytes consumed |
|------|------|----------------|
| `ServerSwitch` 0x100 | — | 8 raw |
| `RequestRetransmit` 0x1000 | — | u32 count + count×u32 |
| `RejectRetransmit` 0x2000 | — | u32 count + count×(4 raw) |
| `AckSequence` 0x4000 | ack | u32 |
| `LoginRequest` 0x10000 | login body | **all remaining bytes**, then stream position is rewound to the start of this field (so there are no fragments; the login data *is* the payload) |
| `WorldLoginRequest` 0x20000 | — | 8 raw (peeked, then position rewound — dead path, §13) |
| `ConnectResponse` 0x80000 | check | 8 raw (peeked, then position rewound) |
| `CICMDCommand` 0x400000 | — | 8 raw (consumed, no rewind) |
| `TimeSync` 0x1000000 | — | f64 |
| `EchoRequest` 0x2000000 | — | f32 |
| `Flow` 0x8000000 | — | u32 + u16 |

**Flags that consume 0 bytes** (metadata only): `LogonServerAddr` 0x200, `EmptyHeader1`
0x400, `Referral` 0x800, `Disconnect` 0x8000, `NetError` 0x100000, `NetErrorDisconnect`
0x200000, `EchoResponse` 0x4000000, `Retransmission` 0x1, `EncryptedChecksum` 0x2,
`BlobFragments` 0x4.

**S2C write order** (what the client will receive) — `NetworkSession.cs` builds, in this
order: `AckSequence` (u32 = last received C2S seq, written at `NetworkSession.cs:962`) →
`TimeSync` (f64 `PortalYearTicks`) → `EchoResponse` (f32 clientTime + f32
(PortalYearTicks − clientTime)). **Important:** `EchoResponse` (0x4000000) is *not* in the
C2S `Unpack` list, so a client parsing **S2C** must skip **8 bytes** for it, appearing after
`TimeSync`. A minimal client can simply treat `EchoResponse` as "skip 8".

### 2.4 Fragments — header 16 bytes (LE)

Present only when `BlobFragments` (0x4) is set; then fragments run to the end of the payload.
`Source/ACE.Server/Network/PacketFragmentHeader.cs:11-40`.

| off | field | type | notes |
|----:|-------|------|-------|
| 0 | `Sequence` | u32 | C2S: global per-connection fragment counter (§5 step 4). |
| 4 | `Id` | u32 | S2C: `0x80000000` (`MessageFragment.cs`). C2S: not validated. |
| 8 | `Count` | u16 | number of fragments making up this message (`1` = single) |
| 10 | `Size` | u16 | `16 + len(data)`; must be ≤ 464 |
| 12 | `Index` | u16 | fragment index `0..Count-1` |
| 14 | `Queue` | u16 | S2C: `GameMessageGroup` (§7). C2S: not validated. |

* **C2S read** (`ClientPacketFragment.cs:10-34`): the server reads the 16-byte header, checks
  `Size` sanity, then reads exactly `Size − 16` data bytes. It does **not** validate `Id` or
  `Queue`. Per-fragment checksum = `Hash32(16B header) + Hash32(data)`.
* **S2C reassembly (client side):** group fragments by `Sequence`, order by `Index`,
  concatenate `data`. The reassembled byte stream is one *game message* = `u32 opcode` +
  body (§7). Most S2C messages are single-fragment; `PlayerDescription` (§12.4) can be
  multi-fragment (tens of KB) and **must** be reassembled.
* A C2S message is delivered only when its fragment `Sequence == lastReceivedFragmentSequence
  + 1` (`NetworkSession.cs:551`); early fragments are buffered and replayed in order
  (`NetworkSession.cs:581-599`).

---

## 3. Strings

### 3.1 `String16L` — used by nearly every message

* **Server write** — `Source/ACE.Server/Network/Extensions.cs:12-21`:
  `u16 length` (= `data.Length`, the **byte** count) + the UTF-8/Windows-1252 bytes +
  `pad((2 + length) → 4)`.
* **Server read** — `Source/ACE.Common/Extensions/BinaryReaderExtensions.cs:41-49`:
  `u16 length`; `ReadChars(length)`; then skip `pad(2 + length → 4)`.

```
String16L := u16 L ; <L bytes> ; pad so (2 + L) is a multiple of 4
```

**Encoding:** the server *writes* with `Encoding.GetEncoding(1252)` (1 byte/char for the
ASCII range) and *reads* with the default `BinaryReader` encoding, which is **UTF-8**
(`ClientMessage.cs:17,25` construct a bare `new BinaryReader(stream)`; ACE targets
`net10.0`, `ACE.Server.csproj:5`). For the ASCII subset (login names, chat, character names)
Windows-1252 and UTF-8 are byte-identical, and `ReadChars(L)` consumes exactly `L` bytes.
**Use ASCII/UTF-8 with `L = byte count`.** For multi-byte (non-ASCII) text the length field
(counts *characters* on read, *bytes* on write) can diverge and the stream would
misalign — avoid non-ASCII in this minimal client (§13).

### 3.2 `String32L` — **login password only**

`Source/ACE.Common/Extensions/BinaryReaderExtensions.cs:12-39`. The source comment calls it
"crazy". Read path: `u32 X`; if `X==0` → empty; else skip 1 byte, `X--`; if `X>255` skip
another byte, `X--`; `ReadChars(X)`; skip `pad(4 + X_final → 4)`.

```
String32L (password, S = byte length):
  u32 X          X = S+1  if S <= 255, else S+2
  <1 byte S>     if S <= 255   (else <2 bytes S>)   ; value discarded by the server
  <S bytes>
  pad so (4 + S) is a multiple of 4   ; trailing pad is NOT validated (end of packet)
```

Because this is the **last field of the LoginRequest**, the trailing padding is not checked
(anything after the password bytes is never read). The client should still emit the 4-aligned
padding to match the retail layout.

---

## 4. Checksums

### 4.1 `Hash32` — `Source/ACE.Common/Cryptography/Hash32.cs:24-37`

```
h = (uint)(length << 16)
for i in 0,4,8,… while i+4 <= length:      h += LE_u32(data[i .. i+4))
shift = 3
j = (length/4)*4
while j < length:                          h += (uint)(data[j] << (8*shift)); shift--; j++
all mod 2^32
```

### 4.2 Header hash — `PacketHeader.cs:55-67`

`Pack` the 20-byte header with the `Checksum` slot set to `0xBADD70DD`, then `Hash32` the
20 bytes. Call the result `headerHash`.

### 4.3 What the client must send (C2S)

`payloadHash = Hash32(optionalHeaderBytes) + Σ fragments (Hash32(fragHeader16) + Hash32(fragData))`
(all mod 2^32). The server verifies in `ClientPacket.cs:138-165`:

* **Plain** (no `EncryptedChecksum` flag): valid iff `headerHash + payloadHash == Checksum`.
* **Encrypted** (`EncryptedChecksum` flag set): `key = (Checksum − headerHash) ^ payloadHash`;
  valid iff the session's `CryptoSystem` (C2S stream) contains `key` via `Search`, then
  `ConsumeKey(key)`.

So the client computes:
```
plain:     Checksum = headerHash + payloadHash
encrypted: Checksum = headerHash + (payloadHash ^ xorKey)      xorKey = next C2S-ISAAC value
```
The server accepts **plain** checksums at any stage — `VerifyCRC` has no stage gating
(`ClientPacket.cs:138-165`). A minimal client may therefore use plain checksums everywhere;
encrypted is the retail-faithful option (§6).

**Per-packet notes:**
* `LoginRequest`: plain, verified (`NetworkSession.cs:284` → `VerifyCRC`).
* `ConnectResponse`: **checksum is never verified** — the `9001` path
  (`NetworkManager.cs:47-80`) does not call `VerifyCRC`; it only matches cookie+IP. Plain is
  safe here.
* All other C2S: verified; plain or encrypted both work.

### 4.4 What the client receives (S2C)

`Source/ACE.Server/Network/ServerPacket.cs:46-72`:
```
payloadHash = Hash32(optionalHeaderBytes) + Σ fragments (Hash32(fragHeader16) + Hash32(fragData))
Header.Checksum = headerHash + (payloadHash ^ issacXor)        (mod 2^32)
```
where `issacXor` is one `Next()` from the S2C ISAAC stream for each encrypted S2C packet
(`NetworkSession.cs:767-772`), or `0` when unencrypted. A client can verify incoming S2C with
the mirror of §4.3.

---

## 5. Connection & authentication (numbered recipe)

### Step 0 — open socket
Open one UDP socket, bind local port `P`. Remember `P`.

### Step 1 — C2S `LoginRequest` → `9000`
Header: `Flags = 0x10000`, `Sequence = 0`, `Id = 0`, `Time = 0`, `Iteration = 0`,
`Size = len(body)`, **plain** checksum. (The `LoginRequest` optional-header flag consumes the
entire remaining payload, `PacketHeaderOptional.cs:69-79`; the login body is then parsed from
the start of that blob by `PacketInboundLoginRequest.cs:21-36`.)

Body (in order):
```
String16L  ClientVersion      ; MUST be exactly "1802" (AuthenticationHandler.cs:113-118)
u32        len                ; ignored — send 0
u32        NetAuthType        ; 2 = AccountPassword (NetAuthType.cs)
u32        AuthFlags          ; ignored — send 0
u32        Timestamp          ; ignored — send 0
String16L  Account            ; <= 50 chars (AuthenticationHandler.cs:38-43)
String16L  accountToLoginAs   ; ignored — send the account again
String32L  Password           ; §3.2
```
Server behavior: if the account does not exist and `NetAuthType == AccountPassword` and the
password is non-empty and config `AllowAutoAccountCreation` is true (default), the account is
**auto-created** with a lowercased name (`AuthenticationHandler.cs:55-98`). Wrong password →
`AccountBoot` ("…password … was not correct"); account already in use → `CharacterError.Logon`
(`AuthenticationHandler.cs:157-196`).

### Step 2 — S2C `ConnectRequest` (from `9000`, `Sequence = 0`, `Flags = 0x40000`, plain)
Body, 32 bytes, in order — `Source/ACE.Server/Network/Packets/PacketoutboundConnectRequest.cs:5-16`:
```
f64  serverTime        ; PortalYearTicks
u64  cookie            ; the shared 64-bit secret (echo back in step 3)
u32  clientId          ; = NetID = session index (1-based)
raw4 OutgoingSeed      ; = ServerSeed (S2C ISAAC seed)
raw4 IncomingSeed      ; = ClientSeed (C2S ISAAC seed)
u32  0                 ; padding
```
**ServerSeed comes BEFORE ClientSeed.** From this the client builds both ISAAC streams (§6).

### Step 3 — C2S `ConnectResponse` → `9001`
Header: `Flags = 0x80000`, `Sequence = 0`, `Id = NetID`, **plain** checksum (unverified, §4.3).
Body (the `ConnectResponse` optional-header field, 8 bytes):
```
u64  Check = <cookie echoed from step 2>
```
The server matches `cookie + C2S-IP + state==AuthConnectResponse`, sets the S2C endpoint to
`P`, and moves to `AuthConnected` (`NetworkManager.cs:60-81`, `AuthenticationHandler.cs:236-251`).

### Step 4 — all later C2S → `9000`
* **`Header.Id = NetID`** (mandatory; `sessionMap[Header.Id]` lookup, §1).
* **Packet `Sequence`:** strictly increasing starting at **2** (the server's
  `lastReceivedPacketSequence` starts at 1 and Login/ConnectResponse are seq-0 or
  non-updating, so the first in-order game packet must be `2`; `NetworkSession.cs:57,349-379,487`).
  Use `2, 3, 4, …`. **PAKs (`AckSequence`) never consume a number**: the server does not advance
  `lastReceivedPacketSequence` for them (`NetworkSession.cs:486-487`) and its duplicate check
  explicitly accepts `Ack && seq == last` (line 350), so each PAK must **reuse** the current value
  (the last real packet's sequence; `1` before any real packet). A PAK that consumed a number
  pushes the next real packet to `seq > last+1`, where it is parked out-of-order and never
  processed, while the server NAKs the missing gap (`Requested retransmit of N`).
* **Fragment `Sequence`:** a *global* per-connection counter, strictly increasing, starting at
  **1** (the first fragment processed is the first game packet;
  `lastReceivedFragmentSequence` starts at 0, `NetworkSession.cs:58,551,573`). Use `1, 2, 3, …`
  across all C2S fragments.
* Reorder rules (`NetworkSession.cs:349-428`): a packet is dropped if
  `seq <= last && seq != 0 && !(Ack && seq == last)`; if `seq > last+1` it is stored
  out-of-order and a NAK (`RequestRetransmit`) is sent when the gap is ≥ 2 and > 1 s has
  elapsed since the last NAK; the session is **terminated** (`AbnormalSequenceReceived`) if
  `rcvdSeq < desired+1` or `rcvdSeq − bottom > 256`. Keep C2S sequences strictly increasing and
  gap-free.

### Step 5 — first S2C bundle (encrypted)
The server sends `CharacterList (0xF658)` + `ServerName (0xF7E1)` in one UIQueue packet and
`DDD_Interrogation (0xF7E5)` in a separate DatabaseQueue packet
(`AuthenticationHandler.cs:253-264`). S2C `Sequence`: `ConnectRequest = 0`; the first
encrypted packet re-inits the S2C sequence counter to `UIntSequence(1)` so that packet is
`Sequence 2`, then `3, 4, 5, …` (`NetworkSession.cs:740-747`). Ack-only/NAK packets reuse the
current value (duplicate seq). The client keys off **opcode**, not seq. (Which of the two
bundles is seq 2 vs 3 depends on bundle-group iteration order — do not depend on it, §13.)

---

## 6. ISAAC & `CryptoSystem` (encryption)

All arithmetic mod 2^32. State: `mm[256]`, `randRsl[256]`, `offset` (init 255), `a,b,c`.
Source: `Source/ACE.Common/Cryptography/ISAAC.cs`.

### 6.1 `Initialize(seed)` — `ISAAC.cs:50-83`
```
mm[i] = randRsl[i] = 0            (i = 0..255)
abcdefgh[0..7] = 0x9E3779B9
Shuffle(abcdefgh) ×4
for i in 0..1:
    for j in 0,8,16,…,248:
        for k in 0..7:  abcdefgh[k] += (i==0 ? randRsl[j+k] : mm[j+k])
        Shuffle(abcdefgh)
        for k in 0..7:  mm[j+k] = abcdefgh[k]
a = LE_u32(seed[0..3])            ; ONLY the first 4 seed bytes
c = b = a
IsaacScramble()
```

### 6.2 `IsaacScramble()` — `ISAAC.cs:85-111`
```
b += ++c
for i in 0..255:
    x = mm[i]
    switch (i & 3):
        0: a ^= (a << 13)
        1: a ^= (a >> 6)
        2: a ^= (a << 2)
        3: a ^= (a >> 16)
    a += mm[(i + 128) & 0xFF]
    y = mm[(x >> 2) & 0xFF] + a + b
    mm[i] = y
    b = randRsl[i] = mm[(y >> 10) & 0xFF] + x
```

### 6.3 `Shuffle(x)` — `ISAAC.cs:113-123` (20 statements, in order)
```
x[0]^=x[1]<<0x0B; x[3]+=x[0]; x[1]+=x[2];
x[1]^=x[2]>>0x02; x[4]+=x[1]; x[2]+=x[3];
x[2]^=x[3]<<0x08; x[5]+=x[2]; x[3]+=x[4];
x[3]^=x[4]>>0x10; x[6]+=x[3]; x[4]+=x[5];
x[4]^=x[5]<<0x0A; x[7]+=x[4]; x[5]+=x[6];
x[5]^=x[6]>>0x04; x[0]+=x[5]; x[6]+=x[7];
x[6]^=x[7]<<0x08; x[1]+=x[6]; x[7]+=x[0];
x[7]^=x[0]>>0x09; x[2]+=x[7]; x[0]+=x[1];
```

### 6.4 `Next()` — `ISAAC.cs:31-48`
```
v = randRsl[offset]
if offset > 0:  offset -= 1
else:           IsaacScramble(); offset = 255
return v
```

### 6.5 `CryptoSystem` — `Source/ACE.Common/Cryptography/CryptoSystem.cs:6-56`
Subclass of ISAAC. Constructor: `CurrentKey = Next()` — i.e. it **consumes and discards the
first value** of the stream. `Search(x)`: true if `x == CurrentKey`, or `x` is in the `xors`
set, or reachable within `MaximumEffortLevel − xors.Count` (≤ 256) look-aheads (advancing the
stream as it searches). `ConsumeKey(x)`: if `x == CurrentKey` advance to the next value, else
remove `x` from `xors`. This tolerates a bounded amount of out-of-order/retransmitted
packets.

### 6.6 Stream wiring — `Source/ACE.Server/Network/SessionConnectionData.cs:44-70`
The server seeds: `ClientSeed` and `ServerSeed` (each 4 random bytes), `ConnectionCookie`
(8 random bytes → u64 LE). It builds:
* `CryptoClient = new CryptoSystem(ClientSeed)` — used to **verify C2S** (skips 1st value;
  one value per encrypted C2S packet, `ClientPacket.cs:138-165`).
* `IssacServer = new ISAAC(ServerSeed)` — used for **S2C** (no skip; one `Next()` per
  encrypted S2C packet, `NetworkSession.cs:767-772`).

The client mirrors this:
* **C2S stream** = ISAAC seeded with **ClientSeed** (the `IncomingSeed` field). Discard the
  first `Next()`; then XOR its next value into `payloadHash` for each **encrypted** C2S packet.
* **S2C stream** = ISAAC seeded with **ServerSeed** (the `OutgoingSeed` field). No discard;
  value #1 corresponds to the **first encrypted S2C packet**, and so on.

> A minimal client does not need to *encrypt* C2S (plain checksums are accepted, §4.3) and
> does not need to *verify* S2C. It only needs the S2C stream if it wants to decrypt/verify
> S2C. The plain path is the simplest fully-working option.

---

## 7. Game message layer

* A fragment's data is one **game message**: `u32 opcode` + body
  (`ClientMessage.cs:14-19` reads the leading u32 as the opcode).
* **Dispatch & state gating (S2C→handler / C2S→handler):** handlers are registered by opcode
  with a required `SessionState`; a message whose opcode's state does not match the current
  session state is **silently ignored** (no error sent) —
  `InboundMessageManager.cs:86-120`. So send each message only in the state where it is
  valid (noted per-message below).
* **S2C `GameEvent` wrapper** (opcode `0xF7B0`) — `GameEventMessage.cs:15-52`:
  ```
  u32 0xF7B0 ; u32 playerGuid ; u32 GameEventSequence ; u32 EventType ; <body>
  ```
  `playerGuid` is `0` before a player is set; `GameEventSequence` is a server counter starting
  at 1.
* **C2S `GameAction`** (opcode `0xF7B1`) — `GameActionPacket.cs:9-17`:
  ```
  u32 0xF7B1 ; u32 clientSequence ; u32 GameActionType ; <body>
  ```
  `clientSequence` is **not validated** (the source has a `// TODO: verify sequence`).
* **`GameMessageGroup`** (S2C fragment `Queue`) — `GameMessageGroup.cs:3-18`:
  `0x00` Invalid, `0x01` Event, `0x02` Control, `0x03` Weenie, `0x04` Login, `0x05`
  Database, `0x06` SecureControl, `0x07` SecureWeenie, `0x08` SecureLogin, `0x09` UI,
  `0x0A` Smartbox, `0x0B` Observer. (C2S queue is not validated.)

`SessionState` values relevant here: `AuthLoginRequest` → `AuthConnectResponse` →
`AuthConnected` → `WorldConnected`.

---

## 8. Character list / login S2C messages

### 8.1 `CharacterList` `0xF658` (UIQueue) — `GameMessageCharacterList.cs:12-38`
```
u32 0
u32 count
repeat count times:
    u32 characterGuid
    String16L name            ; may be "+"-prefixed for plussed admins
    u32 timeSinceDelete       ; 0 if not deleted
u32 0
u32 maxCharsPerAccount
String16L account
u32 useTurbineChat            ; 0/1
u32 hasThroneOfDestiny        ; 1
```
The client reads `count`, then each `(guid, name)` — that is the selectable character list.

### 8.2 `ServerName` `0xF7E1` (UIQueue) — `GameMessageServerName.cs:5-11`
```
u32 currentConnections
u32 maxConnections
String16L serverName
```

### 8.3 `DDD_Interrogation` `0xF7E5` (DatabaseQueue) — `GameMessageDDDInterrogation.cs:7-20`
```
u32 1          ; server region
u32 1          ; name-rule language
u32 productID  ; 0x1, or 0x5 if allow_highres_dat
u32 2          ; supported-language count
u32 0          ; Invalid
u32 1          ; English
```
The C2S `DDD_InterrogationResponse` (`0xF7E6`, state `AuthConnected`, `DDDHandler.cs:24`) is
**optional** — the server never gates world-entry on it, so a minimal client may ignore the
interrogation and go straight to character entry.

---

## 9. Character creation — C2S `0xF656` (state `AuthConnected`)

After the `u32 opcode`, in order — `CharacterCreateInfo.cs:37-68`, `Appearance.cs:28-50`:
```
String16L account             ; MUST == session.Account or the message is silently dropped (CharacterHandler.cs:29-32)
raw4    unknown               ; "Unknown constant" — send 1
u32     Heritage              ; HeritageGroup (below)
u32     Gender                ; 1 = Male, 0 = Female
; Appearance (104 bytes):
14 × u32  [Eyes, Nose, Mouth, HairColor, EyeColor, HairStyle, HeadgearStyle, HeadgearColor,
           ShirtStyle, ShirtColor, PantsStyle, PantsColor, FootwearStyle, FootwearColor]
6  × f64  [SkinHue, HairHue, HeadgearHue, ShirtHue, PantsHue, FootwearHue]
i32     TemplateOption
6 × u32   attributes          ; Strength, Endurance, Coordination, Quickness, Focus, Self — each 10..100, sum <= heritage AttributeCredits
u32     CharacterSlot         ; ignored
u32     ClassId               ; ignored
u32     NumOfSkills           ; MUST be 55 (non-Olthoi) or the account is BOOTED as a client/server skill mismatch (CharacterHandler.cs:137-141)
55 × u32  SkillAdvancementClass ; 0 Inactive, 1 Untrained, 2 Trained, 3 Specialized
String16L Name
u32     StartArea
u32     IsAdmin               ; 0/1
u32     IsSentinel            ; 0/1
```
`HeritageGroup` (`HeritageGroup.cs`): Invalid 0, Aluvian 1, Gharundim 2, Sho 3, Viamontian 4,
Shadowbound 5, Gearknight 6, Tumerok 7, Lugian 8, Empyrean 9, Penumbraen 10, Undead 11,
Olthoi 12, OlthoiAcid 13. (Olthoi/OlthoiAcid are disabled by default config and return
`Pending`.)

**Response** S2C `CharacterCreateResponse` `0xF643` (UIQueue) —
`GameMessageCharacterCreateResponse.cs:8-19`:
```
u32 status
  ; 0 Undef, 1 Ok, 2 Pending, 3 NameInUse, 4 NameBanned, 5 Corrupt,
  ; 6 DatabaseDown, 7 AdminPrivilegeDenied, 8 Count
if status == 1 (Ok):
    u32 characterGuid
    String16L name
    u32 0
```
Attribute/skill-credit failure → `Corrupt` (5); name taken → `NameInUse` (3); taboo name →
`NameBanned` (4).

---

## 10. World entry, exit, and account ops

* **C2S `CharacterEnterWorldRequest` `0xF7C8`** (no body, `AuthConnected`)
  (`CharacterHandler.cs:183-196`) → S2C **`CharacterEnterWorldServerReady` `0xF7DF`** (no
  body, UIQueue).
* **C2S `CharacterEnterWorld` `0xF657`** (`AuthConnected`)
  (`CharacterHandler.cs:198-263`):
  ```
  u32 characterGuid
  String16L account           ; must == session.Account
  ```
  On success the session moves to `WorldConnected` and the S2C entry sequence fires
  (`WorldManager.DoPlayerEnterWorld` → `Player_Networking.SendSelf`): `GameEvent
  PlayerDescription (0x0013)`, `GameEvent CharacterTitle (0x0029)`, `GameEvent
  FriendsListUpdate (0x0021)`, then `PlayerCreate (0xF746)` + `ObjectCreate (0xF745)` for
  *yourself*, then your inventory/equipped `ObjectCreate`/`ViewContents`, a contract tracker
  table, and several `ServerMessage (0xF7E0)` chat lines.
* **C2S `CharacterLogOff` `0xF653`** (no body, `WorldConnected`)
  (`CharacterHandler.cs:266-270`). The server plays a logout animation; after ~6 s
  (`Session.cs:152-153`) and once off the landblock it sends S2C `CharacterLogOff (0xF653,
  empty body, UIQueue)` + `CharacterList` + `ServerName` and returns the session to
  `AuthConnected` (`Session.cs:250-275`).
* **C2S `CharacterDelete` `0xF655`** (`AuthConnected`): `String16L account` + `u32
  characterSlot`. **C2S `CharacterRestore` `0xF7D9`**: `u32 characterGuid`.
* **`CharacterError` `0xF659`** (UIQueue): `u32 error`. Values:
  Logon 1, AccountLogin 3, ServerCrash1 4, Logoff 5, Delete 6, ServerCrash2 8,
  AccountInvalid 9, AccountDoesntExist 0xA, EnterGameGeneric 0xB, EnterGameStressAccount
  0xC, EnterGameCharacterInWorld 0xD, EnterGamePlayerAccountMissing 0xE,
  EnterGameCharacterNotOwned 0xF, EnterGameCharacterInWorldServer 0x10,
  EnterGameOldCharacter 0x11, EnterGameCorruptCharacter 0x12,
  EnterGameStartServerDown 0x13, EnterGameCouldntPlaceCharacter 0x14,
  LogonServerFull 0x15, EnterGameCharacterLocked 0x17, SubscriptionExpired 0x18.

---

## 11. Radar — positions of *other* players (S2C)

These arrive on the Smartbox/UI queues as other players enter/leave your view. Positions come
from `ObjectCreate` (initial) and `UpdatePosition`/`VectorUpdate` (ongoing). All S2C; the
client only reads them.

### 11.1 `PlayerCreate` `0xF746` (Smartbox) — `GameMessagePlayerCreate.cs:7-10`
```
u32 playerGuid
```

### 11.2 `ObjectCreate` `0xF745` (Smartbox) — `GameMessageCreateObject.cs` →
`WorldObject.SerializeCreateObject` (`WorldObject_Networking.cs:34-404`)
```
u32 guid
<SerializeModelData>        ; 0x11 header, 3 count bytes, packed-dword palette/texture/model lists; model-part ids are raw GfxObj indices (no 0x01 type byte — the client restores it, RETAIL.md §2.2)
<SerializePhysicsData>      ; flag-driven (PhysicsDescriptionFlag); the Position section below holds the location
u32 weenieFlags             ; ObjectDescriptionFlag bitmask
String16L Name
packedDword WeenieClassId
packedDwordOfKnownType IconId (mask 0x6000000)
u32 ItemType
u32 objDescFlags
<Align to 4>
[ u32 weenieFlags2 ]        ; only if weenieFlags & IncludesSecondHeader (0x04000000)
<per-WeenieHeaderFlag fields> ; incl. RadarBlipColor (0x100000) / RadarBehavior (0x800000)
<Align to 4>
```
The `<per-WeenieHeaderFlag fields>` block carries two fields the paper-doll /
equip client reads (`WeenieHeaderFlags.cs:25-26`, `WorldObject_Networking.cs:135-139`
— both a plain `u32`, **not** a Location object):
`ValidLocations` `0x00010000` → `u32` (an `EquipMask` of every slot the item may
be worn in; present whenever the item has any) and `CurrentlyWieldedLocation`
`0x00020000` → `u32` (the slot it is worn in; the flag is **omitted** unless the
item is actually worn — `WielderId` set and the location non-zero,
`WorldObject_Networking.cs:769-770`). The inventory widget (§22) places an item
on the paper doll from `CurrentlyWieldedLocation` and knows its wearable slots
from `ValidLocations`.
`PhysicsDescriptionFlag` (`PhysicsDescriptionFlag.cs`): Position `0x8000`. When set, the
physics block contains a `Position.Serialize` (§11.5) = the object's location. A headless
radar client can extract the position from here, but the far simpler and always-present
source of a moving player's position is `UpdatePosition` (§11.3).

### 11.3 `UpdatePosition` `0xF748` (Smartbox) — `GameMessageUpdatePosition.cs:11-21`
```
u32 guid
<PositionPack>
```
`PositionPack.Write` (`PositionPack.cs:90-115`):
```
u32 flags          ; PositionFlags (below)
u32 cell           ; LandblockId raw (Origin.Write: cellID then Vector3)
f32 x
f32 y
f32 z
[ f32 w ]          ; unless flags & OrientationHasNoW  (0x08)
[ f32 qx ]         ; unless flags & OrientationHasNoX  (0x10)
[ f32 qy ]         ; unless flags & OrientationHasNoY  (0x20)
[ f32 qz ]         ; unless flags & OrientationHasNoZ  (0x40)
[ f32 vx, f32 vy, f32 vz ]   ; if flags & HasVelocity  (0x01)
[ u32 placement ]  ; if flags & HasPlacementID (0x02)
u16 instanceSeq
u16 positionSeq
u16 teleportSeq
u16 forcePositionSeq
```
(`PositionFlags`: HasVelocity 0x01, HasPlacementID 0x02, IsGrounded 0x04, OrientationHasNoW
0x08, OrientationHasNoX 0x10, OrientationHasNoY 0x20, OrientationHasNoZ 0x40.) All four
trailing sequences are `UShortSequence` = 2 bytes each
(`SequenceManager.cs:154-180`, `UShortSequence.cs:43-45`). **This is the primary radar
update: read `guid`, `cell`, `x,y,z`.**

### 11.4 `VectorUpdate` `0xF74E` (Smartbox) — `GameMessageVectorUpdate.cs:11-29`
```
u32 guid
f32 velocity ×3
f32 omega   ×3
u16 instanceSeq
u16 vectorSeq
```

### 11.5 `Position.Serialize` (the 2-arg overload used by CreateObject) —
`Source/ACE.Entity/Position.cs:361-373`
```
u32 cell ; f32 x ; f32 y ; f32 z ; f32 w ; f32 qx ; f32 qy ; f32 qz     ; = 32 bytes
```
(Quaternion is written in **w, x, y, z** order — `AllegianceHierarchy.cs:206-211`; `Vector3`
as **x, y, z** — `AllegianceHierarchy.cs:199-204`.)

### 11.6 `packedDword` — `Source/ACE.Server/Network/Extensions.cs:23-44`
```
if value <= 0x7FFF:            write u16 LE(value)
else:                          write u32 LE( (value << 16) | ((value >> 16) | 0x8000) )
packedDwordOfKnownType(v, t):  if (v & t) != 0: v -= t ; then packedDword(v)
```

### 11.7 `Motion` `0xF74C` (Smartbox) — `GameMessageUpdateMotion.cs:22-27`,
`MovementData.Write` (`MovementData.cs:184-215`)
```
u32 guid
u16 instanceSeq
<MovementData>
```
`MovementData` (for a headless client: read `guid`, then you may **skip** the rest by the
fragment size, or parse):
```
u16 movementSeq
[ u16 serverControlSeq ]       ; always present in this S2C writer
u8  isAutonomous
<Align to 4>
u8  movementType               ; 0 Invalid, 1 MoveToObject, 2 MoveToPosition, 3 TurnToObject, 4 TurnToHeading
u8  motionFlags
u16 currentStyle               ; MotionStance
<union per movementType>        ; e.g. MoveToPosition carries a Position + sequences
```

### 11.8 `ObjectDelete` `0xF747` (Smartbox) — `GameMessageDeleteObject.cs:8-13`
```
u32 guid
u16 instanceSeq
<pad to 4>                      ; total 8 bytes
```

---

## 12. Chat

### 12.1 Local speech (say) — C2S `GameAction Talk`
Fragment data:
```
u32 0xF7B1 ; u32 clientSequence ; u32 0x0015 (Talk) ; String16L text
```
(`GameActionTalk.cs:16-20`.) If `text` starts with `@` it is treated as a server command;
otherwise it is broadcast as speech.

**TalkDirect `0x0032` (talk to an NPC)** — same fragment shape with a
target guid appended:
```
u32 0xF7B1 ; u32 clientSequence ; u32 0x0032 (TalkDirect) ; String16L text ; u32 targetGuid
```
(`GameActionTalkDirect.cs:16-51`.) The target must be a creature in the
current landblock (else `WeenieError CharacterNotAvailable`). The server
first echoes `You tell <name>, "<text>"` to the sender as a `ServerMessage`
(0xF7E0, `ChatMessageType.OutgoingTell`); then, if the target is a *player*,
the line is delivered to it as a `Tell` (0x02BD, subject to gag/squelch),
and if it is a *creature*, the target's `ReceiveTalkDirect` emote set is
executed (so an NPC with a talk script can answer).

### 12.2 `HearSpeech` `0x02BB` (UIQueue) — S2C broadcast — `GameMessageHearSpeech.cs:26-33`
```
String16L text
String16L senderName
u32 senderID
u32 ChatMessageType
```
(`ChatMessageType.Speech = 0x02`.)

### 12.3 `ServerMessage` `0xF7E0` (UIQueue) — S2C — `GameMessageSystemChat.cs:32-37`
```
String16L text
u32 ChatMessageType
```
(Seen values: Broadcast 0, System 5, Help 0xF, WorldBroadcast 0x14, …; full enum in §12.6.)

### 12.4 `TurbineChat` `0xF7DE` (state `WorldConnected`)

**C2S** (send a channel message) — `TurbineChatHandler.cs:25-60`:
```
u32 bytesToFollow
u32 blobType        ; 3 = NETBLOB_REQUEST_BINARY
u32 dispatchType    ; 1 = BYNAME, 2 = BYID
u32 1
u32 0 ×4
u32 bytesToFollow
u32 contextId       ; 1..~0x71
u32 2
u32 2
u32 channelID       ; TurbineChatChannel (below)
packedLen           ; 1 byte if < 0x80, else 2 bytes: first byte has 0x80 set (high 7 bits), second = low byte
<UTF-16LE message>  ; 2 bytes per char
u32 0x0C
u32 senderID        ; your own guid
u32 0
u32 ChatType
```
Note the message here is **UTF-16LE** (the server reads `messageLen * 2` bytes as Unicode,
`TurbineChatHandler.cs:52-55`) — unlike `String16L`, which is single-byte.

**S2C event** (receive a channel message) — `GameMessageTurbineChat.cs:79-114`:
```
u32 size1          ; bytes after this field + 4
u32 blobType       ; 1 = NETBLOB_EVENT_BINARY
u32 dispatchType   ; 1
u32 1
u32 0x000B00B5
u32 1
u32 0x000B00B5
u32 0
u32 size2          ; bytes after this field + 4
u32 channel
packedLen name + <UTF-16LE name>
packedLen msg  + <UTF-16LE msg>
u32 0x0C
u32 senderID
u32 0
u32 chatType
```
**S2C response** (ack of your own message, `blobType = 5`) — `GameMessageTurbineChat.cs:115-136`:
the 12-field prefix above (`size1, 5, dispatch, 1, 0x000B00B5, 1, 0x000B00B5, 0, size2`) then
`u32 channel ; u32 2 ; u32 2 ; u32 0`.

### 12.5 `SetTurbineChatChannels` — GameEvent `0x0295` (UIQueue) —
`GameEventSetTurbineChatChannels.cs:7-20`
```
u32 ×10 channels   ; allegiance, General, Trade, LFG, Roleplay, Olthoi, society, 7, 8, 9
```
Sent to initialize/refresh your channel bindings.

### 12.6 Chat enums
* `ChatType` (`ChatType.cs`): Undef 0, Allegiance 1, General 2, Trade 3, LFG 4, Roleplay 5,
  Society 6, SocietyCelHan 7, SocietyEldWeb 8, SocietyRadBlo 9, Olthoi 10.
* `TurbineChatChannel` (`TurbineChatChannel.cs`): Allegiance 1, General 2, Trade 3, LFG 4,
  Roleplay 5, Society 6, SocietyCelestialHand 7, SocietyEldrytchWeb 8, SocietyRadiantBlood 9,
  Olthoi 10.
* `ChatMessageType` (`ChatMessageType.cs`): Broadcast 0, AllChannels 1, Speech 2, Tell 3,
  OutgoingTell 4, System 5, Combat 6, Magic 7, Channel 8, ChannelSend 9, Social 0xA,
  SocialSend 0xB, Emote 0xC, Advancement 0xD, Abuse 0xE, Help 0xF, Appraisal 0x10,
  Spellcasting 0x11, Allegiance 0x12, Fellowship 0x13, WorldBroadcast 0x14, CombatEnemy
  0x15, CombatSelf 0x16, Recall 0x17, Craft 0x18, Salvaging 0x19.

---

## 13. Keepalive & termination

* **Timeout:** any *valid* C2S packet resets the session timeout — `DefaultAuthTimeout = 15`
  s (`AuthenticationHandler.cs:27`) during auth, otherwise `DefaultSessionTimeout = 60` s
  (`NetworkSettings.cs:33`). The server terminates on timeout
  (`Session.cs:140-146`, `NetworkTimeout`).
* **S2C keepalives the server sends:** an `Ack` (flag `AckSequence` 0x4000, body u32 = last
  received C2S seq) every **2 s** (`timeBetweenAck = 2000`, `NetworkSession.cs:32`); a
  `TimeSync` (flag 0x1000000, f64) every **20 s** (`timeBetweenTimeSync = 20000`,
  `NetworkSession.cs:31`); and, while in `AuthConnected`, a `GameEvent PingResponse (0x01EA)`
  (empty body) every **100 s** (`Session.cs:163-172`).
* **Client keepalive options (either is sufficient):**
  * optional-header `EchoRequest` (flag 0x2000000, body f32 clientTime) → server replies with
    an `EchoResponse` (flag 0x4000000, body f32 + f32) piggybacked on the next S2C; or
  * `GameAction PingRequest` (`0xF7B1 … 0x01E9`, no body) → S2C `GameEvent PingResponse
    (0x01EA)`.
* **Termination:** the server sends an optional final game message — `CharacterError (0xF659)`,
  `AccountBoot (0xF7DC, optional String16L reason)`, or `AccountBanned (0xF7C1, u32
  secondsRemaining + optional String16L reason)` — waits ~2 s, then goes **silent**. It never
  sends an S2C packet with the `Disconnect` flag set (grep-verified). **The client detects
  death by silence** (e.g. no S2C for ~10–15 s) and/or by receiving one of those error
  messages.

`AccountBoot` `0xF7DC` (UIQueue): `[String16L reason]`. `AccountBanned` `0xF7C1` (UIQueue):
`u32 secondsRemaining [; String16L reason]`.

---

## 14. Game message opcode table (subset relevant to a minimal client)

`Source/ACE.Server/Network/GameMessages/GameMessageOpcode.cs`. Direction: S2C unless noted.

| opcode | name | dir | notes |
|--------|------|-----|-------|
| `0x02BB` | HearSpeech | S2C | §12.2 |
| `0x02BC` | HearRangedSpeech | S2C | |
| `0x02CD` | PrivateUpdatePropertyInt | S2C | §21.5 (CoinValue / purse) |
| `0xF643` | CharacterCreateResponse / CharacterRestoreResponse | S2C | §9 |
| `0xF653` | CharacterLogOff | C2S+S2C | §10 |
| `0xF655` | CharacterDelete | C2S | §10 |
| `0xF656` | CharacterCreate | C2S | §9 |
| `0xF657` | CharacterEnterWorld | C2S | §10 |
| `0xF658` | CharacterList | S2C | §8.1 |
| `0xF659` | CharacterError | S2C | §10 |
| `0xF745` | ObjectCreate | S2C | §11.2 |
| `0xF746` | PlayerCreate | S2C | §11.1 |
| `0xF747` | ObjectDelete | S2C | §11.8 |
| `0xF748` | UpdatePosition | S2C | §11.3 |
| `0xF74C` | Motion (MovementEvent) | S2C | §11.7 |
| `0xF74E` | VectorUpdate | S2C | §11.4 |
| `0xF7B0` | GameEvent | S2C | §7 wrapper |
| `0xF7B1` | GameAction | C2S | §7 wrapper |
| `0xF7C1` | AccountBanned | S2C | §13 |
| `0xF7C8` | CharacterEnterWorldRequest | C2S | §10 |
| `0xF7D9` | CharacterRestore | C2S | §10 |
| `0xF7DB` | UpdateObject | S2C | |
| `0xF7DC` | AccountBoot | S2C | §13 |
| `0xF7DE` | TurbineChat | C2S+S2C | §12.4 |
| `0xF7DF` | CharacterEnterWorldServerReady | S2C | §10 |
| `0xF7E0` | ServerMessage | S2C | §12.3 |
| `0xF7E1` | ServerName | S2C | §8.2 |
| `0xF7E5` | DDD_Interrogation | S2C | §8.3 |
| `0xF7E6` | DDD_InterrogationResponse | C2S | optional |

**C2S `GameActionType`** (`GameActionType.cs`), the ones a minimal client uses:
Talk `0x0015`, TalkDirect `0x0032`, Tell `0x005D`, LoginComplete `0x00A1`, IdentifyObject `0x00C8`,
SetCharacterOptions `0x01A1`, Emote `0x01DF`, SoulEmote `0x01E1`, PingRequest `0x01E9`;
inventory: PutItemInContainer `0x0019`, GetAndWieldItem `0x001A`, DropItem `0x001B`,
Use `0x0036`, Buy `0x005F`, Sell `0x0060` (§21.2, §22).
panels: AllegianceUpdateRequest `0x001F`, FellowshipUpdateRequest `0x00A6`,
HouseQuery `0x021E` (§23).

**S2C `GameEventType`** (inside the `0xF7B0` wrapper), the ones a minimal client sees:
PopupString `0x0004`, PlayerDescription `0x0013`, FriendsListUpdate `0x0021` (§23.1),
InventoryPutObjInContainer `0x0022`, CharacterTitle `0x0029` (§23.1),
InventoryServerSaveFailed `0x00A0` (§21.4),
InventoryPutObjectIn3D `0x019A`, IdentifyObjectResponse `0x00C9`, WeenieError `0x028A`,
WeenieErrorWithString `0x028B`, SetTurbineChatChannels `0x0295`, PingResponse `0x01EA`,
ViewContents `0x0196`, Tell `0x02BD`, MagicUpdateEnchantment `0x02C2` (§22.2).
panel events: UpdateTitle `0x002B`, AllegianceUpdate `0x0020`,
AllegianceAllegianceUpdateDone `0x01C8` (ignored), SetSquelchDB `0x01F4`,
HouseData `0x0225`, HouseStatus `0x0226`, FellowshipFullUpdate `0x02BE`,
SendClientContractTrackerTable `0x0314`, SendClientContractTracker `0x0315`
(§23).

---

## 15. `PlayerDescription` GameEvent `0x0013` (your own full state, S2C)

Sent on world entry (UIQueue, may span many fragments). `GameEventPlayerDescription.cs:41-428`.
A headless client can **skip** it (it is only your own stats), but the layout is:
```
u32 propertyFlags        ; written 0 first, back-patched (DescriptionPropertyFlag)
u32 weenieType
if flags & 0x0001:  <HashHdr 64> then count × (u32 propId, i32 value)      ; PropertyInt32
if flags & 0x0080:  <HashHdr 64> then count × (u32 propId, i64 value)      ; PropertyInt64
if flags & 0x0002:  <HashHdr 32> then count × (u32 propId, u32 0/1)         ; PropertyBool
if flags & 0x0004:  <HashHdr 32> then count × (u32 propId, f64 value)       ; PropertyDouble
if flags & 0x0010:  <HashHdr 32> then count × (u32 propId, String16L value) ; PropertyString
if flags & 0x0008:  <HashHdr 32> then count × (u32 propId, u32 value)       ; PropertyDid
if flags & 0x0040:  <HashHdr 32> then count × (u32 propId, u32 value)       ; PropertyIid
if flags & 0x0020:  <HashHdr 16> then 1 × (u32 positionType, <Position flag-form>)
u32 propertyFlags        ; back-patched real value
u32 vectorFlags          ; Attribute|Skill|Spell|Enchantment
u32 hasHealth            ; 0/1
if vectorFlags & Attribute (0x0001):
    u32 attributeFlags
    ; per present attribute (Strength,Endurance,Quickness,Coordination,Focus,Self): u32×3 (ranks, starting, xp)
    ; per present vital (Health,Stamina,Mana): u32×4 (ranks, starting, xp, current)
if vectorFlags & Skill (0x0002):
    <HashHdr 32> then count × (u32 skillId, u32 pointsRaised, u16 1, u32 advClass,
                               u32 xpSpent, u32 initLevel, u32 0, f64 0)
if vectorFlags & Spell (0x0100):
    <HashHdr 64> then count × (u32 spellId, f32 2.0)
if vectorFlags & Enchantment (0x0200):
    u32 registryMask   ; Multiplicative=0x1, Additive=0x2, Vitae=0x4, Cooldown=0x8
    if mask & 0x1: u32 count + count×<Enchantment>
    if mask & 0x2: u32 count + count×<Enchantment>
    if mask & 0x8: u32 count + count×<Enchantment>
    if mask & 0x4: 1×<Enchantment>               ; the single Vitae entry
u32 optionFlags (CharacterOptionDataFlag)
u32 characterOptions1
[ u32 shortcutCount + shortcuts ]
[ SpellLists8: 8 × (u32 count + count×u32 spellId) ]
[ DesiredComps: <HashHdr 256> + entries (u32 compId, u32 qty) ]
u32 spellbookFilters
u32 characterOptions2
[ gameplayOptions ]
u32 inventoryCount ; then (u32 guid, u32 containerType) per non-container, then containers/foci
u32 equippedCount  ; then (u32 guid, u32 wieldLoc, u32 clothingPriority) each
```
`<HashHdr n>` = `PackableHashTable.WriteHeader` = `u16 count ; u16 n`
(`PackableHashTable.cs:14-18`). `<Enchantment>` (`Enchantment.cs:150-169`) =
`u16 spellId ; u16 layer ; u16 spellCategory ; u16 hasSpellSetId ; u32 powerLevel ;
f64 startTime ; f64 duration ; u32 casterGuid ; f32 degradeModifier ; f32 degradeLimit ;
f64 lastTimeDegraded ; u32 statModType ; u32 statModKey ; f32 statModValue ;
[u32 spellSetId if hasSpellSetId]`. The client extracts the Vitae entry's (spell 666)
`statModValue` — the death-penalty multiplier (1.0 = no penalty; 0.95 = 5%), shown as
a line in the You pane. `MagicUpdateEnchantment` (0x02C2) carries a single `<Enchantment>`
(no count) and is sent on death (penalty applied) and on XP reduction.
The flag-form `Position`
(`Position.cs:325-359`) is: `u32 positionFlags ; u32 cell ; f32 x,y,z ; [f32 w unless
OrientationHasNoW] … ; [i32 animFrame if HasPlacementID] ; [f32×3 velocity if HasVelocity]`.

---

## 16. Ambiguities, dead code & notes (no silent guesses)

1. **`WorldLoginRequest` / referral / `ServerSwitch` are dead in this build.** ACE runs login
   and world in one process: `PacketOutboundReferral`/`ServerSwitch` constructors are never
   invoked, and no live `PacketInboundWorldLoginRequest` handler is referenced. The
   `WorldLoginRequest` (0x20000) optional-header field is still *consumed* (8-byte peek +
   rewind, `PacketHeaderOptional.cs:81-87`) but is never sent. A client never needs to send it.
2. **LoginRequest `len`, `Timestamp`, `AuthFlags`, `accountToLoginAs` are not read** by the
   server (`PacketInboundLoginRequest.cs` / `AuthenticationHandler.cs` ignore them). Send
   `0` / the account again.
3. **C2S `GameAction` sequence is not validated** — the source carries an explicit
   `// TODO: verify sequence` (`GameActionPacket.cs:12-14`). Any value works.
4. **S2C `EchoResponse` (0x4000000) is not in the C2S `Unpack` list**, so a client parsing
   *S2C* must skip its 8 bytes (after `TimeSync`). Documented in §2.3.
5. **`DDD_InterrogationResponse` is optional.** The server does not block world entry on it
   (world entry is available in the same `AuthConnected` state). A minimal client may skip it.
6. **Seq-0 C2S packets are always accepted** (the reorder logic special-cases `seq == 0`,
   `NetworkSession.cs:346-352` — "we have several Seq 0 packets during connect"). This is what
   lets LoginRequest/ConnectResponse use seq 0 without advancing the server counter.
7. **Non-ASCII strings:** the server writes Windows-1252 and reads UTF-8 with the length field
   counting *bytes* on write but *characters* on read. They coincide for ASCII. Use ASCII
   only; multi-byte text risks stream misalignment (§3.1).
8. **Order of the first two S2C bundles** (`CharacterList`+`ServerName` vs `DDD_Interrogation`)
   depends on per-`GameMessageGroup` bundle iteration; do not depend on which is seq 2 vs 3.
   Key off opcode.
9. **`ObjectCreate` full parse is large.** For a headless radar, rely on
   `PlayerCreate`/`UpdatePosition`/`VectorUpdate`/`ObjectDelete` for other players' positions
   and treat `ObjectCreate`'s flag-driven tail as "read `guid`, then skip to the next
   message" (the fragment size bounds it).

---

## 17. Minimal-client end-to-end summary

1. Open UDP socket (local port `P`).
2. Send `LoginRequest` → `9000` (seq 0, plain). Body per §5 step 1.
3. Receive `ConnectRequest` from `9000`. Parse `cookie`, `NetID`, `ServerSeed`, `ClientSeed`.
4. Send `ConnectResponse` → `9001` (seq 0, plain, `Id = NetID`) with body `u64 cookie`.
5. Receive `CharacterList` + `ServerName` (+ `DDD_Interrogation`), encrypted. (Optionally
   verify S2C with the `ServerSeed` stream, §4.4/§6.6.)
6. If no character: send `CharacterCreate (0xF656)` per §9; await `CharacterCreateResponse`.
7. Send `CharacterEnterWorldRequest (0xF7C8)` → receive `CharacterEnterWorldServerReady`.
8. Send `CharacterEnterWorld (0xF657)` (`guid` + account). Now in `WorldConnected`.
9. Receive `PlayerDescription`, `PlayerCreate`/`ObjectCreate` (self), chat, and for other
   players `PlayerCreate`/`ObjectCreate`/`UpdatePosition`/`VectorUpdate`/`ObjectDelete` —
   maintain the radar from `UpdatePosition` (guid, cell, x,y,z) and remove on `ObjectDelete`.
10. Chat: send `GameAction Talk` (say); render `HearSpeech`/`ServerMessage`/`TurbineChat`.
11. Keep alive: send `GameAction PingRequest (0x01E9)` (or `EchoRequest`) periodically;
    consume `Ack`/`TimeSync`/`PingResponse` S2C.
12. Log out: send `CharacterLogOff (0xF653)`; await S2C `CharacterLogOff` + `CharacterList` +
    `ServerName` (back in `AuthConnected`), then close.
13. All post-handshake C2S: `Header.Id = NetID`, packet seq `2,3,4,…` (PAKs reuse the current
    value — Step 4), fragment seq `1,2,3,…`, plain (or encrypted) checksum per §4.3. Detect server
    death by silence (§13).

## 18. Trade

Trades are driven by **C2S `GameAction`s** (§7 wrapper `0xF7B1`) that open the window and
move items, and **S2C `GameEvent`s** (§7 wrapper `0xF7B0`, `UIQueue`) that report the
window back to each side. There is **no request→accept handshake**: sending
`OpenTradeNegotiations` opens the window on *both* sides (each receives `RegisterTrade`).

### 18.1 C2S `GameAction`s — opcodes from `GameActionType.cs:90-95`

| Opc    | Action                     | Payload |
| ------ | -------------------------- | ------- |
| `0x01F6` | `OpenTradeNegotiations`  | `u32 partnerGuid` |
| `0x01F7` | `CloseTradeNegotiations` | *(none)* |
| `0x01F8` | `AddToTrade`             | `u32 itemGuid, u32 tradeSlot` |
| `0x01FA` | `AcceptTrade`            | `u32 partnerGuid, f64 stamp, u32 status, u32 initiatorGuid, u32 initiatorAccepts, u32 partnerAccepts` — the server reads all six but only uses `partnerGuid` (`GameActionAcceptTrade.cs:11-17`) |
| `0x01FB` | `DeclineTrade`           | *(none)* |
| `0x0204` | `ResetTrade`             | *(none)* — `whoReset` is derived as the sender's guid (`GameActionResetTrade.cs`) |

### 18.2 S2C `GameEvent`s — opcodes from `GameEventType.cs:52-61`, all `UIQueue`

| Opc    | Event                    | Payload |
| ------ | ------------------------ | ------- |
| `0x01FD` | `RegisterTrade`         | `u32 initiator, u32 partner, i64 0` |
| `0x0200` | `AddToTrade`            | `u32 itemGuid, u32 side, u32 slot(0)` — `side` is `TradeSide`: `1` = Self (your own window), `2` = Partner (what the partner put in) |
| `0x0201` | `RemoveFromTrade`       | Defined in the enum but **no `GameEvent` class and no sender exist in this build** — the server never sends it. Do not wait for it. |
| `0x0202` | `AcceptTrade`           | `u32 whoAccepted` |
| `0x0203` | `DeclineTrade`          | `u32 whoDeclined` |
| `0x0205` | `ResetTrade`            | `u32 whoReset` |
| `0x0207` | `TradeFailure`          | `u32 itemGuid, u32 reason` (`WeenieError`) — e.g. an attuned item → `AttunedItem` `0x0426` (1062); the add simply does not happen |
| `0x01FF` | `CloseTrade`            | `u32 endTradeReason` (`EndTradeReason`) |
| `0x0208` | `ClearTradeAcceptance`  | *(none)* |

Completion is **not** a dedicated event: `FinalizeTrade` sends `WeenieError` (`0x028A`)
with code `TradeComplete` `0x0529` to both sides, then `ResetTrade` to each
(`Player_Trade.cs:267-279`).

### 18.3 Acceptance semantics (why a trading bot re-accepts)

`HandleActionAddToTrade` sets `TradeAccepted = false` on **both** the sender and the
partner (`Player_Trade.cs:121-128`), so adding an item anywhere invalidates every prior
acceptance on both sides. `HandleActionAcceptTrade` finalizes **only if the partner is
already accepted** (`Player_Trade.cs:214-215`). Therefore a "dump my pack and accept"
bot must **re-accept every time it sees the partner accept** (`AcceptTrade` with
`who == partner`); its own acceptance echo (`who == self`) is ignored. Without the
re-accept, whichever side adds items last leaves the trade un-finalized.

### 18.4 `PlayerDescription` item tail — guids only, no names

The end of the `PlayerDescription` body (§15) carries the item lists a headless client
needs to fill a trade:

```
u32 invCount
invCount ×  u32 guid, u32 containerType            ; non-equipped (0 = NonContainer, 1 = Container)
u32 eqpCount
eqpCount ×  u32 guid, u32 wieldLoc, u32 clothingPriority
```

Items have **no names** on the client side, so a trading bot identifies them by guid
only (count/guid-based commands rather than name-based).

---

## 19. Examine (IdentifyObject)

### 19.1 C2S `IdentifyObject` `0x00C8` — §7 wrapper `0xF7B1`

```
u32 0xF7B1 ; u32 clientSequence ; u32 0x00C8 ; u32 objectGuid
```

The server appraises the object in the sender's landblock — creature, player,
item, door, or a ground item — and replies with `IdentifyObjectResponse`
(`GameEventIdentifyObjectResponse.cs`).

### 19.2 S2C `IdentifyObjectResponse` `0x00C9` (UIQueue) — §7 wrapper `0xF7B0`

```
u32 0xF7B0 ; u32 playerGuid ; u32 GameEventSequence ; u32 0x00C9
u32 objectGuid
AppraiseInfo
```

`AppraiseInfo` (`AppraiseInfo.cs:738+`, written by `AppraiseInfoExtensions`):
`u32 flags, u32 success (1/0)`, then flag-guarded sections in a **fixed write
order** — a flag off means the section is absent from the stream, so the
reader must advance the cursor per flag:

| flag     | section        | layout |
| -------- | -------------- | ------ |
| `0x0001` | IntStats       | hash table: `u16 count, u16 buckets, count × (u32 PropertyInt, i32)` |
| `0x2000` | Int64Stats     | same, value `i64` |
| `0x0002` | BoolStats      | same, value `u32` (`Convert.ToUInt32(bool)`, `AppraiseInfo.cs:831`) |
| `0x0004` | FloatStats     | same, value `f64` |
| `0x0008` | StringStats    | `u16 count, u16 buckets, count × (u32 PropertyString, String16L)` |
| `0x1000` | DidStats       | same as IntStats, value `u32` |
| `0x0010` | SpellBook      | `u32 count, count × u32 spellId` |
| `0x0080` | ArmorProfile   | 8 × `f32` resistances: slash, pierce, bludgeon, cold, fire, acid, nether, lightning |
| `0x0100` | CreatureProfile | `u32 cflags, u32 health, u32 healthMax`, then `[cflags & 0x8] 10 × u32` (strength, endurance, quickness, coordination, focus, self, stamina, mana, staminaMax, manaMax), then `[cflags & 0x1] 2 × u16` (attribute highlight/color) |
| `0x0020` | WeaponProfile  | 4 × `u32` (damageType, weaponTime, weaponSkill, damage) + 5 × `f64` (damageVariance, damageMod, weaponLength, maxVelocity, weaponOffense) + `u32 maxVelocityEstimated` — 60 bytes |
| `0x0040` | HookProfile    | 3 × `u32` (flags, validLocations, ammoType) |
| `0x0200` | ArmorEnch      | 2 × `u16` (highlight, color) |
| `0x0800` | WeaponEnch     | 2 × `u16` |
| `0x0400` | ResistEnch     | 2 × `u16` |
| `0x4000` | ArmorLevels    | 9 × `u32` per-body-part armor (Head..Foot) |

`success` is `0` when the object cannot be appraised (no sections follow).
Verified byte-exact against live captures (creature 240B, player 240B, item
228B, door, ground item).

Property IDs used by the TUI examine widget (`PropertyInt.cs` /
`PropertyString.cs`): Int `5` Encumbrance, `19` Value, `25` Level, `33` Bonded,
`114` Attuned; String `5` Template, `14` Use, `15` ShortDescription,
`16` LongDescription. The description line is LongDesc ?? ShortDesc ?? Use ??
Template (first non-empty).

## 20. Movement

Movement is **client-authoritative** on this server build. The client runs its
own physics simulator (`src/movement.ts`, constants from the decompiled retail
client — see `docs/RETAIL.md` §3) and ships the resulting position/orientation;
the server does **not** integrate velocities. It adopts what the client says and
broadcasts it:

- `GameActionMoveToState` → `SetRequestedLocation(...)` + `BroadcastMovement(...)`
  unconditionally (`GameActionMoveToState.cs`).
- `GameActionAutonomousPosition` → `SetRequestedLocation(...)` and, when the
  contact bit is set, `LastGroundPos = position` (`GameActionAutonomousPosition.cs`).

So the local sim **is** the position. The server's only push-back is a correction
(collision, bounds, or a forced/teleport move), which the client detects and
snaps to (§20.7).

### 20.1 Shared sub-structures

**Position (32 B)** — used by all three C2S actions:

```
u32 cell            ; landblock id (0 = "unspecified", adopt current)
f32 x, y, z
f32 qw, qx, qy, qz  ; orientation quaternion
```

**RawMotionState (48 B here)** — `RawMotionState.cs`. First a `u32 PackedFlags`:
**bits 0–10** are field-present flags, **bits 11+** are the motion-command list
length. The eleven optional fields follow in this **fixed order**, each present
only if its flag bit is set:

| bit    | field            | type |
| ------ | ---------------- | ---- |
| `0x1`  | `currentHoldKey` | u32 (`HoldKey`) |
| `0x2`  | `currentStyle`   | u32 (`MotionStance`) |
| `0x4`  | `forwardCommand` | u32 (`MotionCommand`) |
| `0x8`  | `forwardHoldKey` | u32 (`HoldKey`) |
| `0x10` | `forwardSpeed`   | f32 |
| `0x20` | `sidestepCommand`| u32 (`MotionCommand`) |
| `0x40` | `sidestepHoldKey`| u32 (`HoldKey`) |
| `0x80` | `sidestepSpeed`  | f32 |
| `0x100`| `turnCommand`    | u32 (`MotionCommand`) |
| `0x200`| `turnHoldKey`    | u32 (`HoldKey`) |
| `0x400`| `turnSpeed`      | f32 |

The client sends **all eleven fields** (`PackedFlags = 0x7ff`, command list
length **0**). This is required: the server's C# field defaults are 0/0.0 (not
the retail defaults) and `apply_raw_movement` copies raw → interpreted
verbatim, so an omitted speed field reads 0.0 and kills the motion.

**Command list: always length 0.** A non-`SoulEmote` command-list entry makes the
server log an error (`RawMotionState.cs`). Locomotion is fully described by the
command/hold-key/speed fields above; the list is reserved for emotes.

### 20.2 C2S `MoveToState` `0xF61C` — §7 wrapper `0xF7B1`

Sent on every **motion-state change** (key pressed/released, walk↔run, air
↔ ground). Body is **89 B** = RawMotionState + Position + tail:

```
u32 0xF7B1 ; u32 clientSeq ; u32 0xF61C
u32  PackedFlags (=0x7ff)        ; RawMotionState header
u32  currentHoldKey              ; running ? 2 : 1
u32  currentStyle                ; 0x8000003d (NonCombat)
u32  forwardCommand              ; e.g. WalkForward
u32  forwardHoldKey
f32  forwardSpeed                ; always 1.0
u32  sidestepCommand
u32  sidestepHoldKey
f32  sidestepSpeed               ; always 1.0
u32  turnCommand
u32  turnHoldKey
f32  turnSpeed                   ; always 1.0
u32  cell ; f32 x,y,z ; f32 qw,qx,qy,qz   ; Position (32 B)
u16  instanceSequence        (=0)
u16  serverControlSequence   (=0)
u16  teleportSequence        (=0)
u16  forcePositionSequence   (=0)
u8   contactLongJump         ; bit0 contact, bit1 longJump
; server reader.Align() — no-op at packet end (MoveToState.cs)
```

### 20.3 C2S `AutonomousPosition` `0xF753` — §7 wrapper `0xF7B1`

Sent **at most once per second while moving** (retail rate;
`GameActionAutonomousPosition.cs` comment). Body is **41 B** = Position + tail:

```
u32 0xF7B1 ; u32 clientSeq ; u32 0xF753
u32  cell ; f32 x,y,z ; f32 qw,qx,qy,qz   ; Position (32 B)
u16  instanceTimestamp     (=0)
u16  serverControlTimestamp (=0)
u16  teleportTimestamp      (=0)
u16  forcePositionTimestamp (=0)
u8   contact               ; 1 = on ground
; server reader.Align() — no-op at packet end
```

### 20.4 C2S `Jump` `0xF61B` — §7 wrapper `0xF7B1`

Body is **32 B**. This is the server's `JumpPack` layout (not the retail
56-byte form) — `GameActionJump.cs` / `JumpPack`:

```
u32 0xF7B1 ; u32 clientSeq ; u32 0xF61B
f32  extent
f32  vx, vy, vz
u16  ×4   (=0)
u32  objectGuid  (=0)
u32  spellId     (=0)
```

`vx, vy, vz` are the horizontal + vertical takeoff velocities the sim computes
from the character's Jump skill (see `docs/RETAIL.md` §3.4). The server
records the request; the vertical arc itself is dead-reckoned locally.

### 20.5 Enums

**HoldKey** (`0x1` walk/run axis): `Invalid = 0`, `None = 1`, `Run = 2`.

**MotionCommand** (values used): `Invalid = 0`, `Ready = 0x41000003`,
`WalkForward = 0x45000005`, `WalkBackwards = 0x45000006`,
`RunForward = 0x44000007`, `TurnRight = 0x6500000d`, `TurnLeft = 0x6500000e`,
`SideStepRight = 0x6500000f`, `SideStepLeft = 0x65000010`.

**MotionStance**: `NonCombat = 0x8000003d` (always sent as `currentStyle`).

**Run encoding.** "Running" is not a separate forward command on the wire — it is
sent as `forwardCommand = WalkForward` + `forwardHoldKey = Run(2)`. The server's
`adjust_motion` (in `MotionInterp.cs`) converts that to `RunForward × R`, and
applies `RunTurnFactor = 1.5` to turns. The local sim mirrors this exactly, so
local and server agree without the client ever sending `RunForward`.

### 20.6 S2C corrections

The server never streams a position to the client on its own while the client is
simulating — it *echoes* the position we sent via the self `UpdatePosition`.
That echo **lags the live sim while moving**, so a naive snap would jerk the
character backward. Two S2C messages matter:

**`UpdatePosition` `0xF748`** (SecureWeenieQueue) — `[guid, cell, x, y, z, quat]`,
parsed by `M.parseUpdatePosition`. For our own guid this is the echo/correction
source (§20.7).

**`PlayerTeleport` `0xF751`** (SmartboxQueue) — `GameMessagePlayerTeleport.cs`
writes a **u32** teleport sequence (`Sequences.GetNextSequence(ObjectTeleport)`)
then `Writer.Align()`. The new position is *not* in this packet; it arrives in the
subsequent self `UpdatePosition` / `ObjectCreate`.

### 20.7 Client correction policy

`session.ts` (`_selfPositionUpdate`) classifies each self `UpdatePosition`:

- **Teleport pending** (a `PlayerTeleport` was seen) → snap unconditionally.
- else if `|received − lastSent| > MOVE_SNAP_EPS` (**0.25 u**) → it is a **server
  correction** (collision/bounds/forced move) → snap the sim.
- else → it is our own **echo** → ignore the position (adopt the landblock
  `cell` only).

After any snap the motion signature is cleared to force a fresh `MoveToState`.

### 20.8 Send cadence

All timings live in `session.ts`:

| constant | value | role |
| -------- | ----- | ---- |
| `MOVE_TICK_MS` | 50 | sim integration step |
| `AUTOPOS_INTERVAL_MS` | 1000 | max rate of `AutonomousPosition` while moving |
| `MOVE_RELEASE_MS` | 120 | a held direction is expired if not re-asserted within this (raw terminals emit keydown + auto-repeat but no keyup) |
| `MOVE_SNAP_EPS` | 0.25 | echo-vs-correction threshold (u) |

`MoveToState` goes out on every motion-state change (an 8-bit input signature
covering the six directions, run, and airborne). `AutonomousPosition` throttles
to ~1/s while moving. Jump sets the airborne flag on the next `MoveToState`.

## 21. Vendor (shop)

Vendor interactions are driven by **C2S `GameAction`s** (§7 wrapper `0xF7B1`) that
open the shop and move items, and **S2C `GameEvent`s** (§7 wrapper `0xF7B0`, `UIQueue`)
that report the shop stock back. Opening the shop is just a `Use` on the vendor —
the server auto-walks the character to the vendor and answers with the
`ApproachVendor` event carrying the full stock. There is **no separate
"open shop" action**, and no request→accept handshake.

### 21.1 S2C `ApproachVendor` `0x0062` (UIQueue) — `GameEventApproachVendor.cs:11-64`

Sent on shop open, and **re-sent after every successful buy**
(`FinalizeBuyTransaction` → `vendor.ApproachVendor`, `Player_Commerce.cs:112`) to
refresh the stock (sold-out unique items) and the player's alt-currency balance.
It is **not** re-sent after a sell.

Body after the GameEvent prefix (`u32 playerGuid ; u32 GameEventSequence ; u32 0x0062`):

```
u32 vendorGuid
u32 merchandiseItemTypes   ; ItemType bitmask of what the vendor will buy back
u32 merchandiseMinValue
u32 merchandiseMaxValue
u32 dealMagicalItems       ; 0/1
f32 buyPrice               ; rate the vendor PAYS when you sell (× item value)
f32 sellPrice              ; rate the vendor CHARGES when you buy (× item value)
u32 altCurrencyWcid        ; 0 = pyreals
[ altCurrencyWcid ≠ 0:  u32 altCurrencyCount + String16L pluralName
 | altCurrencyWcid == 0: u32 0 + String16L "" ]
u32 numItems
numItems × (u32 prefix + SerializeGameDataOnly)
; prefix = (stackSize & 0xFFFFFF) | (0xFF << 24); 0xFFFFFF = unlimited supply
; SerializeGameDataOnly = u32 itemGuid + the object-description tail — the same
;   field sequence as ObjectCreate (§11.2) starting at `u32 weenieFlags`, minus
;   the Model/Physics blocks. Pitfall: the `AmmoType` u16 (WeenieHeaderFlag
;   0x00000100) sits between ContainersCapacity and Value — skipping it
;   misaligns Value and every field after it.
; 4-byte aligned at the end
```

The per-item `itemGuid` is the vendor item's **instance guid** — this is the
`objectId` to send back in the Buy action. The tail carries name / itemtype /
value, so the client can label the stock with no extra messages.

### 21.2 C2S `GameAction`s — opcodes from `GameActionType.cs:28,51-52`

| Opc    | Action | Payload |
| ------ | ------ | ------- |
| `0x0036` | `Use`  | `u32 targetGuid` — on a vendor: the server auto-walks and answers with S2C `ApproachVendor` (`Vendor.ActOnUse`) |
| `0x005F` | `Buy`  | `u32 vendorGuid, u32 numItems, numItems × (i32 amount, u32 objectId)` — `objectId` = the vendor item's instance guid from `ApproachVendor` (`GameActionBuyItems.cs:12-27`) |
| `0x0060` | `Sell` | `u32 vendorGuid, u32 numItems, numItems × (i32 amount, u32 objectGuid)` — `objectGuid` = the player's own item guid (`GameActionSellItems.cs:12-24`) |

### 21.3 Prices — the exact server math, `Vendor.cs:577-609`

* **Buy** — the player pays `GetSellCost` per item:
  `max(1, ceil(sellPrice × item.value − 0.1))` pyreals (PromissoryNote: fixed rate 1.15).
* **Sell** — the player receives `GetBuyCost` per item:
  `max(1, floor(buyPrice × item.value + 0.1))` pyreals (PromissoryNote: fixed rate 1.0).

`buyPrice` / `sellPrice` are the vendor's rates from `ApproachVendor`: `sellPrice`
is what the vendor **charges** (buying from it), `buyPrice` what it **pays**
(selling to it).

### 21.4 Validation and result signals

* **Buy validation** (`Vendor.BuyItems_ValidateTransaction`): any single invalid
  item fails the **entire** transaction, and the only failure signal is
  `GameEventInventoryServerSaveFailed` (GameEvent `0x00A0`, no body) — there is no error text.
  unfunded character (0 pyreals) fails this way, indistinguishably. On success
  the item arrives as an `ObjectCreate` (§11.2) in the player's main pack, and
  the shop re-opens (`ApproachVendor`).
* **Sell validation** (`Player_Commerce.VerifySellItems`): invalid items are
  **skipped individually** (logged server-side) and the valid ones proceed; an
  empty remainder fails with `GameEventInventoryServerSaveFailed` (`0x00A0`).
* **A sold item never gets an `ObjectDelete`.** `TryRemoveFromInventoryWithNetworking`
  (`Player_Inventory.cs:217-250`) skips every remove/delete message for
  `RemoveFromInventoryAction.SellItem`. The **only** client-side signal that the
  item left the player is the S2C `InventoryPutObjInContainer` (GameEvent
  `0x0022`, `GameEventItemServerSaysContainId.cs:7-14`):

  ```
  u32 itemGuid
  u32 containerGuid      ; = the vendor's guid on a sell
  u32 placementPosition
  u32 itemContainerType
  ```

  The same event is used for pack moves, so a client should only drop the item
  from its inventory when the new container is neither the player nor a
  player-owned pack. Wire order for a sell: `0x0022` for each sold item first,
  then the payout coin stack as a separate `ObjectCreate`.

### 21.5 Purse (CoinValue) tracking

The initial pyreal balance is the `PlayerDescription` Int32 property table
entry pid 20 = `PropertyInt.CoinValue` (§15). Live updates ride **S2C
`PrivateUpdatePropertyInt`** (opcode `0x02CD`, `GameMessagePrivateUpdatePropertyInt.cs`):

```
u8  seq              ; 1-byte ByteSequence (Sequences.GetNextSequence) — NOT a u32
u32 property         ; 20 = CoinValue (also 5 = EncumbranceVal, 125 = Age, …)
u32 value            ; the new value
```

The body is 9 bytes: the sequence is a 1-byte `ByteSequence`, so reading it as
a u32 overruns the body and the update is dropped. The server emits this opcode
for any `PropertyInt` change, but for the purse (property 20) it only sends it
on a **split-stack buy** (the payment splits a coin stack, and `SpendCurrency`
calls `UpdateCoinValue`). A **sell** and a **whole-stack buy** send no CoinValue
update — the coin itself is the signal: the purse is the sum of the `Pyreal`
coin stacks (ItemType 64) in the player's inventory. The client therefore keeps
a coin ledger: a sell's new stack (an `ObjectCreate` whose guid is absent from
the `PlayerDescription` baseline) is added to the balance, and a whole-stack
buy's removed stack (an `ObjectDelete`) is subtracted. An alt-currency vendor's
count is not a player property: it is re-sent inside `ApproachVendor` after each
buy (§21.1).

## 22. Inventory (packs, equip, drop)

Inventory manipulation is driven by **C2S `GameAction`s** (the `0xF7B1` wrapper,
§7). The server keeps the client in sync with a mix of **S2C `GameEvent`s** and
**re-streamed `ObjectCreate`s** (§11.2) — there is no dedicated "inventory
changed" event.

### 22.1 C2S `GameAction`s — opcodes from `GameActionType.cs`

| Opc    | Action               | Payload |
| ------ | -------------------- | ------- |
| `0x0019` | `PutItemInContainer` | `u32 itemGuid, u32 containerGuid, i32 placement` — `containerGuid` = the player's **own** guid **unequips** the item to the main pack; a **pack's** guid **moves** the item into that pack; a **ground pickup** is the same action with `containerGuid` = the player (`GameActionPutItemInContainer.cs:9-11`) |
| `0x001A` | `GetAndWieldItem`    | `u32 itemGuid, i32 location` — `location` is an `EquipMask` bit. The server rejects if `location ∉ item.ValidLocations` (`Player_Inventory.cs:1747-1757`) or if the slot is taken (`WieldedLocationIsAvailable`, `Creature_Equipment.cs:50,318` — for `Clothing` the conflict is a `ClothingPriority` clash, not a bit overlap, `Creature_Equipment.cs:94-104`); there is **no auto-swap** to another slot. For clothing the requested slot is replaced by the item's full `ValidLocations` before the check (`Player_Inventory.cs:1791`), so the client picks the first **free** valid bit itself (`GameActionGetAndWieldItem.cs:10-11`) |
| `0x001B` | `DropItem`           | `u32 itemGuid` — drops the item at the player's feet (`GameActionDropItem.cs:10`) |
| `0x0036` | `Use`                | `u32 itemGuid` — works on inventory items (food, a mana stone, …) as well as world objects |

`EquipMask` (`EquipMask.cs`), the bits sent as `location`:
HeadWear `0x1`, ChestWear `0x2`, AbdomenWear `0x4`, UpperArmWear `0x8`,
LowerArmWear `0x10`, HandWear `0x20`, UpperLegWear `0x40`, LowerLegWear `0x80`,
FootWear `0x100`, ChestArmor `0x200`, AbdomenArmor `0x400`, UpperArmArmor `0x800`,
LowerArmArmor `0x1000`, UpperLegArmor `0x2000`, LowerLegArmor `0x4000`,
NeckWear `0x8000`, WristWearLeft `0x10000`, WristWearRight `0x20000`,
FingerWearLeft `0x40000`, FingerWearRight `0x80000`, MeleeWeapon `0x100000`,
Shield `0x200000`, MissileWeapon `0x400000`, MissileAmmo `0x800000`,
Held `0x1000000`, TwoHanded `0x2000000`, TrinketOne `0x4000000`,
Cloak `0x8000000`, SigilOne `0x10000000`, SigilTwo `0x20000000`,
SigilThree `0x40000000`.

### 22.2 S2C signals that keep the client in sync

* **Equip** (`GetAndWieldItem`): the self-facing signal is the S2C
  `WieldObject` GameEvent `0x0023` (`GameEventWieldItem.cs:7-12`, UIQueue):

  ```
  u32 itemGuid
  u32 location
  ```

  (`Player_Inventory.cs:297-305`, `TryEquipObjectWithNetworking`.) The
  `ObjectCreate` re-stream (`TrackEquippedObject`, `Player_Tracking.cs:116-129`)
  is sent **only to other players** — it early-returns when the wielder is the
  local player — so a self-equip is never visible as an `ObjectCreate`. The
  client records `wielder = self`, `curLoc = location` on `0x0023` and places
  the item on the paper doll. Moving an already-worn item to another slot also
  sends `0x0023` (`Player_Inventory.cs:1819`).
* **Unequip / move / pickup** (`PutItemInContainer`): the server sends
  `GameMessagePublicUpdateInstanceID` (`Container` → the new guid) and the S2C
  `InventoryPutObjInContainer` GameEvent `0x0022` (UIQueue,
  `GameEventItemServerSaysContainId.cs:7-14`):

  ```
  u32 itemGuid
  u32 containerGuid
  u32 placementPosition
  u32 itemContainerType
  ```

  (`Player_Inventory.cs:1358-1360`.) An unequip is just a move to the player's
  own guid; a pickup is a move from the ground to the player. The client
  refreshes the item's recorded container from `containerGuid`, and only drops
  the item from its inventory when the new container is neither the player nor
  a player-owned pack (a sell, §21.4).
* **Drop** (`DropItem`): the server sends **no `ObjectDelete`**. It sends
  `GameEventItemServerSaysMoveItem` = GameEventType `InventoryPutObjectIn3D`
  `0x019A` (UIQueue, `GameEventItemServerSaysMoveItem.cs:8-11`), body just
  `u32 itemGuid`, then a positioned ground `ObjectCreate`
  (`Player_Inventory.cs:1440-1443`; `TryDropItem`
  `Player_Inventory.cs:1472-1514` adds the item to the landblock). The client
  removes the item from its inventory model on `0x019A`; the ground
  `ObjectCreate` repopulates the world view and makes the item a targetable
  ground entity.

### 22.3 The `ObjectCreate` tail fields the client reads

`SerializeCreateObject` (§11.2, `WorldObject_Networking.cs:135-139`) carries,
per `WeenieHeaderFlag`:
`ValidLocations` `0x00010000` → `u32` (an `EquipMask` of every slot the item
may wear) and `CurrentlyWieldedLocation` `0x00020000` → `u32` (the slot it is
worn in). Both are plain `u32`s — **not** Location objects (the `// Location`
comment in `WeenieHeaderFlags.cs:26` is misleading). The
`CurrentlyWieldedLocation` flag is **omitted** unless the item is actually worn
(`WielderId` set + non-zero location, `WorldObject_Networking.cs:769-770`).
The paper doll places an item from `CurrentlyWieldedLocation` (the first
matching `EquipMask` bit in the slot map wins, so a multi-bit robe that spans
the shirt and pants cells lands on the shirt; the shirt, pants, and weapon
cells each accept several bits).
The `StackSize` flag `0x00001000` → `u16` (the current stack count;
`MaxStackSize` `0x00002000` → `u16` follows it) — the vendor Sell tab sells
whole stacks with it, and the inventory / vendor lists show ` xN` for stacks.

## 23. Panel messages (social / status / housing)

The right-column multi widgets (`src/ui.tsx`) are fed by six login pushes and
three on-demand C2S/S2C pairs. Everything here rides the §7 `0xF7B0` (S2C) /
`0xF7B1` (C2S) wrapper.

### 23.1 Login pushes (S2C, UIQueue)

* **`CharacterTitle` `0x0029`** (`GameEventCharacterTitle.cs`) — at world
  entry: `u32 (1)`, `u32 currentTitleId` (0 = none), `u32 count`, then
  `u32 × count` owned title ids (ids are the 0-based sequential
  `ACE.Server/Enum/CharacterTitle` values).
* **`UpdateTitle` `0x002B`** (`GameEventUpdateTitle.cs`) — when a title is
  earned or the display title changes: `u32 titleId`,
  `u32 setAsDisplayTitle` (0/1).
* **`FriendsListUpdate` `0x0021`** (`GameEventFriendsListUpdate.cs`) — at
  world entry (the full list) and on every friend change:
  `u32 count`, then per friend `u32 id`, `u32 online` (0/1),
  `u32 appearOffline` (0), `String16L name`, `u32 (0)`, `u32 (0)`; finally
  `u32 updateType` (`FriendsUpdateTypeFlag`: 0 full list, 1 added, 2 removed,
  4 status changed).
* **`SetSquelchDB` `0x01F4`** — at world entry **only when non-empty**
  (no event = empty): `u16 (0) u16 (0)` (the account squelch table, always
  empty), then `u16 count u16 (32)`, per entry `u32 id` + `<SquelchInfo>`,
  and one trailing global `<SquelchInfo>`.
  `<SquelchInfo>` = `u32 nFilters`, `u32 × nFilters` (`SquelchMask`),
  `String16L name`, `u32 accountSquelch` (0/1).
* **`SendClientContractTrackerTable` `0x0314`** — at world entry:
  `u16 count u16 (32)`, per entry `u32 id`, `u32 version`, `u32 id` (again —
  the `ContractTracker` nests the id), `u32 stage` (`ContractStage`: 0 "",
  1 Available, 2 InProgress, 3 DoneOrPending, 4 Counter),
  `f64 timeWhenDone`, `f64 timeWhenRepeats`.
* **`SendClientContractTracker` `0x0315`** — single-contract push:
  `u32 version`, `u32 id`, `u32 stage`, `f64 timeWhenDone`,
  `f64 timeWhenRepeats`, `u32 deleteContract` (0/1),
  `u32 setAsDisplayContract` (deprecated, 0).

### 23.2 On-demand panel requests (C2S `GameAction`, §7 wrapper)

The retail panels fetch their data on open; the client does the same
(`src/ui.tsx` sends the request when the widget is selected):

| GameAction | opcode | payload | reply |
| --- | --- | --- | --- |
| `AllegianceUpdateRequest` | `0x001F` | `u32 panelOpen` (0/1) | always `AllegianceUpdate` (an empty profile when not in one), then `AllegianceAllegianceUpdateDone` `0x01C8` (ignored) |
| `FellowshipUpdateRequest` | `0x00A6` | `u32 panelOpen` (0/1) | `FellowshipFullUpdate` **only when in a fellowship** (silence otherwise) |
| `HouseQuery` | `0x021E` | (none) | `HouseData` when the player owns a house, else `HouseStatus` (a non-zero `u32` weenie error) |

### 23.3 On-demand panel responses (S2C, UIQueue)

* **`AllegianceUpdate` `0x0020`** (`GameEventAllegianceUpdate.cs`,
  `AllegianceHierarchy.cs`):
  ```
  u32 rank, u32 totalMembers, u32 totalVassals
  u16 recordCount, u16 oldVersion (0xB)
  u16 officerCount (0), u16 (256)
  u32 officerTitlesCount (0)  [+ that many String16L]
  u32 monarchBroadcastTime, u32 monarchBroadcastsToday,
  u32 spokesBroadcastTime, u32 spokesBroadcastsToday
  String16L motd, String16L motdSetBy
  u32 chatRoomID
  u32 cell, f32 × 3 (Pos), f32 × 4 (Rotation)      ; the bindpoint Position
  String16L allegianceName
  u32 nameLastSetTime, u32 isLocked (0/1), i32 approvedVassal
  if recordCount > 0: <AllegianceData>              ; the monarch, no parent prefix
  if recordCount > 1: (recordCount-1) × (u32 parentGUID, <AllegianceData>)
  ```
  `<AllegianceData>`: `u32 guid`, `u32 cpCached`, `u32 cpTithed`,
  `u32 bitfield` (`AllegianceIndex`: 0x1 LoggedIn, 0x2 Update, 0x4
  HasAllegianceAge, 0x8 HasPackedLevel, 0x10 MayPassupExperience),
  `u8 gender`, `u8 heritage`, `u16 rank`,
  [bitfield & 0x8 → `u32 level`], `u16 loyalty`, `u16 leadership`,
  [bitfield & 0x4 → `u32 timeOnline`, `u32 allegianceAge`; else the
  deprecated `u64 timeOnline` as 8 bytes], `String16L name`.
  The record list writes **no count** (the count write is commented out in
  `AllegianceHierarchy.cs`); `recordCount = 0` (a fresh character) is just the
  header with no `<AllegianceData>`.

* **`FellowshipFullUpdate` `0x02BE`** (`GameEventFellowshipFullUpdate.cs`):
  ```
  u16 count, u16 (16)
  per fellow: u32 guid, u32 cpCached (0), u32 lumCached (0), u32 level,
              u32 hMax, u32 sMax, u32 mMax, u32 hCur, u32 sCur, u32 mCur,
              u32 shareLoot, String16L name
  String16L fellowshipName
  u32 leaderGuid
  u32 shareXP, u32 evenShare, u32 open, u32 isLocked    (each 0/1)
  u16 depCount, u16 (32), depCount × (u32 guid, i32 timeDeparted)
  u16 lockCount, u16 (32), lockCount × (String16L key, u32 × 5)
  ```

* **`HouseData` `0x0225`** (`HouseProfile.cs`) — when the player owns a
  house:
  ```
  u32 DwellingID, u32 OwnerID, u32 Bitmask
  i32 MinLevel, i32 MaxLevel, i32 MinAllegRank, i32 MaxAllegRank
  u32 MaintenanceFree (0/1), u32 Type (1 cottage, 2 villa, 3 mansion, 4 apartment)
  String16L OwnerName
  <HousePaymentList> Buy, <HousePaymentList> Rent
  ```
  `<HousePaymentList>` = `u32 count`, count × `<HousePayment>`;
  `<HousePayment>` = `i32 Num`, `i32 Paid`, `u32 WeenieID`,
  `String16L Name`, `String16L PluralName`.

* **`HouseStatus` `0x0226`** — when the player does **not** own a house:
  `u32 weenieError` (non-zero).

---

## 24. Containers (chests / corpses, loot, locks and keys)

Ground containers — **chests** and **corpses** (`ItemType` 512, `WeenieType`
20 / 14) — open, loot, and re-lock over the existing `Use` / `UseWithTarget` /
`PutItemInContainer` actions plus three `GameEvent`s. Doors (`ItemType` 128,
`WeenieType` 19) share the lock path. Keys and lockpicks are both `ItemType`
16384 (`WeenieType` 22 / 23). `ItemType` comes from `PropertyInt.ItemType`
(`= 1`) in the weenie, written into the `ObjectCreate` stream.

### 24.1 C2S `GameAction`s — §7 wrapper `0xF7B1`, opcodes from `GameActionType.cs`

| Opc    | Action                     | Payload |
| ------ | -------------------------- | ------- |
| `0x0036` | `Use`                    | `u32 targetGuid` — open a container (or a door, or use an NPC); a **locked** container answers "The <name> is locked." instead of opening |
| `0x0035` | `UseWithTarget`          | `u32 srcGuid, u32 targetGuid` — a key / lockpick (`ItemType` 16384) on a locked container / door runs the unlock check (`Lock.cs` `UseUnlocker`) |
| `0x0019` | `PutItemInContainer`     | `u32 itemGuid, u32 containerGuid, u32 placement` — **loot** is this with `containerGuid = self`; a **put-back** is this with `containerGuid = the container` |
| `0x0195` | `NoLongerViewingContents` | `u32 containerGuid` — close the loot window (`GameActionNoLongerViewingContents.cs` reads the guid and calls `HandleActionNoLongerViewingContents`) |

### 24.2 S2C `GameEvent`s — §7 wrapper `0xF7B0`, all `UIQueue`, opcodes from `GameEventType.cs`

| Opc    | Event                      | Payload (verified against the server writers) |
| ------ | -------------------------- | --------------------------------------------- |
| `0x0196` | `ViewContents` (`GameEventViewContents.cs`) | `u32 containerGuid, u32 count`, then `count × (u32 itemGuid, u32 containerType)` where `containerType` is `ContainerType` (`0` NonContainer / `1` Container / `2` Foci) — a container opened; the per-item `ObjectCreate` stream (each carrying the container's guid as `ContainerId`) follows, then `UseDone` |
| `0x0052` | `CloseGroundContainer` (`GameEventCloseGroundContainer.cs`) | `u32 containerGuid` — the container closed (you stopped viewing it, or it reset) |
| `0x01c7` | `UseDone` (`GameEventUseDone.cs`) | `u32 weenieError` (`0` = `WeenieError.None`) — a `Use` / `UseWithTarget` finished |
| `0x0022` | `InventoryPutObjInContainer` | `u32 itemGuid, u32 containerGuid, u32 placement, u32 containerType` — the **only** signal for a loot / put-back move (the server sends no `ObjectCreate` re-stream for a container move, so the client derives the loot window from these `containerId` changes) |

`ViewContents` opens the client's loot box (`openContainer = containerGuid`),
`CloseGroundContainer` closes it, and a non-zero `UseDone` is surfaced as a
system line only when the server does **not** also send a `ServerMessage`
(§12.3) for the same action (see 24.3).

### 24.3 Locks, keys, lockpicks

Unlock = `UseWithTarget` (key / unlocker → lock), handled server-side by
`Lock.cs` `UseUnlocker` + `ConsumeUnlocker`. The outcomes:

* **Success** (key or lockpick): `ServerMessage` `0xF7E0` system chat —
  "The <target> has been unlocked." (key) / "You have successfully picked the
  lock! It is now unlocked." (lockpick) — plus the consumption text and a
  `UseDone` `0`.
* **Pick failure** (lockpick): `ServerMessage` "You have failed to pick the
  lock. It is still locked." + consumption + `UseDone` `0`. The client cannot
  distinguish a pick success from a failure on `UseDone` alone (both are `0`)
  — the `ServerMessage` text is authoritative.
* **Rejected** (no system chat — the `UseDone` carries the `WeenieError`, which
  the client maps to a system line):

| `WeenieError`             | Opc    | Meaning |
| ------------------------- | ------ | ------- |
| `LockAlreadyUnlocked`     | `0x04af` | the lock is already open |
| `YouCannotLockOrUnlockThat` | `0x04b0` | not a lock / cannot be picked |
| `YouCannotLockWhatIsOpen` | `0x04b1` | trying to lock an open lock |
| `KeyDoesntFitThisLock`    | `0x04b2` | the key is for a different lock |
| `YouArentTrainedInLockpicking` | `0x04b4` | lockpick without the skill |

A successful pick **consumes** the key / lockpick (`ObjectDelete` if used up,
else a structure update). A chest **reset** empties it via `ObjectDelete` of
the contents and re-locks it (`PublicUpdatePropertyBool` `0x02d2` `Locked =
true`); the client does not parse `0x02d2`, so a reset under an open box just
shows the box emptying (close it with `Esc`).

## 25. Combat (melee)

The core gameplay loop: stance, swing, damage, kill. All C2S are the §7
`GameAction` wrapper `0xF7B1` (opcodes from `GameActionType.cs`); all S2C
are the §7 `GameEvent` wrapper `0xF7B0`, all `UIQueue` (opcodes from
`GameEventType.cs`).

### 25.1 C2S `GameAction`s — §7 wrapper `0xF7B1`

| Opc    | Action               | Payload |
| ------ | -------------------- | ------- |
| `0x0053` | `ChangeCombatMode` | `u32 combatMode` — `1` NonCombat / `2` Melee / `4` Missile / `8` Magic (retail's stance bitmask). The server echoes the accepted stance back as `PrivateUpdatePropertyInt` `0x02CD` with `PropertyInt.CombatMode = 40`, so the displayed stance is server-authoritative. Switching away from Melee also cancels any in-flight swing (there is no separate cancel action the client sends) |
| `0x0008` | `TargetedMeleeAttack` | `u32 targetGuid, u32 attackHeight, f32 powerLevel` — `attackHeight` `1` High / `2` Medium / `3` Low; `powerLevel` `0..1`. The retail default swing is **Medium + full power** (`2`, `1.0`). The server walks/rotates the player to the target and resolves the swing |
| `0x01BF` | `QueryHealth`      | `u32 objectGuid` — the client's selected target (`0` = deselect). The server answers with an `UpdateHealth` fraction for that guid. Retail issues this as a ~1 s heartbeat while a target's health bar is visible |

### 25.2 S2C `GameEvent`s — §7 wrapper `0xF7B0`, all `UIQueue`

| Opc    | Event                       | Payload (verified against the server writers) |
| ------ | --------------------------- | --------------------------------------------- |
| `0x01b1` | `AttackerNotification`    | `String16L targetName, u32 damageType, f64 damagePercent, u32 damage, u32 criticalHit, u64 attackConditions` — a landed, **non-lethal** hit (see 25.3) |
| `0x01b3` | `EvasionAttackerNotification` | `String16L targetName` — a miss |
| `0x01c0` | `UpdateHealth`            | `u32 objectId, f32 health` — the target's health as a `0..1` fraction of max; the answer to a `QueryHealth` (`WorldObject.cs:610-617`) |
| `0x01ad` | `KillerNotification`      | `String16L deathMessage` — the server's kill text, e.g. "You reduce <name> to a sizzling, oozing mass!" / "The thunder of crushing <name> is followed by the deafening silence of death!" |
| `0x01a7` | `AttackDone`              | `u32 weenieError` — a swing finished; the client ignores it (the outcome is fully covered by the events above) |
| `0x01b8` | `CombatCommenceAttack`    | (empty) — the server started the swing; the client ignores it |

### 25.3 Server-side combat facts (verified against the running server)

* **The killing blow suppresses the hit notification.** `Player_Combat.cs:157`
  gates the `AttackerNotification` on `target.IsAlive`, so a swing that kills
  sends only the `KillerNotification`, plus the target's `ObjectDelete`
  (`0xF747`) and a new `512` corpse entity ("Corpse of <name>") in its place.
* **The stance echo is authoritative.** Every accepted `ChangeCombatMode`
  returns as `0x02CD` `u8 seq, u32 40, i32 mode`; the client shows the
  server's value, not the requested one.
* **This build's `/createinst` creatures all appraise at `Health.MaxValue = 3`**
  (via §19 `CreatureProfile.HealthMax`), whatever the class level. A
  multi-swing test needs a weapon whose per-swing damage is below 3 (e.g.
  `daggerfauxkey`, `Damage = 1`). Unarmed swings also land (HandCombat,
  base damage ≈ 2; the server logs a "no weapon" warning).

Client mapping (see AC-UI.md "Combat (TUI)"): a hit → a chat line
`You hit <name> for <N>` / `You crit <name> for <N>`; a miss → `You miss
<name>`; a kill → the death text verbatim. `UpdateHealth` folds the
fraction into the appraise vitals cache (`health = round(fraction ×
healthMax)`) that drives the status bar's `%H` readout.

### 25.4 Death & respawn (verified against the live server)

Unlike the combat events above, death is signalled by two **top-level**
GameMessages (not the §7 `0xF7B0` GameEvent wrapper), both captured
byte-for-byte from the live server log and covered by the `acproto` capture
regression tests:

| Opc    | Message                          | Payload |
| ------ | -------------------------------- | ------- |
| `0x019E` | `PlayerKilled`                | `String16L message, u32 victim, u32 killer` — broadcast when a player dies, to nearby players **including the victim themselves**; e.g. `+TuiHero's perforated corpse falls before Black Bunny!` |
| `0x02E9` | `PrivateUpdateAttribute2ndLevel` | `u8 seq, u32 vital, u32 value` — server-authoritative self vitals. `vital`: `2` Health / `4` Stamina / `6` Mana; `value` = **absolute** points; `seq` = per-vital counter |

The server's death sequence (`Player_Death.cs` `Die()` +
`ThreadSafeTeleportOnDeath()`): a `Motion(Dead)` broadcast, `0x02E9
Health=0`, `0x02CD` NumDeaths, the `0x02C2` vitae-penalty enchant, corpse
creation (a new `512` "Corpse of <name>" weenie), a `ThreadSafeTeleport` to
the sanctuary/lifestone, and ~3 s later `0x02E9` restoring Health/Stamina/Mana
to `round(0.75 × max)` — and to **full** at the lifestone.

Client mapping: `Health == 0` (0x02E9) sets the dead state — a red "YOU ARE
DEAD" overlay, motion packets stop, input is gated (retail's
`PlayerIsDead`), and any held key / pending jump is flushed so nothing fires
on revive. The `PlayerKilled` broadcast is handled retail-faithfully:
victim==self → dead (message **swallowed**, no chat line); killer==self →
swallowed (own kills are already covered by the `KillerNotification`);
otherwise → the message scrolls to chat. The revive vitals (`Health > 0`)
clear the dead state. The death teleport itself rides the existing
`PlayerTeleport` (`0xF751`) / `UpdatePosition` (`0xF748`) flow.
