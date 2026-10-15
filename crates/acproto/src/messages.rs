//! ACE game messages: opcode constants, C2S body builders, and S2C decoders.
//!
//! Every layout is byte-exact against the ACE emulator server source
//! (`ACE.Server.Network` @ 47edade). C2S game messages are carried as a single
//! fragment whose data is `u32 opcode ++ body`; the handshake packets carry
//! their body inline in the optional-header bytes (see `crate::packet`).

use crate::codec::{Position, Quat, Reader, Vec3, Writer};
use crate::motion;

// =====================================================================
// Opcode constants
// =====================================================================

/// `GameMessageOpcode` values (shared C2S/S2C direction).
pub mod opcode {
    pub const CHARACTER_CREATE: u32 = 0xF656;
    pub const CHARACTER_CREATE_RESPONSE: u32 = 0xF643;
    pub const CHARACTER_DELETE: u32 = 0xF655;
    pub const CHARACTER_ENTER_WORLD: u32 = 0xF657;
    pub const CHARACTER_ENTER_WORLD_REQUEST: u32 = 0xF7C8;
    pub const CHARACTER_ENTER_WORLD_SERVER_READY: u32 = 0xF7DF;
    pub const CHARACTER_LIST: u32 = 0xF658;
    pub const CHARACTER_ERROR: u32 = 0xF659;
    pub const CHARACTER_LOGOFF: u32 = 0xF653;
    pub const OBJECT_CREATE: u32 = 0xF745;
    pub const PLAYER_CREATE: u32 = 0xF746;
    pub const OBJECT_DELETE: u32 = 0xF747;
    pub const UPDATE_POSITION: u32 = 0xF748;
    pub const MOTION: u32 = 0xF74C;
    pub const VECTOR_UPDATE: u32 = 0xF74E;
    pub const PLAYER_TELEPORT: u32 = 0xF751;
    pub const AUTONOMOUS_POSITION: u32 = 0xF753;
    pub const GAME_EVENT: u32 = 0xF7B0;
    pub const GAME_ACTION: u32 = 0xF7B1;
    pub const ACCOUNT_BANNED: u32 = 0xF7C1;
    pub const ACCOUNT_BOOT: u32 = 0xF7DC;
    pub const TURBINE_CHAT: u32 = 0xF7DE;
    pub const SERVER_MESSAGE: u32 = 0xF7E0;
    pub const SERVER_NAME: u32 = 0xF7E1;
    pub const DDD_INTERROGATION: u32 = 0xF7E5;
    pub const HEAR_SPEECH: u32 = 0x02BB;
    pub const EMOTE_TEXT: u32 = 0x01E0;
}

/// `GameActionType` values (C2S actions, wrapped in `GAME_ACTION`).
pub mod action {
    pub const TARGETED_MELEE_ATTACK: u32 = 0x0008;
    pub const TALK: u32 = 0x0015;
    pub const TALK_DIRECT: u32 = 0x0032;
    pub const CHANGE_COMBAT_MODE: u32 = 0x0053;
    pub const TELL: u32 = 0x005D;
    pub const LOGIN_COMPLETE: u32 = 0x00A1;
    pub const IDENTIFY_OBJECT: u32 = 0x00C8;
    pub const QUERY_HEALTH: u32 = 0x01BF;
    pub const PING_REQUEST: u32 = 0x01E9;
    pub const JUMP: u32 = 0xF61B;
    pub const MOVE_TO_STATE: u32 = 0xF61C;
    pub const AUTONOMOUS_POSITION: u32 = 0xF753;
}

/// `GameEventType` values (S2C events, wrapped in `GAME_EVENT`).
pub mod event {
    pub const PLAYER_DESCRIPTION: u32 = 0x0013;
    pub const IDENTIFY_OBJECT_RESPONSE: u32 = 0x00C9;
    pub const KILLER_NOTIFICATION: u32 = 0x01AD;
    pub const ATTACKER_NOTIFICATION: u32 = 0x01B1;
    pub const DEFENDER_NOTIFICATION: u32 = 0x01B2;
    pub const EVASION_ATTACKER_NOTIFICATION: u32 = 0x01B3;
    pub const UPDATE_HEALTH: u32 = 0x01C0;
    pub const PING_RESPONSE: u32 = 0x01EA;
    pub const SET_TURBINE_CHAT_CHANNELS: u32 = 0x0295;
    pub const TELL: u32 = 0x02BD;
}

/// `CombatMode` bitmask (PropertyInt 40 / `CHANGE_COMBAT_MODE` body).
pub mod combat_mode {
    pub const NON_COMBAT: u32 = 0x1;
    pub const MELEE: u32 = 0x2;
    pub const MISSILE: u32 = 0x4;
    pub const MAGIC: u32 = 0x8;
}

/// `PropertyInt` keys we surface in the TUI.
pub mod property_int {
    pub const ENCUMBRANCE: u32 = 5;
    pub const ITEMS_CAPACITY: u32 = 6;
    pub const COIN_VALUE: u32 = 20;
    pub const LEVEL: u32 = 25;
    pub const COMBAT_MODE: u32 = 40;
    pub const ENCUMBRANCE_CAPACITY: u32 = 96;
    pub const AGE: u32 = 125;
}

/// `PropertyInt64` keys.
pub mod property_int64 {
    pub const TOTAL_EXPERIENCE: u32 = 1;
    pub const AVAILABLE_EXPERIENCE: u32 = 2;
}

/// `PropertyString` keys.
pub mod property_string {
    pub const NAME: u32 = 1;
    pub const TITLE: u32 = 2;
}

/// `ChatMessageType` values (S2C chat styling).
pub mod chat_type {
    pub const BROADCAST: u32 = 0;
    pub const SPEECH: u32 = 2;
    pub const TELL: u32 = 3;
    pub const OUTGOING_TELL: u32 = 4;
    pub const SYSTEM: u32 = 5;
    pub const COMBAT: u32 = 6;
    pub const CHANNEL: u32 = 8;
    pub const EMOTE: u32 = 0x0C;
}

/// TurbineChat `TurbineChatChannel` / `ChatType` ids (they coincide).
pub mod turbine_channel {
    pub const ALLEGIANCE: u32 = 1;
    pub const GENERAL: u32 = 2;
    pub const TRADE: u32 = 3;
    pub const LFG: u32 = 4;
    pub const ROLEPLAY: u32 = 5;
    pub const OLTHOI: u32 = 10;
}

// =====================================================================
// C2S handshake + auth body builders
// =====================================================================

/// The `LoginRequest` optional-header body.
///
/// `String16L ClientVersion, u32 len, u32 NetAuthType, u32 authFlags,
/// u32 timestamp, String16L account, String16L accountToLoginAs,
/// String32L password`.
#[must_use]
pub fn login_request_body(client_version: &str, account: &str, password: &str) -> Vec<u8> {
    let mut rest = Writer::new();
    rest.u32(2); // NetAuthType::AccountPassword
    rest.u32(0); // authFlags
    rest.u32(0); // timestamp (client sequence, unused by server)
    rest.string16l(account);
    rest.string16l(account); // accountToLoginAs (admin-only override, same name)
    rest.string32l(password);
    let mut w = Writer::new();
    w.string16l(client_version);
    w.u32(rest.out.len() as u32); // bytes remaining incl. password
    w.raw(&rest.out);
    w.out
}

/// The `ConnectResponse` optional-header body: a single `u64 cookie`.
#[must_use]
pub fn connect_response_body(cookie: u64) -> Vec<u8> {
    let mut w = Writer::new();
    w.u64(cookie);
    w.out
}

/// `u32 opcode ++ body`: a complete C2S game message.
#[must_use]
pub fn game_message(opcode: u32, body: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(opcode);
    w.raw(body);
    w.out
}

// ---------------------------------------------------------------------
// Character creation
// ---------------------------------------------------------------------

/// A character-creation request. Appearance is fixed to defaults (all-zero
/// palette ids except `HeadgearStyle = 0xFFFFFFFF`, zero hues); the server
/// fills in a valid default from the heritage template.
#[derive(Clone, Debug)]
pub struct CharacterCreate {
    /// Account name — the server's `CharacterCreate` handler reads this
    /// first (`ReadString16L`) and silently drops the request unless it
    /// equals `session.Account`.
    pub account: String,
    /// HeritageGroup id (1..=13).
    pub heritage: u32,
    /// 1 = male, 0 = female.
    pub gender: u32,
    /// Template index within the heritage (see `crate::chargen`).
    pub template: i32,
    /// Six attributes in creation order: Str, End, Coord, Quick, Focus, Self.
    pub attrs: [u32; 6],
    pub name: String,
    /// Index into the starter areas (see `crate::chargen`).
    pub start_area: u32,
}

impl CharacterCreate {
    /// Build the `CharacterCreate` (0xF656) body.
    ///
    /// Layout (server `CharacterHandler.CharacterCreate` +
    /// `CharacterCreateInfo.Unpack`):
    /// `String16L account, u32 1 (unknown const), u32 heritage, u32 gender,
    /// Appearance(14 u32 + 6 f64), i32 template, 6 u32 attrs, u32 slot,
    /// u32 class, u32 numSkills + N u32, String16L name, u32 startArea,
    /// u32 isAdmin, u32 isSentinel`.
    #[must_use]
    pub fn body(&self) -> Vec<u8> {
        let mut w = Writer::new();
        // The server compares this to session.Account and silently returns
        // on mismatch — it is NOT optional.
        w.string16l(&self.account);
        w.u32(1); // "unknown constant" — always 1 on the wire
        w.u32(self.heritage);
        w.u32(self.gender);
        // Appearance: 14 u32 (index 6 = HeadgearStyle) + 6 f64 hues.
        for i in 0..14u32 {
            w.u32(if i == 6 { 0xFFFF_FFFF } else { 0 });
        }
        for _ in 0..6 {
            w.f64(0.0);
        }
        w.i32(self.template);
        for a in self.attrs {
            w.u32(a);
        }
        w.u32(0); // CharacterSlot
        w.u32(0); // ClassId
        w.u32(55); // NumOfSkills
        for _ in 0..55 {
            w.u32(0); // SkillAdvancementClass::Inactive
        }
        w.string16l(&self.name);
        w.u32(self.start_area);
        w.u32(0); // IsAdmin
        w.u32(0); // IsSentinel
        w.out
    }
}

/// `CharacterDelete` (0xF655) body: `String16L account, u32 slot`.
#[must_use]
pub fn character_delete_body(account: &str, slot: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.string16l(account);
    w.u32(slot);
    w.out
}

/// `CharacterEnterWorld` (0xF657) body: `u32 guid, String16L account`.
#[must_use]
pub fn character_enter_world_body(guid: u32, account: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.string16l(account);
    w.out
}

// ---------------------------------------------------------------------
// C2S GameAction wrapper + action bodies
// ---------------------------------------------------------------------

/// A complete C2S `GameAction` (0xF7B1) game message:
/// `u32 opcode, u32 clientSeq, u32 actionType, body`.
#[must_use]
pub fn game_action(client_seq: u32, action_type: u32, body: &[u8]) -> Vec<u8> {
    let mut b = Writer::new();
    b.u32(client_seq);
    b.u32(action_type);
    b.raw(body);
    game_message(opcode::GAME_ACTION, &b.out)
}

/// `TALK` (0x0015) body: `String16L text`.
#[must_use]
pub fn body_talk(text: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.string16l(text);
    w.out
}

/// `TELL` (0x005D) body: `String16L text, String16L target` (message first).
#[must_use]
pub fn body_tell(text: &str, target: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.string16l(text);
    w.string16l(target);
    w.out
}

/// `CHANGE_COMBAT_MODE` (0x0053) body: `u32 mode`.
#[must_use]
pub fn body_change_combat_mode(mode: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(mode);
    w.out
}

/// `IDENTIFY_OBJECT` (0x00C8) body: `u32 guid`.
#[must_use]
pub fn body_identify_object(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `QUERY_HEALTH` (0x01BF) body: `u32 guid`.
#[must_use]
pub fn body_query_health(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `TARGETED_MELEE_ATTACK` (0x0008) body:
/// `u32 target, u32 height (2), f32 power (1.0)`.
#[must_use]
pub fn body_targeted_melee_attack(target: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(target);
    w.u32(2);
    w.f32(1.0);
    w.out
}

// =====================================================================
// C2S TurbineChat
// =====================================================================

/// Read a packed byte length (1 byte, or 2 bytes with high bit set).
fn read_packed_len(r: &mut Reader) -> Option<u32> {
    if r.remaining() < 1 {
        return None;
    }
    let b = r.u8();
    if b & 0x80 != 0 {
        if r.remaining() < 1 {
            return None;
        }
        let low = r.u8();
        Some((((b & 0x7F) as u32) << 8) | low as u32)
    } else {
        Some(b as u32)
    }
}

fn write_packed_len(w: &mut Writer, len: u32) {
    if len < 0x80 {
        w.u8(len as u8);
    } else {
        w.u8(0x80 | ((len >> 8) as u8));
        w.u8((len & 0xFF) as u8);
    }
}

/// The `TurbineChat` (0xF7DE) C2S body: a `NETBLOB_REQUEST_BINARY`
/// (blobType 3) `SENDTOROOMBYNAME` (dispatch 1) chat send.
#[must_use]
pub fn turbine_chat_body(channel: u32, sender_name: &str, message: &str, chat_type: u32) -> Vec<u8> {
    // Build the payload that follows the second "bytes to follow".
    let mut payload = Writer::new();
    payload.u32(1); // contextId
    payload.u32(2); // responseID
    payload.u32(2); // methodID
    payload.u32(channel);
    write_packed_len(&mut payload, message.chars().count() as u32);
    payload.raw(&message.encode_utf16().collect::<Vec<u16>>().iter().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>());
    payload.u32(0x0C);
    payload.u32(0); // senderID (0 = self)
    payload.u32(0);
    payload.u32(chat_type);

    let mut w = Writer::new();
    let first_size = (8 * 7 + 4 + payload.out.len()) as u32;
    w.u32(first_size); // bytes to follow
    w.u32(3); // NETBLOB_REQUEST_BINARY
    w.u32(1); // ASYNCMETHOD_SENDTOROOMBYNAME
    w.u32(1);
    w.u32(0);
    w.u32(0);
    w.u32(0);
    w.u32(0);
    w.u32(payload.out.len() as u32); // bytes to follow
    w.raw(&payload.out);
    let _ = sender_name; // BYNAME sends the player name implicitly via session
    w.out
}

// =====================================================================
// S2C types
// =====================================================================

/// One entry from the `CharacterList` (0xF658) message.
#[derive(Clone, Debug)]
pub struct CharacterListEntry {
    pub guid: u32,
    pub name: String,
    /// Seconds since last deletion, or 0 if not deleted.
    pub time_since_delete: u32,
}

/// The `CharacterList` (0xF658) message.
#[derive(Clone, Debug)]
pub struct CharacterList {
    pub characters: Vec<CharacterListEntry>,
    pub slot_count: u32,
    pub account: String,
    pub use_turbine_chat: u32,
}

/// The `CharacterCreateResponse` (0xF643) message.
#[derive(Clone, Debug)]
pub struct CharacterCreateResponse {
    /// `CharacterGenerationVerificationResponse`: 1 Ok, 2 Pending, 3
    /// NameInUse, 4 NameBanned, 5 Corrupt, 6 DatabaseDown.
    pub status: u32,
    pub guid: u32,
    pub name: Option<String>,
}

/// The `ServerName` (0xF7E1) message.
#[derive(Clone, Debug)]
pub struct ServerName {
    pub current: u32,
    pub max: u32,
    pub name: String,
}

/// An `UpdatePosition` (0xF748) message.
#[derive(Clone, Debug)]
pub struct UpdatePosition {
    pub guid: u32,
    pub cell: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Option<Vec3>,
    pub grounded: bool,
}

/// A parsed `ObjectCreate` (0xF745) message. `name` and `position` are
/// best-effort: `None` if the payload could not be fully decoded.
#[derive(Clone, Debug)]
pub struct ObjectCreate {
    pub guid: u32,
    pub name: Option<String>,
    pub position: Option<Position>,
}

/// A `VectorUpdate` (0xF74E) message.
#[derive(Clone, Debug)]
pub struct VectorUpdate {
    pub guid: u32,
    pub vel: Vec3,
    pub omega: Vec3,
}

/// A `PlayerDescription` (event 0x0013) decoded up through the attribute
/// and vital sections — everything the TUI needs, stopping before the
/// variable-length skills/spells/enchantments/options/inventory tail.
#[derive(Clone, Debug, Default)]
pub struct PlayerDescription {
    pub weenie_type: u32,
    /// (PropertyInt, value)
    pub ints: Vec<(u32, i32)>,
    /// (PropertyInt64, value)
    pub int64s: Vec<(u32, i64)>,
    /// (PropertyString, value)
    pub strings: Vec<(u32, String)>,
    /// Current attribute values: Str, End, Quick, Coord, Focus, Self.
    pub attrs: [u32; 6],
    /// Current vitals: Health, Stamina, Mana.
    pub vitals: [u32; 3],
    /// Max vitals (the "starting" value): Health, Stamina, Mana.
    pub vitals_max: [u32; 3],
}

/// An `AppraiseInfo` (event 0x00C9) decoded for creature health.
#[derive(Clone, Debug, Default)]
pub struct Appraise {
    pub guid: u32,
    pub success: bool,
    pub health: u32,
    pub health_max: u32,
    /// (PropertyInt, value)
    pub ints: Vec<(u32, i32)>,
}

/// A `TurbineChat` (0xF7DE) S2C message.
#[derive(Clone, Debug)]
pub struct TurbineChat {
    pub channel: u32,
    pub sender: String,
    pub message: String,
    pub chat_type: u32,
    /// True for a server acknowledgement (NETBLOB_RESPONSE_BINARY, no text).
    pub is_ack: bool,
}

/// A decoded `GAME_EVENT` body.
#[derive(Clone, Debug)]
pub enum Event {
    PlayerDescription(PlayerDescription),
    Appraise(Appraise),
    UpdateHealth {
        guid: u32,
        health: f32,
    },
    DefenderNotification {
        attacker: String,
        damage: u32,
        critical: bool,
    },
    AttackerNotification {
        name: String,
        damage: u32,
        critical: bool,
    },
    EvasionAttacker {
        name: String,
    },
    KillerNotification {
        text: String,
    },
    SetTurbineChatChannels([u32; 10]),
    Tell {
        text: String,
        name: String,
        chat_type: u32,
    },
    PingResponse,
    Unknown,
}

/// A fully parsed `GAME_EVENT` (0xF7B0) message.
#[derive(Clone, Debug)]
pub struct GameEvent {
    pub player_guid: u32,
    pub evt_seq: u32,
    pub event_type: u32,
    pub event: Event,
}

/// A decoded S2C game message (the reassembled fragment data).
#[derive(Clone, Debug)]
pub enum Incoming {
    CharacterList(CharacterList),
    CharacterCreateResponse(CharacterCreateResponse),
    CharacterLogOff,
    CharacterDelete,
    CharacterError(u32),
    ServerName(ServerName),
    EnterWorldServerReady,
    AccountBanned {
        seconds: u32,
        reason: Option<String>,
    },
    AccountBoot {
        reason: Option<String>,
    },
    DddInterrogation,
    ObjectCreate(ObjectCreate),
    PlayerCreate(u32),
    ObjectDelete(u32),
    UpdatePosition(UpdatePosition),
    Motion(motion::MotionEvent),
    VectorUpdate(VectorUpdate),
    PlayerTeleport(u32),
    GameEvent(GameEvent),
    TurbineChat(TurbineChat),
    HearSpeech {
        text: String,
        sender: String,
        sender_id: u32,
        chat_type: u32,
    },
    ServerMessage {
        text: String,
        chat_type: u32,
    },
    /// Anything else (logged and ignored).
    Other(u32),
}

// =====================================================================
// S2C parsing
// =====================================================================

/// Decode a reassembled S2C game message (starting at the `u32 opcode`).
#[must_use]
pub fn parse_incoming(data: &[u8]) -> Option<Incoming> {
    if data.len() < 4 {
        return None;
    }
    let mut r = Reader::new(data);
    let op = r.u32();
    match op {
        opcode::CHARACTER_LIST => Some(Incoming::CharacterList(parse_character_list(&mut r)?)),
        opcode::CHARACTER_CREATE_RESPONSE => {
            Some(Incoming::CharacterCreateResponse(parse_create_response(&mut r)?))
        }
        opcode::CHARACTER_LOGOFF => Some(Incoming::CharacterLogOff),
        opcode::CHARACTER_DELETE => Some(Incoming::CharacterDelete),
        opcode::CHARACTER_ERROR => Some(Incoming::CharacterError(r.u32())),
        opcode::SERVER_NAME => Some(Incoming::ServerName(parse_server_name(&mut r)?)),
        opcode::CHARACTER_ENTER_WORLD_SERVER_READY => Some(Incoming::EnterWorldServerReady),
        opcode::ACCOUNT_BANNED => {
            let seconds = r.u32();
            let reason = if r.remaining() >= 4 { Some(r.string16l()) } else { None };
            Some(Incoming::AccountBanned { seconds, reason })
        }
        opcode::ACCOUNT_BOOT => {
            let reason = if r.remaining() >= 4 { Some(r.string16l()) } else { None };
            Some(Incoming::AccountBoot { reason })
        }
        opcode::DDD_INTERROGATION => Some(Incoming::DddInterrogation),
        opcode::OBJECT_CREATE => Some(Incoming::ObjectCreate(parse_object_create(&data[4..]))),
        opcode::PLAYER_CREATE => Some(Incoming::PlayerCreate(r.u32())),
        opcode::OBJECT_DELETE => Some(Incoming::ObjectDelete(r.u32())),
        opcode::UPDATE_POSITION => Some(Incoming::UpdatePosition(parse_update_position(&mut r)?)),
        opcode::MOTION => Some(Incoming::Motion(motion::parse_motion(&data[4..])?)),
        opcode::VECTOR_UPDATE => Some(Incoming::VectorUpdate(parse_vector_update(&mut r)?)),
        opcode::PLAYER_TELEPORT => Some(Incoming::PlayerTeleport(r.u32())),
        opcode::GAME_EVENT => Some(Incoming::GameEvent(parse_game_event(&mut r)?)),
        opcode::TURBINE_CHAT => Some(Incoming::TurbineChat(parse_turbine_chat(&mut r)?)),
        opcode::HEAR_SPEECH => {
            let text = r.string16l();
            let sender = r.string16l();
            let sender_id = r.u32();
            let chat_type = r.u32();
            Some(Incoming::HearSpeech { text, sender, sender_id, chat_type })
        }
        opcode::SERVER_MESSAGE => {
            let text = r.string16l();
            let chat_type = r.u32();
            Some(Incoming::ServerMessage { text, chat_type })
        }
        _ => Some(Incoming::Other(op)),
    }
}

fn parse_character_list(r: &mut Reader) -> Option<CharacterList> {
    r.u32(); // leading 0
    let count = r.u32() as usize;
    let mut characters = Vec::with_capacity(count);
    for _ in 0..count {
        let guid = r.u32();
        let name = r.string16l();
        let time_since_delete = r.u32();
        characters.push(CharacterListEntry { guid, name, time_since_delete });
    }
    r.u32(); // trailing 0
    let slot_count = r.u32();
    let account = r.string16l();
    let use_turbine_chat = r.u32();
    Some(CharacterList { characters, slot_count, account, use_turbine_chat })
}

fn parse_create_response(r: &mut Reader) -> Option<CharacterCreateResponse> {
    let status = r.u32();
    if status == 1 {
        let guid = r.u32();
        let name = Some(r.string16l());
        r.u32();
        Some(CharacterCreateResponse { status, guid, name })
    } else {
        Some(CharacterCreateResponse { status, guid: 0, name: None })
    }
}

fn parse_server_name(r: &mut Reader) -> Option<ServerName> {
    let current = r.u32();
    let max = r.u32();
    let name = r.string16l();
    Some(ServerName { current, max, name })
}

/// `UpdatePosition` (0xF748): `u32 guid, u32 flags, Origin (cell + 3 f32),
/// [orientation unless masked], [velocity], [placement], 4 x u16 seqs`.
fn parse_update_position(r: &mut Reader) -> Option<UpdatePosition> {
    let guid = r.u32();
    let flags = r.u32();
    let cell = r.u32();
    let pos = Vec3 { x: r.f32(), y: r.f32(), z: r.f32() };
    let mut rot = Quat::default();
    if flags & 0x8 == 0 {
        rot.w = r.f32();
    }
    if flags & 0x10 == 0 {
        rot.x = r.f32();
    }
    if flags & 0x20 == 0 {
        rot.y = r.f32();
    }
    if flags & 0x40 == 0 {
        rot.z = r.f32();
    }
    let vel = if flags & 0x1 != 0 {
        Some(Vec3 { x: r.f32(), y: r.f32(), z: r.f32() })
    } else {
        None
    };
    if flags & 0x2 != 0 {
        r.u32(); // placement
    }
    r.u16();
    r.u16();
    r.u16();
    r.u16();
    Some(UpdatePosition { guid, cell, pos, rot, vel, grounded: flags & 0x4 != 0 })
}

fn parse_vector_update(r: &mut Reader) -> Option<VectorUpdate> {
    let guid = r.u32();
    let vel = Vec3 { x: r.f32(), y: r.f32(), z: r.f32() };
    let omega = Vec3 { x: r.f32(), y: r.f32(), z: r.f32() };
    r.u16();
    r.u16();
    Some(VectorUpdate { guid, vel, omega })
}

/// Decode a `GAME_EVENT` (0xF7B0) body (opcode already consumed).
fn parse_game_event(r: &mut Reader) -> Option<GameEvent> {
    let player_guid = r.u32();
    let evt_seq = r.u32();
    let event_type = r.u32();
    let event = decode_event(event_type, r);
    Some(GameEvent { player_guid, evt_seq, event_type, event })
}

fn decode_event(event_type: u32, r: &mut Reader) -> Event {
    match event_type {
        event::PLAYER_DESCRIPTION => Event::PlayerDescription(parse_player_description(r)),
        event::IDENTIFY_OBJECT_RESPONSE => Event::Appraise(parse_appraise(r)),
        event::UPDATE_HEALTH => Event::UpdateHealth { guid: r.u32(), health: r.f32() },
        event::DEFENDER_NOTIFICATION => {
            let attacker = r.string16l();
            r.u32(); // damageType
            r.f64(); // percent
            let damage = r.u32();
            r.u32(); // location
            let critical = r.u32() != 0;
            Event::DefenderNotification { attacker, damage, critical }
        }
        event::ATTACKER_NOTIFICATION => {
            let name = r.string16l();
            r.u32(); // type
            r.f64(); // percent
            let damage = r.u32();
            let critical = r.u32() != 0;
            Event::AttackerNotification { name, damage, critical }
        }
        event::EVASION_ATTACKER_NOTIFICATION => Event::EvasionAttacker { name: r.string16l() },
        event::KILLER_NOTIFICATION => Event::KillerNotification { text: r.string16l() },
        event::SET_TURBINE_CHAT_CHANNELS => {
            let mut ch = [0u32; 10];
            for c in ch.iter_mut() {
                *c = r.u32();
            }
            Event::SetTurbineChatChannels(ch)
        }
        event::PING_RESPONSE => Event::PingResponse,
        event::TELL => {
            let text = r.string16l();
            let name = r.string16l();
            r.u32(); // senderID
            r.u32(); // targetID
            let chat_type = r.u32();
            Event::Tell { text, name, chat_type }
        }
        _ => Event::Unknown,
    }
}

/// `PlayerDescription` (0x0013) front section. Stops after the attribute
/// and vital blocks; the variable-length tail (skills, spells, enchantments,
/// options, inventory) is not decoded.
fn parse_player_description(r: &mut Reader) -> PlayerDescription {
    let mut pd = PlayerDescription {
        weenie_type: 0,
        ints: Vec::new(),
        int64s: Vec::new(),
        strings: Vec::new(),
        attrs: [0; 6],
        vitals: [0; 3],
        vitals_max: [0; 3],
    };

    // Back-patched property flags (written as 0 at the front, patched at the
    // end). Each property hash section is present iff its bit is set — the
    // server writes a section only when it is non-empty.
    const PROP_INT32: u32 = 0x0001;
    const PROP_BOOL: u32 = 0x0002;
    const PROP_DOUBLE: u32 = 0x0004;
    const PROP_DID: u32 = 0x0008;
    const PROP_STRING: u32 = 0x0010;
    const PROP_POSITION: u32 = 0x0020;
    const PROP_IID: u32 = 0x0040;
    const PROP_INT64: u32 = 0x0080;

    let prop_flags = r.u32();
    pd.weenie_type = r.u32();

    // Property hash sections, in the server's write order, each gated on its
    // propertyFlags bit.
    if prop_flags & PROP_INT32 != 0 {
        let (count, _) = read_hash_header(r);
        for _ in 0..count {
            pd.ints.push((r.u32(), r.i32()));
        }
    }
    if prop_flags & PROP_INT64 != 0 {
        let (count, _) = read_hash_header(r);
        for _ in 0..count {
            pd.int64s.push((r.u32(), r.i64()));
        }
    }
    if prop_flags & PROP_BOOL != 0 {
        let (count, _) = read_hash_header(r);
        for _ in 0..count {
            r.u32();
            r.u32();
        }
    }
    if prop_flags & PROP_DOUBLE != 0 {
        let (count, _) = read_hash_header(r);
        for _ in 0..count {
            r.u32();
            r.f64();
        }
    }
    if prop_flags & PROP_STRING != 0 {
        let (count, _) = read_hash_header(r);
        for _ in 0..count {
            pd.strings.push((r.u32(), r.string16l()));
        }
    }
    if prop_flags & PROP_DID != 0 {
        let (count, _) = read_hash_header(r);
        for _ in 0..count {
            r.u32();
            r.u32();
        }
    }
    if prop_flags & PROP_IID != 0 {
        let (count, _) = read_hash_header(r);
        for _ in 0..count {
            r.u32();
            r.u32();
        }
    }
    // Position (LastOutsideDeath) — present only if the player has one.
    if prop_flags & PROP_POSITION != 0 {
        let (count, _) = read_hash_header(r);
        for _ in 0..count {
            r.u32(); // PositionType
            r.raw(32); // Position
        }
    }

    let vector_flags = r.u32();
    let _has_health = r.u32();

    // Attribute + vitals, gated on the Attribute vector flag.
    if vector_flags & 0x0001 != 0 {
        let attr_flags = r.u32();
        // Attribute order: Str 0x1, End 0x2, Quick 0x4, Coord 0x8, Focus 0x10,
        // Self 0x20 — each `u32 ranks, u32 starting, u32 xpSpent`. At login the
        // current value of a base attribute equals its starting value.
        let attr_order = [0x1, 0x2, 0x4, 0x8, 0x10, 0x20];
        for (i, &flag) in attr_order.iter().enumerate() {
            if attr_flags & flag != 0 {
                r.u32(); // ranks
                pd.attrs[i] = r.u32(); // starting == current at login
                r.u32(); // xpSpent
            }
        }
        // Vital order: Health 0x40, Stamina 0x80, Mana 0x100 — each
        // `u32 ranks, u32 starting, u32 xpSpent, u32 current`.
        let vital_order = [0x40, 0x80, 0x100];
        for (i, &flag) in vital_order.iter().enumerate() {
            if attr_flags & flag != 0 {
                r.u32(); // ranks
                pd.vitals_max[i] = r.u32(); // starting == max
                r.u32(); // xp
                pd.vitals[i] = r.u32(); // current
            }
        }
    }

    pd
}

/// Read a `PackableHashTable` header: `u16 count, u16 numBuckets`.
fn read_hash_header(r: &mut Reader) -> (usize, u16) {
    let count = r.u16() as usize;
    let buckets = r.u16();
    (count, buckets)
}

/// Read a `PackableHashTable` header (`u16 count, u16 numBuckets`) and verify
/// that `count` entries of `entry_bytes` each still fit in the buffer.
/// Returns the entry count, or `None` when the message is truncated (the
/// caller stops parsing).
fn read_table(r: &mut Reader, entry_bytes: usize) -> Option<usize> {
    if r.remaining() < 4 {
        return None;
    }
    let count = r.u16() as usize;
    let _buckets = r.u16();
    let needed = count.checked_mul(entry_bytes)?;
    (r.remaining() >= needed).then_some(count)
}

/// Skip `bytes` whole, reporting truncation instead of panicking.
fn skip_section(r: &mut Reader, bytes: usize) -> bool {
    if r.remaining() < bytes {
        return false;
    }
    r.raw(bytes);
    true
}

/// Skip a `PackableHashTable` section of fixed-size entries.
fn skip_table(r: &mut Reader, entry_bytes: usize) -> bool {
    match read_table(r, entry_bytes) {
        Some(count) => skip_section(r, count * entry_bytes),
        None => false,
    }
}

/// `AppraiseInfo` (0x00C9). `GameEventIdentifyObjectResponse` writes the
/// target guid *before* the AppraiseInfo body, so it is read here.
///
/// Sections are decoded in the C# write order, capturing the guid, success,
/// creature health, and int stats. Every read is bounds-checked: a truncated
/// or malformed message ends parsing cleanly instead of panicking — a network
/// parser must never take the session down.
fn parse_appraise(r: &mut Reader) -> Appraise {
    let mut a = Appraise::default();
    if r.remaining() < 12 {
        return a;
    }
    a.guid = r.u32();
    let flags = r.u32();
    a.success = r.u32() != 0;

    // IntStatsTable (0x1): capture (PropertyInt, value) pairs.
    if flags & 0x1 != 0 {
        match read_table(r, 8) {
            Some(count) => {
                for _ in 0..count {
                    a.ints.push((r.u32(), r.i32()));
                }
            }
            None => return a,
        }
    }
    // Int64StatsTable (0x2000): (u32, i64)
    if flags & 0x2000 != 0 && !skip_table(r, 12) {
        return a;
    }
    // BoolStatsTable (0x2): (u32, u32)
    if flags & 0x2 != 0 && !skip_table(r, 8) {
        return a;
    }
    // FloatStatsTable (0x4): (u32, f64)
    if flags & 0x4 != 0 && !skip_table(r, 12) {
        return a;
    }
    // StringStatsTable (0x8): (u32 key, string16l) — variable length.
    if flags & 0x8 != 0 {
        if r.remaining() < 4 {
            return a;
        }
        let count = r.u16() as usize;
        let _buckets = r.u16();
        for _ in 0..count {
            if r.remaining() < 6 {
                return a;
            }
            let _key = r.u32();
            if r.remaining() < 2 {
                return a;
            }
            match crate::strings::read_string16l(r.buf, r.pos) {
                Some((_, new_pos)) => r.pos = new_pos,
                None => return a,
            }
        }
    }
    // DidStatsTable (0x1000): (u32, u32)
    if flags & 0x1000 != 0 && !skip_table(r, 8) {
        return a;
    }
    // SpellBook (0x10): u32 count ++ count × u32
    if flags & 0x10 != 0 {
        if r.remaining() < 4 {
            return a;
        }
        let count = r.u32() as usize;
        if !skip_section(r, count.saturating_mul(4)) {
            return a;
        }
    }
    // ArmorProfile (0x80): 8 × f32
    if flags & 0x80 != 0 && !skip_section(r, 32) {
        return a;
    }
    // CreatureProfile (0x100): flags, health, health_max,
    // [0x8: 10 × u32 attributes], [0x1: 2 × u16 buff masks]
    if flags & 0x100 != 0 {
        if r.remaining() < 12 {
            return a;
        }
        let cflags = r.u32();
        a.health = r.u32();
        a.health_max = r.u32();
        if cflags & 0x8 != 0 && !skip_section(r, 40) {
            return a;
        }
        if cflags & 0x1 != 0 && !skip_section(r, 4) {
            return a;
        }
    }
    // WeaponProfile (0x20): 5 × u32 ++ 4 × f64 ++ u32 = 60 bytes
    if flags & 0x20 != 0 && !skip_section(r, 60) {
        return a;
    }
    // HookProfile (0x40): 3 × u32
    if flags & 0x40 != 0 && !skip_section(r, 12) {
        return a;
    }
    // Enchantment bitfields (C# order: armor, weapon, resist): 2 × u16 each
    for mask in [0x200u32, 0x800, 0x400] {
        if flags & mask != 0 && !skip_section(r, 4) {
            return a;
        }
    }
    // ArmorLevels (0x4000): 9 × u32
    if flags & 0x4000 != 0 && !skip_section(r, 36) {
        return a;
    }
    a
}

/// `TurbineChat` (0xF7DE) S2C. Handles both `NETBLOB_EVENT_BINARY` (1, chat
/// text) and `NETBLOB_RESPONSE_BINARY` (5, ack).
fn parse_turbine_chat(r: &mut Reader) -> Option<TurbineChat> {
    r.u32(); // bytes to follow
    let blob_type = r.u32();
    let _dispatch = r.u32();
    r.u32(); // 1
    r.u32(); // unique id
    r.u32(); // 1
    r.u32(); // unique id
    r.u32(); // 0
    r.u32(); // bytes to follow
    let channel = r.u32();
    if blob_type == 5 {
        // NETBLOB_RESPONSE_BINARY: no text.
        Some(TurbineChat { channel, sender: String::new(), message: String::new(), chat_type: 0, is_ack: true })
    } else {
        // NETBLOB_EVENT_BINARY
        let sender = read_packed_string(r)?;
        let message = read_packed_string(r)?;
        r.u32(); // 0x0C
        r.u32(); // senderID
        r.u32(); // 0
        let chat_type = r.u32();
        Some(TurbineChat { channel, sender, message, chat_type, is_ack: false })
    }
}

fn read_packed_string(r: &mut Reader) -> Option<String> {
    let len = read_packed_len(r)? as usize;
    let bytes = r.raw(len * 2);
    let (pairs, _rest) = bytes.as_chunks::<2>();
    let units: Vec<u16> = pairs
        .iter()
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// `ObjectCreate` (0xF745): `u32 guid, ModelData, PhysicsData, u32
/// weenieFlags, String16L name, ...`. Decodes the guid and, best-effort, the
/// name and position. Returns the guid even if the tail is truncated.
fn parse_object_create(data: &[u8]) -> ObjectCreate {
    if data.len() < 4 {
        return ObjectCreate { guid: 0, name: None, position: None };
    }
    let mut r = Reader::new(data);
    let guid = r.u32();

    // ModelData
    let (name, position) = match r.remaining() {
        n if n >= 4 => {
            let marker = r.u8();
            if marker == 0x11 {
                let nsub = r.u8() as usize;
                let ntex = r.u8() as usize;
                let nanim = r.u8() as usize;
                if nsub > 0 {
                    read_packed_dword(&mut r, 0x4000000);
                }
                for _ in 0..nsub {
                    read_packed_dword(&mut r, 0x4000000);
                    r.u8();
                    r.u8();
                }
                for _ in 0..ntex {
                    r.u8();
                    read_packed_dword(&mut r, 0x5000000);
                    read_packed_dword(&mut r, 0x5000000);
                }
                for _ in 0..nanim {
                    r.u8();
                    read_packed_dword(&mut r, 0x1000000);
                }
                r.align4();
                // PhysicsData
                let phys = r.u32();
                r.u32(); // state
                let mut position = None;
                if phys & 0x010000 != 0 {
                    let len = r.u32() as usize;
                    if len > 0 {
                        r.raw(len);
                        r.u32(); // isAutonomous
                    }
                }
                if phys & 0x020000 != 0 {
                    r.u32(); // placement
                }
                if phys & 0x008000 != 0 {
                    position = Some(Position::unpack(&mut r));
                }
                if phys & 0x000002 != 0 {
                    r.u32(); // MTable
                }
                if phys & 0x000800 != 0 {
                    r.u32(); // STable
                }
                if phys & 0x001000 != 0 {
                    r.u32(); // PeTable
                }
                if phys & 0x000001 != 0 {
                    r.u32(); // CSetup
                }
                if phys & 0x000020 != 0 {
                    r.u32();
                    r.u32();
                }
                if phys & 0x000040 != 0 {
                    let c = r.u32() as usize;
                    for _ in 0..c {
                        r.u32();
                        r.u32();
                    }
                }
                if phys & 0x000080 != 0 {
                    r.f32();
                }
                if phys & 0x000100 != 0 {
                    r.f32();
                }
                if phys & 0x000200 != 0 {
                    r.f32();
                }
                if phys & 0x040000 != 0 {
                    r.f32();
                }
                // Velocity (0x4), Acceleration (0x8), Omega (0x10) — each a
                // single Vec3, written once by the server.
                if phys & 0x4 != 0 {
                    r.f32();
                    r.f32();
                    r.f32();
                }
                if phys & 0x8 != 0 {
                    r.f32();
                    r.f32();
                    r.f32();
                }
                if phys & 0x10 != 0 {
                    r.f32();
                    r.f32();
                    r.f32();
                }
                if phys & 0x002000 != 0 {
                    r.u32();
                }
                if phys & 0x004000 != 0 {
                    r.f32();
                }
                for _ in 0..9 {
                    r.u16(); // sequence timestamps (ObjectPosition..ObjectInstance)
                }
                r.align4();
                r.u32(); // weenieFlags
                let n = r.string16l();
                (Some(n), position)
            } else {
                (None, None)
            }
        }
        _ => (None, None),
    };


    ObjectCreate { guid, name, position }
}

/// Read a `WritePackedDword` value, adding back the known type.
fn read_packed_dword(r: &mut Reader, known_type: u32) -> u32 {
    let low = r.u16() as u32;
    let base = if low & 0x8000 != 0 {
        let high = r.u16() as u32;
        (high << 16) | (low & 0x7FFF)
    } else {
        low
    };
    base | known_type
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_request_body_layout() {
        let b = login_request_body("1802", "hero", "pw");
        let mut r = Reader::new(&b);
        assert_eq!(r.string16l(), "1802");
        let len = r.u32() as usize;
        assert_eq!(len, r.remaining()); // len == everything after the u32
        assert_eq!(r.u32(), 2); // NetAuthType
        assert_eq!(r.u32(), 0); // authFlags
        assert_eq!(r.u32(), 0); // timestamp
        assert_eq!(r.string16l(), "hero");
        assert_eq!(r.string16l(), "hero");
        // password is String32L: u32 X (=n+1 for n<=255), packed length,
        // content, pad against 4+len.
        let x = r.u32() as usize;
        assert_eq!(x, 3); // "pw" has n=2, so X = n+1
        let packed = r.u8() as usize;
        assert_eq!(packed, 2);
        assert_eq!(r.raw(2), &b"pw"[..]);
        assert_eq!(r.remaining(), 2); // pad = (4 - n%4) % 4 = 2
        r.raw(2);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn character_create_body_layout() {
        let cc = CharacterCreate {
            account: "probe5".into(),
            heritage: 1,
            gender: 1,
            template: 0,
            attrs: [10, 10, 10, 10, 10, 10],
            name: "Rurik".into(),
            start_area: 0,
        };
        let b = cc.body();
        let mut r = Reader::new(&b);
        assert_eq!(r.string16l(), "probe5"); // account (must match session)
        assert_eq!(r.u32(), 1); // unknown constant
        assert_eq!(r.u32(), 1); // heritage
        assert_eq!(r.u32(), 1); // gender
        // 14 appearance u32, index 6 = 0xFFFFFFFF
        for i in 0..14 {
            let v = r.u32();
            assert_eq!(v, if i == 6 { 0xFFFF_FFFF } else { 0 }, "appearance[{i}]");
        }
        for _ in 0..6 {
            assert_eq!(r.f64(), 0.0);
        }
        assert_eq!(r.i32(), 0); // template
        for i in 0..6 {
            assert_eq!(r.u32(), 10, "attr[{i}]");
        }
        assert_eq!(r.u32(), 0); // slot
        assert_eq!(r.u32(), 0); // class
        let n = r.u32();
        assert_eq!(n, 55);
        for _ in 0..55 {
            assert_eq!(r.u32(), 0);
        }
        assert_eq!(r.string16l(), "Rurik");
        assert_eq!(r.u32(), 0); // start_area
        assert_eq!(r.u32(), 0); // admin
        assert_eq!(r.u32(), 0); // sentinel
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn game_action_talk_roundtrip() {
        let msg = game_action(7, action::TALK, &body_talk("hello"));
        let mut r = Reader::new(&msg);
        assert_eq!(r.u32(), opcode::GAME_ACTION);
        assert_eq!(r.u32(), 7); // client seq
        assert_eq!(r.u32(), action::TALK);
        assert_eq!(r.string16l(), "hello");
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn melee_attack_body() {
        let b = body_targeted_melee_attack(0x0011_0042);
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), 0x0011_0042);
        assert_eq!(r.u32(), 2); // height
        assert!((r.f32() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn tell_body_message_first() {
        let b = body_tell("psst", "Rurik");
        let mut r = Reader::new(&b);
        assert_eq!(r.string16l(), "psst");
        assert_eq!(r.string16l(), "Rurik");
    }

    #[test]
    fn update_position_parse() {
        let mut w = Writer::new();
        w.u32(opcode::UPDATE_POSITION);
        w.u32(0x0011_0001); // guid
        w.u32(0x0); // flags: all present (no orientation masked, no vel/placement)
        w.u32(0x8602_0101); // cell
        w.f32(1.0);
        w.f32(2.0);
        w.f32(3.0);
        w.f32(1.0); // w
        w.f32(0.0);
        w.f32(0.0);
        w.f32(0.0);
        w.u16(0);
        w.u16(0);
        w.u16(0);
        w.u16(0);
        let got = parse_incoming(&w.out).unwrap();
        match got {
            Incoming::UpdatePosition(u) => {
                assert_eq!(u.guid, 0x0011_0001);
                assert!((u.pos.x - 1.0).abs() < f32::EPSILON);
                assert!(u.vel.is_none());
            }
            _ => panic!("expected UpdatePosition"),
        }
    }

    #[test]
    fn appraise_identify_object_response_capture() {
        // Exact 124-byte S2C capture from the live server: Tab on a chest
        // sent GameAction 0xC8, the server answered GameEventIdentifyObject
        // Response (0xC9). The old parser skipped the leading target guid,
        // shifted every field, and panicked out of bounds on the bool table.
        let mut msg = vec![
            0xB0, 0xF7, 0x00, 0x00, // opcode GAME_EVENT
            0xD0, 0x01, 0x00, 0x50, // player guid 0x500001D0
            0x0F, 0x00, 0x00, 0x00, // event seq
            0xC9, 0x00, 0x00, 0x00, // event type IdentifyObjectResponse
            0xAE, 0x20, 0x60, 0x78, // target guid 0x786020AE (chest)
            0x0B, 0x00, 0x00, 0x00, // appraise flags Int|Bool|String
            0x01, 0x00, 0x00, 0x00, // success
            0x03, 0x00, 0x10, 0x00, // int table: count 3, buckets 16
            0x62, 0x00, 0x00, 0x00, 0xCE, 0x6B, 0xCF, 0x6A,
            0x13, 0x00, 0x00, 0x00, 0xC8, 0x00, 0x00, 0x00,
            0x05, 0x00, 0x00, 0x00, 0x70, 0x17, 0x00, 0x00,
            0x01, 0x00, 0x08, 0x00, // bool table: count 1, buckets 2048
            0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x01, 0x00, 0x08, 0x00, // string table: count 1, buckets 2048
            0x0E, 0x00, 0x00, 0x00,
        ];
        let text = b"Use this item to open it and see its contents.";
        assert_eq!(text.len(), 46);
        msg.extend_from_slice(&46u16.to_le_bytes());
        msg.extend_from_slice(text);

        let mut r = Reader::new(&msg);
        r.u32(); // opcode
        let ge = parse_game_event(&mut r).expect("game event");
        assert_eq!(r.remaining(), 0); // layout consumes the whole message
        assert_eq!(ge.event_type, 0xC9);
        let Event::Appraise(a) = ge.event else {
            panic!("expected Appraise");
        };
        assert_eq!(a.guid, 0x7860_20AE);
        assert!(a.success);
        assert_eq!(
            a.ints,
            vec![(0x62, 0x6A_CF_6B_CE_i32), (0x13, 0xC8), (0x05, 0x1770)]
        );
    }

    #[test]
    fn appraise_truncated_table_does_not_panic() {
        // Truncated appraise: int table claims 3 entries, only 4 bytes follow.
        // Must bail cleanly (guid intact, no ints), not panic.
        let msg: &[u8] = &[
            0xB0, 0xF7, 0x00, 0x00,
            0xD0, 0x01, 0x00, 0x50,
            0x0F, 0x00, 0x00, 0x00,
            0xC9, 0x00, 0x00, 0x00,
            0xAE, 0x20, 0x60, 0x78,
            0x0B, 0x00, 0x00, 0x00,
            0x01, 0x00, 0x00, 0x00,
            0x03, 0x00, 0x10, 0x00, // count 3 …
            0x62, 0x00, 0x00, 0x00, // … but only one key fits
        ];
        let mut r = Reader::new(msg);
        r.u32();
        let ge = parse_game_event(&mut r).expect("game event");
        let Event::Appraise(a) = ge.event else {
            panic!("expected Appraise");
        };
        assert_eq!(a.guid, 0x7860_20AE);
        assert!(a.ints.is_empty());
    }

    #[test]
    fn appraise_creature_profile_health() {
        // Creature appraise (bunny): Int|Bool|Creature flags with
        // ShowAttributes|HasBuffsDebuffs; health fields and the variable
        // creature sections must line up byte-for-byte.
        let mut w = Writer::new();
        w.u32(opcode::GAME_EVENT);
        w.u32(0x5000_01D0); // player guid
        w.u32(1); // event seq
        w.u32(0xC9); // IdentifyObjectResponse
        w.u32(0x0028_0042); // target guid (bunny)
        w.u32(0x103); // flags Int|Bool|Creature
        w.u32(1); // success
        w.u16(1);
        w.u16(4); // int table
        w.u32(0x16);
        w.i32(120);
        w.u16(1);
        w.u16(4); // bool table
        w.u32(1);
        w.u32(1);
        w.u32(0x9); // cflags ShowAttributes|HasBuffsDebuffs
        w.u32(35); // health
        w.u32(50); // health_max
        for _ in 0..10 {
            w.u32(0); // attributes
        }
        w.u16(0);
        w.u16(0); // buff masks

        let mut r = Reader::new(&w.out);
        r.u32();
        let ge = parse_game_event(&mut r).expect("game event");
        assert_eq!(r.remaining(), 0);
        let Event::Appraise(a) = ge.event else {
            panic!("expected Appraise");
        };
        assert_eq!(a.guid, 0x0028_0042);
        assert_eq!(a.health, 35);
        assert_eq!(a.health_max, 50);
    }

    #[test]
    fn packed_dword_roundtrip() {
        // small value
        let mut w = Writer::new();
        w.u16(0x1234);
        let mut r = Reader::new(&w.out);
        assert_eq!(read_packed_dword(&mut r, 0), 0x1234);
        // large value: standard retail packing (flag in low u16, high 16 in
        // the second u16)
        let v: u32 = 0x1234_5678;
        let packed = ((v >> 16) << 16) | 0x8000 | (v & 0x7FFF);
        let mut w2 = Writer::new();
        w2.u32(packed);
        let mut r2 = Reader::new(&w2.out);
        assert_eq!(read_packed_dword(&mut r2, 0), v);
    }

    #[test]
    fn packed_len_roundtrip() {
        let mut w = Writer::new();
        write_packed_len(&mut w, 5);
        let mut r = Reader::new(&w.out);
        assert_eq!(read_packed_len(&mut r), Some(5));
        assert_eq!(r.remaining(), 0);

        let mut w2 = Writer::new();
        write_packed_len(&mut w2, 0x123);
        let mut r2 = Reader::new(&w2.out);
        assert_eq!(read_packed_len(&mut r2), Some(0x123));
    }
}
