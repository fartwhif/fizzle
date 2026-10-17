//! The application state machine: auth, character selection/creation, and the
//! in-world session (movement, combat, chat, keepalive).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, ModifierKeyCode};

use acproto::codec::Vec3;
use acproto::messages::{self, combat_mode, Event, GameEvent, Incoming};
use crate::movement::{HeldKeys, Movement, TICK};
use crate::net::{Net, NetEvent};
use acgeo::{World3D, WorldProbe};
use crate::themes::Theme;
use crate::world::{ChatStyle, ItemState, World};
/// Auto-reconnect backoff (Stockpile parity): start 10 s, double per failed
/// attempt, cap 5 min. A successful login resets the sequence.
const RECONNECT_MIN_MS: u64 = 10_000;
const RECONNECT_MAX_MS: u64 = 300_000;

/// Top-level session phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No connection.
    Idle,
    /// Login sent, waiting for `ConnectRequest` / character list.
    Connecting,
    /// Character list shown.
    CharSelect,
    /// Character-creation form.
    CharCreate,
    /// World-entry sequence in flight.
    Entering,
    /// In the world.
    InWorld,
    /// Logging off.
    LoggingOut,
    /// Session ended (server-side death or exit). The user reconnects with
    /// Enter (retail `gmDisconnectedUI`, mode 0x10000002).
    Disconnected,
}

/// The Esc menu and its subviews (the retail menu minus the server,
/// account, and plugin management — the profile store and the CLI own
/// that).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuView {
    /// The top-level menu list.
    Menu,
    /// The theme picker (borders + colors).
    Themes,
    /// The right-column multi-widget picker.
    Widgets,
    /// The world-pane render mode (tactical / 3d).
    WorldView,
    /// The key-map help.
    Help,
}

/// Character-creation form state.
#[derive(Clone, Debug)]
pub struct CharCreateForm {
    pub name: String,
    pub heritage_idx: usize,
    pub template_idx: usize,
    /// 1 = male, 0 = female.
    pub gender: u32,
    /// Index into the heritage's `start_areas`.
    pub start_area_idx: usize,
    pub attrs: [u32; 6],
    pub error: Option<String>,
}

impl CharCreateForm {
    /// The selected heritage (static data; does not borrow the form).
    #[must_use]
    pub fn heritage(&self) -> &'static acproto::chargen::Heritage {
        &acproto::chargen::HERITAGES[self.heritage_idx]
    }

    /// The selected template (static data; does not borrow the form).
    #[must_use]
    pub fn template(&self) -> &'static acproto::chargen::Template {
        let h = self.heritage();
        &h.templates[self.template_idx.min(h.templates.len() - 1)]
    }

    fn new() -> Self {
        let heritage = acproto::chargen::HERITAGES[0];
        Self {
            name: String::new(),
            heritage_idx: 0,
            template_idx: 0,
            gender: 1,
            start_area_idx: 0,
            attrs: heritage.templates[0].attributes,
            error: None,
        }
    }

    #[must_use]
    pub fn attr_sum(&self) -> u32 {
        self.attrs.iter().sum()
    }

    /// Build the `CharacterCreate` (0xF656) wire body for `account`.
    #[must_use]
    pub fn body(&self, account: &str) -> Vec<u8> {
        let h = self.heritage();
        let start_area = *h.start_areas.get(self.start_area_idx).unwrap_or(&0);
        messages::CharacterCreate {
            account: account.to_string(),
            heritage: h.id,
            gender: self.gender,
            template: self.template_idx as i32,
            attrs: self.attrs,
            name: self.name.clone(),
            start_area,
        }
        .body()
    }
}

impl Default for CharCreateForm {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a key code is a movement key (held-key, not a command).
#[must_use]
pub fn is_move_key(c: KeyCode) -> bool {
    matches!(
        c,
        KeyCode::Char('w')
            | KeyCode::Char('a')
            | KeyCode::Char('s')
            | KeyCode::Char('d')
            | KeyCode::Char('z')
            | KeyCode::Char('c')
            | KeyCode::Char('x')
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
    )
}

/// The 10 hotkey slot labels (1-9, then 0).
const SLOTS: [char; 10] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];

/// The label of hotkey slot `i` (0-based).
#[must_use]
fn slot_label(i: usize) -> char {
    SLOTS[i % 10]
}

/// Rotate a model-space AABB's 8 corners by the quaternion, translate by the
/// world origin, and re-bound: the world-space collision slab `World3D`
/// expects (its `setup_aabb` returns the model-space box).
#[must_use]
fn transform_aabb(aabb: [f32; 6], origin: [f32; 3], quat: [f32; 4]) -> [f32; 6] {
    let (w, qx, qy, qz) = (quat[0], quat[1], quat[2], quat[3]);
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for cx in [aabb[0], aabb[3]] {
        for cy in [aabb[1], aabb[4]] {
            for cz in [aabb[2], aabb[5]] {
                // t = 2 * cross(qv, p); p' = p + w*t + cross(qv, t)
                let tx = 2.0 * (qy * cz - qz * cy);
                let ty = 2.0 * (qz * cx - qx * cz);
                let tz = 2.0 * (qx * cy - qy * cx);
                let rx = cx + w * tx + (qy * tz - qz * ty) + origin[0];
                let ry = cy + w * ty + (qz * tx - qx * tz) + origin[1];
                let rz = cz + w * tz + (qx * ty - qy * tx) + origin[2];
                mn[0] = mn[0].min(rx);
                mn[1] = mn[1].min(ry);
                mn[2] = mn[2].min(rz);
                mx[0] = mx[0].max(rx);
                mx[1] = mx[1].max(ry);
                mx[2] = mx[2].max(rz);
            }
        }
    }
    [mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]]
}

/// The full application.
pub struct App {
    pub net: Option<Net>,
    pub phase: Phase,
    pub status: String,
    pub host: String,
    pub game_port: u16,
    pub account: String,
    pub password: String,
    pub server_name: String,
    pub char_list: Vec<messages::CharacterListEntry>,
    pub char_sel: usize,
    pub create_form: CharCreateForm,
    pub world: World,
    pub movement: Option<Movement>,
    pub self_name: String,
    pub self_guid: Option<u32>,
    // Input
    pub keys_down: HashSet<KeyCode>,
    pub shift_held: bool,
    pub chat_buf: String,
    pub chat_focused: bool,
    pub run_lock: bool,
    pub combat_mode: u32,
    pub targets: Vec<u32>,
    // Kind of the current target list (0 creature, 1 object, 2 player).
    pub target_kind: u8,
    // When set (R on an NPC), chat lines send as TalkDirect to this guid.
    pub talk_to: Option<u32>,
    // Timers / liveness (the retail timer set — see step())
    pub last_rx: Instant,
    pub last_tick: Instant,
    pub pending_jump: bool,
    pub pending_teleport: bool,
    pub client_seq: u32,
    pub ping_ms: Option<f32>,
    pub crypto_bad: u32,
    /// NetID from the S2C `ConnectRequest`; `None` until the handshake. The
    /// server assigns the first free `sessionMap` slot, which is often `0`,
    /// so presence (not a nonzero value) is what marks an established
    /// session — this gates the PAK keepalive, the 40 s link check, and the
    /// exit Disconnect broadcast.
    pub net_id: Option<u16>,
    /// When the `LoginRequest` was last (re)sent; `login_retries` = sends so
    /// far (retail `LogonUseTime`, 0x544A10: every 2.0 s, max 20).
    pub login_sent_at: Option<Instant>,
    pub login_retries: u32,
    /// `ConnectResponse` cookie still awaiting a matching `CharacterList`.
    /// The server sets the session's auth state asynchronously after the
    /// `ConnectRequest` is flushed, so the first (immediate) response can be
    /// dropped; we retransmit every 0.333 s (retail net state 4, 0x545450)
    /// until the handshake completes. `None` once done.
    pub connect_cookie: Option<u64>,
    /// When the `ConnectResponse` was last (re)sent.
    pub connect_sent_at: Option<Instant>,
    /// When the PAK keepalive (0x4000) was last sent (retail: every 2.0 s).
    pub last_pak: Option<Instant>,
    /// (packet seq, time) of the PAK awaiting its S2C `Ack` (ping probe).
    pub last_pak_probe: Option<(u32, Instant)>,
    /// Last run of the 40 s no-data link check (retail: every ~4 s).
    pub last_link_check: Option<Instant>,
    /// World-entry start (retail 110 s enter-world deadline).
    pub enter_started_at: Option<Instant>,
    /// When the logout (0xF653) was sent (25 s local fallback).
    pub logoff_sent_at: Option<Instant>,
    pub quit: bool,
    /// The Esc menu (Some) and its subview; None = the world UI.
    pub menu_view: Option<MenuView>,
    /// Selection cursor in the menu list (themes, widgets, worldview, glow,
    /// help, logout, quit).
    pub menu_sel: usize,
    /// Selection cursors in the menu subviews.
    pub theme_sel: usize,
    pub widget_sel: usize,
    pub worldview_sel: usize,
    /// Help scroll offset (first visible line; clamped at draw).
    pub help_scroll: usize,
    /// Selected slot on the "Keys:" hotkey bar (the slots are empty; the
    /// bar mirrors the retail power bar).
    pub hotkey_sel: usize,
    /// The item guid being mouse-dragged (None = no drag in flight).
    pub drag: Option<u32>,
    /// Last reported mouse position (screen cell); the UI draws a marker.
    pub mouse_xy: Option<(u16, u16)>,
    /// A pending vendor order asked for a second Esc (retail's confirm
    /// dialog); any other input disarms it.
    pub confirm_close_vendor: bool,
    /// The item guid selected in the item widgets (inventory / vendor /
    /// trade / loot), plus which list it came from: 0 none, 1 inventory,
    /// 2 vendor, 3 trade, 4 loot. The widgets' actions act on it.
    pub item_sel: Option<u32>,
    pub item_sel_ctx: u8,
    /// The previous mouse click (time + cell) for double-click detection.
    pub last_click_at: Option<Instant>,
    pub last_click_pos: Option<(u16, u16)>,
    /// Wheel scroll offset (first visible row) of the active multi-widget
    /// list; reset when the active widget changes.
    pub list_scroll: usize,
    /// If set, auto-select this character name on the character list.
    pub auto_character: Option<String>,
    // Retail-parity session state:
    /// The 3D world (retail DATs); `None` when the DATs are unavailable.
    pub world3d: Option<World3D>,
    /// UI theme.
    pub theme: Theme,
    /// In-world view: true = 3D renderer, false = tactical radar.
    pub world_view: bool,
    /// Draw the 3D actor glow halos.
    pub glow_actors: bool,
    /// Retail DAT directory override (None = auto-resolve).
    pub dat_dir: Option<String>,
    /// Selected multi-widget index (0..=13).
    pub multi_sel: usize,
    // Auto-reconnect watchdog:
    pub reconnect_at: Option<Instant>,
    pub reconnect_delay: u64,
    /// Auto-reconnect is armed (the server-down watchdog fired): a backoff
    /// retry is scheduled on the disconnected screen.
    pub auto_reconnect: bool,
    // Far-cull throttle (≤1/s).
    pub last_cull: Option<Instant>,
}

impl App {
    #[must_use]
    pub fn new(host: &str, game_port: u16, account: &str, password: &str) -> Self {
        Self {
            net: None,
            phase: Phase::Idle,
            status: "Not connected.".into(),
            host: host.to_string(),
            game_port,
            account: account.to_string(),
            password: password.to_string(),
            server_name: String::new(),
            char_list: Vec::new(),
            char_sel: 0,
            create_form: CharCreateForm::new(),
            world: World::new(),
            movement: None,
            self_name: String::new(),
            self_guid: None,
            keys_down: HashSet::new(),
            shift_held: false,
            chat_buf: String::new(),
            chat_focused: false,
            run_lock: true,
            combat_mode: combat_mode::NON_COMBAT,
            targets: Vec::new(),
            target_kind: 0,
            talk_to: None,
            last_rx: Instant::now(),
            last_tick: Instant::now(),
            pending_jump: false,
            pending_teleport: false,
            client_seq: 0,
            ping_ms: None,
            crypto_bad: 0,
            net_id: None,
            login_sent_at: None,
            login_retries: 0,
            connect_cookie: None,
            connect_sent_at: None,
            last_pak: None,
            last_pak_probe: None,
            last_link_check: None,
            enter_started_at: None,
            logoff_sent_at: None,
            quit: false,
            menu_view: None,
            menu_sel: 0,
            theme_sel: 0,
            widget_sel: 0,
            worldview_sel: 0,
            help_scroll: 0,
            hotkey_sel: 0,
            drag: None,
            mouse_xy: None,
            confirm_close_vendor: false,
            item_sel: None,
            item_sel_ctx: 0,
            last_click_at: None,
            last_click_pos: None,
            list_scroll: 0,
            auto_character: None,
            world3d: None,
            theme: Theme::Custom,
            world_view: false,
            glow_actors: true,
            dat_dir: None,
            multi_sel: 0,
            reconnect_at: None,
            reconnect_delay: 10_000,
            auto_reconnect: false,
            last_cull: None,
        }
    }

    /// Open a connection and send the login. Failures land on the
    /// disconnected screen; the user retries with Enter.
    pub fn start(&mut self) {
        self.do_connect();
    }

    fn do_connect(&mut self) {
        let net = match Net::connect(&self.host, self.game_port) {
            Ok(net) => net,
            Err(e) => {
                self.phase = Phase::Disconnected;
                self.status = format!("Connect failed: {e}");
                // Keep the auto-reconnect backoff going on a failed connect.
                if self.auto_reconnect {
                    self.reconnect_at =
                        Some(Instant::now() + Duration::from_millis(self.reconnect_delay));
                }
                return;
            }
        };
        self.net = Some(net);
        self.phase = Phase::Connecting;
        self.status = "Logging in…".into();
        self.last_rx = Instant::now();
        self.net_id = None;
        self.login_retries = 1;
        self.login_sent_at = Some(Instant::now());
        self.connect_cookie = None;
        self.connect_sent_at = None;
        self.last_pak = None;
        self.last_pak_probe = None;
        self.last_link_check = Some(Instant::now());
        self.enter_started_at = None;
        self.logoff_sent_at = None;
        self.ping_ms = None;
        if let Some(net) = self.net.as_mut() {
            if let Err(e) = net.send_login(&self.account, &self.password) {
                self.server_died(&format!("Login send failed: {e}"));
            }
        }
    }

    /// Drop the transport (state unchanged).
    fn close_net(&mut self) {
        if let Some(net) = self.net.take() {
            net.close();
        }
    }

    /// The session ended unexpectedly. Retail parity: the client posts the
    /// ServerDied notice (`gmUIFlow::RecvNotice_ServerDied`, 0x47A9A0) and
    /// shows the disconnected screen (`gmDisconnectedUI`, mode 0x10000002);
    /// its "Reconnect" button (0x4E99C0) re-runs connect + login, which the
    /// user initiates — unless auto-reconnect is armed, in which case a
    /// backoff retry is scheduled here (see `step`).
    fn server_died(&mut self, reason: &str) {
        self.close_net();
        self.phase = Phase::Disconnected;
        self.status = reason.to_string();
        self.movement = None;
        self.world = World::new();
        self.targets.clear();
        self.combat_mode = combat_mode::NON_COMBAT;
        self.keys_down.clear();
        self.chat_buf.clear();
        self.chat_focused = false;
        self.pending_teleport = false;
        self.menu_view = None;
        self.net_id = None;
        self.login_sent_at = None;
        self.login_retries = 0;
        self.connect_cookie = None;
        self.connect_sent_at = None;
        self.last_pak = None;
        self.last_pak_probe = None;
        self.last_link_check = None;
        self.enter_started_at = None;
        self.logoff_sent_at = None;
        // Auto-reconnect (Stockpile parity): a server-down arms a backoff
        // retry; the disconnected screen fires it (see `step`), or the user's
        // Enter takes over.
        if self.auto_reconnect {
            self.reconnect_at =
                Some(Instant::now() + Duration::from_millis(self.reconnect_delay));
            self.status =
                format!("server down: {reason}; retrying in {} s", self.reconnect_delay / 1000);
        }
    }

    /// User-initiated exit (called once from the event loop before the
    /// process ends): broadcast the retail `Disconnect` (0x8000) so the
    /// server drops the session immediately (`Client::CleanupNet`, 0x411D00,
    /// → `ClientNet::LogOffServer`, 0x543EF0) instead of letting it linger
    /// through the 15 s auth / 60 s session timeout — the lingering stale
    /// session is what made quick restarts hit "account in use".
    pub fn teardown(&mut self) {
        if self.phase == Phase::InWorld {
            // Request the character save first (retail's clean-exit path
            // logs the character off); the broadcast tears the session down.
            self.send_msg(messages::opcode::CHARACTER_LOGOFF, &[]);
        }
        if self.net_id.is_some() {
            if let Some(net) = self.net.as_ref() {
                let _ = net.send_disconnect();
            }
        }
    }

    // ============================================================= net events

    pub fn handle_net_event(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::ConnectRequest { cookie, net_id, iteration, .. } => {
                if self.phase != Phase::Connecting {
                    return;
                }
                // Handshake accepted (retail net state 4, 0x545450): resend
                // the referral cookie every 0.333 s until the server
                // completes auth. No handshake deadline of our own (retail
                // has none) — the 40 s no-data check in step() is the bound.
                self.net_id = Some(net_id as u16);
                self.connect_cookie = Some(cookie);
                self.connect_sent_at = Some(Instant::now());
                if let Some(net) = self.net.as_mut() {
                    if let Err(e) = net.send_connect_response(cookie, net_id, iteration) {
                        self.server_died(&format!("ConnectResponse failed: {e}"));
                        return;
                    }
                }
                self.last_rx = Instant::now();
                self.status = "Connected — waiting for character list…".into();
            }
            NetEvent::GameMessage(data) => {
                self.last_rx = Instant::now();
                if self.phase == Phase::Connecting {
                    // Any S2C game message means the server matched our
                    // ConnectResponse; stop retransmitting.
                    self.connect_cookie = None;
                }
                self.handle_game_message(&data);
            }
            NetEvent::Control { ack, .. } => {
                self.last_rx = Instant::now();
                // Ping from the server's 2 s S2C `Ack` (u32 = last received
                // C2S packet seq): the first ack covering our PAK's seq
                // bounds that PAK's round trip.
                if let Some(ack) = ack {
                    if let Some((seq, sent_at)) = self.last_pak_probe.take() {
                        if ack >= seq {
                            self.ping_ms = Some(sent_at.elapsed().as_secs_f32() * 1000.0);
                        } else {
                            self.last_pak_probe = Some((seq, sent_at));
                        }
                    }
                }
            }
            NetEvent::CryptoMismatch { .. } => {
                self.last_rx = Instant::now();
                self.crypto_bad += 1;
            }
            NetEvent::Debug(msg) => {
                // A local parse problem is not proof the server is alive.
                self.status = msg;
            }
        }
    }

    fn handle_game_message(&mut self, data: &[u8]) {
        let incoming = match messages::parse_incoming(data) {
            Some(i) => i,
            None => {
                self.status = format!("Unparsed S2C message ({} bytes)", data.len());
                return;
            }
        };
        self.dispatch(incoming);
    }

    // ================================================================ dispatch

    fn dispatch(&mut self, inb: Incoming) {
        match inb {
            Incoming::CharacterList(list) => {
                // A character list means the login succeeded: reset the
                // auto-reconnect backoff sequence.
                self.auto_reconnect = false;
                self.reconnect_delay = RECONNECT_MIN_MS;
                self.reconnect_at = None;
                self.char_list = list.characters;
                self.char_sel = 0;
                if matches!(self.phase, Phase::Connecting | Phase::CharSelect) {
                    self.phase = Phase::CharSelect;
                    self.status = if self.char_list.is_empty() {
                        "No characters. Press C to create one.".into()
                    } else {
                        "Select a character (↑/↓ + Enter), C create, Esc quit.".into()
                    };
                    if let Some(want) = self.auto_character.clone() {
                        if let Some(idx) = self
                            .char_list
                            .iter()
                            .position(|c| c.name.eq_ignore_ascii_case(&want))
                        {
                            self.char_sel = idx;
                            self.auto_character = None;
                            self.select_character(idx);
                        }
                    }
                }
            }
            Incoming::CharacterCreateResponse(resp) => match resp.status {
                1 => {
                    self.char_list.push(messages::CharacterListEntry {
                        guid: resp.guid,
                        name: resp.name.unwrap_or_default(),
                        time_since_delete: 0,
                    });
                    self.char_sel = self.char_list.len() - 1;
                    self.create_form.error = None;
                    self.phase = Phase::CharSelect;
                    self.status = "Character created.".into();
                    if let Some(want) = self.auto_character.clone() {
                        if self.char_list[self.char_sel].name.eq_ignore_ascii_case(&want) {
                            self.auto_character = None;
                            self.select_character(self.char_sel);
                        }
                    }
                }
                other => {
                    self.create_form.error = Some(create_error_text(other).to_string());
                    self.phase = Phase::CharCreate;
                }
            },
            Incoming::ServerName(sn) => {
                self.server_name = sn.name;
            }
            Incoming::EnterWorldServerReady => {
                if let Some(guid) = self.self_guid {
                    let body = messages::character_enter_world_body(guid, &self.account.to_lowercase());
                    self.send_msg(messages::opcode::CHARACTER_ENTER_WORLD, &body);
                }
            }
            Incoming::DddInterrogation => {}
            Incoming::PlayerCreate(guid) => {
                self.world.self_guid = self.self_guid;
                self.world.player_create(guid);
            }
            Incoming::ObjectCreate(o) => {
                let position = o.position;
                let item = ItemState {
                    guid: o.guid,
                    name: o.name.clone().unwrap_or_default(),
                    item_type: o.item_type,
                    value: o.value,
                    stack_size: o.stack_size,
                    items_capacity: o.items_capacity,
                    cur_loc: o.cur_loc,
                    valid_locs: o.valid_locs,
                    container_id: o.container_id,
                    wielder_id: o.wielder_id,
                    mtable_id: o.mtable_id,
                    setup_id: o.setup_id,
                    model_parts: o.model_parts.clone(),
                    icon_id: 0,
                    coin_value: 0,
                    motion_cmd: o.motion_cmd,
                    physics_state: o.physics_state,
                    is_vendor: o.is_vendor,
                };
                self.world.object_create(
                    o.guid,
                    o.name,
                    position.map(|p| p.cell),
                    position.map(|p| p.pos),
                    Some(&item),
                    position.map(|p| [p.rot.w, p.rot.x, p.rot.y, p.rot.z]),
                );
                // Track player-owned items in the inventory (paper doll /
                // main pack): equipped when worn, else in the main pack (a
                // capacity-bearing container becomes a side pack).
                if let Some(self_guid) = self.self_guid {
                    let owned = o.wielder_id == self_guid
                        || o.container_id == self_guid
                        || (o.container_id != 0 && self.world.inventory.packs.contains(&o.container_id));
                    if owned {
                        if o.cur_loc != 0 {
                            self.world.upsert_equipped_item(item.clone());
                        } else {
                            self.world.upsert_pack_item(item.clone());
                            if o.item_type == crate::world::ITEM_TYPE_CONTAINER && o.items_capacity > 0 {
                                if !self.world.inventory.packs.contains(&o.guid) {
                                    self.world.inventory.packs.push(o.guid);
                                }
                            }
                        }
                    }
                }
                // Register a door's collision slab with the 3D world; a
                // closed door blocks the movement sim, an open one does not.
                if o.item_type == crate::world::ITEM_TYPE_DOOR {
                    if let Some(w) = self.world3d.as_mut() {
                        let aabb = o
                            .setup_id
                            .and_then(|s| w.setup_aabb(s))
                            .and_then(|m| {
                                position.map(|p| {
                                    transform_aabb(
                                        m,
                                        [p.pos.x, p.pos.y, p.pos.z],
                                        [p.rot.w, p.rot.x, p.rot.y, p.rot.z],
                                    )
                                })
                            });
                        let open = o.motion_cmd == crate::world::DOOR_MOTION_ON
                            || (o.physics_state & crate::world::PHYSICS_ETHEREAL) != 0;
                        w.set_door(o.guid, aabb, open);
                    }
                }
                // The player's own ObjectCreate carries the spawn position —
                // the server sends no UpdatePosition for the player at world
                // entry, so this is the authoritative "you are here".
                if Some(o.guid) == self.self_guid
                    && self.phase == Phase::Entering
                    && self.movement.is_none()
                {
                    if let Some(p) = position {
                        self.movement = Some(Movement::new(p.cell, p.pos));
                        self.phase = Phase::InWorld;
                        self.status = "In world.".into();
                        // Tell the server we exited portal space. Until
                        // LoginComplete arrives the player stays in the
                        // pink-bubble "Teleporting" state: the server
                        // ignores movement/combat and logs the session
                        // out after 5 minutes.
                        self.send_action(messages::action::LOGIN_COMPLETE, &[]);
                    }
                }
            }
            Incoming::ObjectDelete(guid) => {
                self.world.object_delete(guid);
                // A vanished door drops its collision slab.
                if let Some(w) = self.world3d.as_mut() {
                    w.remove_door(guid);
                }
            }
            Incoming::UpdatePosition(up) => {
                if Some(up.guid) == self.self_guid {
                    if self.movement.is_none() {
                        self.movement = Some(Movement::new(up.cell, up.pos));
                        self.world.self_guid = Some(up.guid);
                        if self.phase == Phase::Entering {
                            self.phase = Phase::InWorld;
                            self.status = "In world.".into();
                            self.send_action(messages::action::LOGIN_COMPLETE, &[]);
                        }
                    } else if let Some(mv) = self.movement.as_mut() {
                        mv.apply_correction(up.cell, &up.pos, self.pending_teleport);
                        if self.pending_teleport {
                            self.pending_teleport = false;
                        }
                    }
                } else {
                    self.world.update_position(up.guid, up.cell, up.pos, Some([up.rot.w, up.rot.x, up.rot.y, up.rot.z]));
                }
            }
            Incoming::PlayerTeleport(guid) => {
                if Some(guid) == self.self_guid {
                    self.pending_teleport = true;
                }
            }
            Incoming::VectorUpdate(_) => {}
            Incoming::Motion(m) => {
                // Door On/Off arrives in the retail motion command list
                // (DoorOn 0x4000000b / DoorOff 0x4000000c). Non-door motion is
                // otherwise unused (positions come from UpdatePosition).
                let (is_door, cur) = match self.world.entities.get(&m.guid) {
                    Some(e) if e.is_door() => (true, e.door_open()),
                    _ => (false, false),
                };
                if !is_door {
                    return;
                }
                let open = if m.commands.contains(&crate::world::DOOR_MOTION_ON) {
                    true
                } else if m.commands.contains(&crate::world::DOOR_MOTION_OFF) {
                    false
                } else {
                    cur
                };
                if open == cur {
                    return;
                }
                if let Some(ent) = self.world.entities.get_mut(&m.guid) {
                    ent.motion_cmd = if open {
                        crate::world::DOOR_MOTION_ON
                    } else {
                        crate::world::DOOR_MOTION_OFF
                    };
                }
                if let Some(w) = self.world3d.as_mut() {
                    let (setup, origin, rot) = self
                        .world
                        .entities
                        .get(&m.guid)
                        .map_or((None, [0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]), |e| {
                            (e.setup_id, [e.pos.x, e.pos.y, e.pos.z], e.rot)
                        });
                    let aabb = setup
                        .and_then(|s| w.setup_aabb(s))
                        .map(|m| transform_aabb(m, origin, rot));
                    w.set_door(m.guid, aabb, open);
                }
            }
            Incoming::PlayerKilled { message, victim, killer } => {
                if Some(victim) == self.self_guid {
                    // Retail swallows its own death message (no death screen);
                    // the dead state drives the HUD and input gating.
                    self.world.dead = true;
                    self.kill_input();
                } else if Some(killer) != self.self_guid {
                    // Someone else died nearby (retail scrolls it; our own
                    // kills are covered by the KillerNotification).
                    self.world.chat.push("", &message, ChatStyle::System);
                }
            }
            Incoming::VitalUpdate { vital, value } => {
                if vital == messages::vital::HEALTH && value == 0 && !self.world.dead {
                    self.kill_input();
                }
                self.world.set_vital(vital, value);
            }
            Incoming::GameEvent(ge) => self.handle_game_event(ge),
            Incoming::TurbineChat(tc) => {
                if tc.is_ack {
                    return;
                }
                let ch = channel_name(tc.channel);
                self.world.chat.push(&format!("[{ch}] {}", tc.sender), &tc.message, ChatStyle::Channel);
            }
            Incoming::HearSpeech { text, sender, .. } => {
                if !sender.is_empty() && sender == self.self_name {
                    self.world.chat.push("You", &text, ChatStyle::Own);
                } else {
                    self.world.chat.push(&sender, &text, ChatStyle::Speech);
                }
            }
            Incoming::ServerMessage { text, .. } => {
                self.world.chat.push("Server", &text, ChatStyle::System);
            }
            Incoming::CharacterError(e) => {
                // Retail (CPlayerSystem::Handle_CharacterError, 0x55D5D0):
                // clear the entering state, post the notice, let the user
                // retry — no disconnect, no auto-reconnect.
                let text = character_error_text(e);
                self.world.chat.push("Error", &format!("CharacterError {e}: {text}"), ChatStyle::Error);
                self.status = format!("CharacterError {e}: {text}");
                if self.phase == Phase::Entering {
                    self.phase = Phase::CharSelect;
                    self.enter_started_at = None;
                }
                if e == 1 {
                    // "Account in use": this fork's server kills both the
                    // stale and the new session and then goes silent
                    // (verified live), so this session is dead. Go straight
                    // to the disconnected screen (retail's 40 s no-data
                    // detector reaches the same verdict); the user retries
                    // with Enter.
                    self.server_died("Account in use — a previous session is still on the server");
                }
            }
            Incoming::AccountBoot { reason } => {
                let reason = reason.unwrap_or_default();
                self.world.chat.push("Server", &format!("Booted from server: {reason}"), ChatStyle::System);
                self.server_died(&format!("Booted from server: {reason}"));
            }
            Incoming::AccountBanned { seconds, reason } => {
                let reason = reason.unwrap_or_default();
                self.server_died(&format!("Banned for {seconds}s: {reason}"));
            }
            Incoming::CharacterLogOff => {
                // In-world logout complete (retail: `ExecuteLogOff` 0x55D780
                // → `ExitWorldDisconnect` 0x541E00 keeps the logon
                // connection): back to character select, session still
                // alive — the PAK keepalive continues, no re-login.
                // Clear in-world transient state so nothing leaks into the
                // next entry: a stale `menu = true` made the first Esc of
                // the new world *close* the phantom menu, so `q` toggled
                // run instead of quitting (held keys and chat focus leak
                // the same way).
                self.phase = Phase::CharSelect;
                self.world = World::new();
                self.movement = None;
                self.targets.clear();
                self.combat_mode = combat_mode::NON_COMBAT;
                self.keys_down.clear();
                self.chat_buf.clear();
                self.chat_focused = false;
                self.pending_jump = false;
                self.pending_teleport = false;
                self.menu_view = None;
                self.logoff_sent_at = None;
                self.status = "Logged out.".into();
            }
            Incoming::CharacterDelete => {}
            Incoming::HearRangedSpeech { text, sender, .. } => {
                if !sender.is_empty() && sender == self.self_name {
                    self.world.chat.push("You", &text, ChatStyle::Own);
                } else {
                    self.world
                        .chat
                        .push(&format!("{sender} (distant)"), &text, ChatStyle::Speech);
                }
            }
            Incoming::PrivateUpdatePropertyInt { prop, value } => {
                if prop == messages::property_int::COIN_VALUE {
                    self.world.inventory.coin_value = value as i64;
                }
            }
            Incoming::Other(op) => {
                self.status = format!("S2C opcode {op:#06x}");
            }
        }
    }

    fn handle_game_event(&mut self, ge: GameEvent) {
        match ge.event {
            Event::PlayerDescription(pd) => {
                self.world.set_vitals_from_description(&pd);
                self.world.apply_player_description_tail(&pd);
                if let Some((_, name)) = pd.strings.iter().find(|(id, _)| *id == messages::property_string::NAME) {
                    self.self_name = name.clone();
                }
            }
            Event::Appraise(a) => {
                self.world.apply_appraise(&a);
            }
            Event::UpdateHealth { guid, health } => {
                self.world.update_health_event(guid, health);
            }
            Event::DefenderNotification { attacker, damage, critical } => {
                self.world.apply_defender(damage);
                let crit = if critical { " (critical)" } else { "" };
                self.world
                    .chat
                    .push("Combat", &format!("You take {damage} damage from {attacker}{crit}"), ChatStyle::Combat);
            }
            Event::AttackerNotification { name, damage, critical } => {
                let crit = if critical { " (critical)" } else { "" };
                self.world
                    .chat
                    .push("Combat", &format!("You deal {damage} damage to {name}{crit}"), ChatStyle::Combat);
            }
            Event::EvasionAttacker { name } => {
                self.world
                    .chat
                    .push("Combat", &format!("You evade an attack from {name}"), ChatStyle::Combat);
            }
            Event::KillerNotification { text } => {
                self.world.chat.push("Combat", &text, ChatStyle::Combat);
            }
            Event::SetTurbineChatChannels(_) => {}
            Event::Tell { text, name, .. } => {
                self.world.chat.push(&format!("Tell {name}"), &text, ChatStyle::Tell);
            }
            Event::PingResponse => {}
            Event::Unknown => {}
            Event::PopupString { text } => {
                self.world.chat.push("System", &text, ChatStyle::System);
            }
            Event::FriendsListUpdate { friends, .. } => {
                self.world.social.friends = friends.iter().map(|f| f.name.clone()).collect();
            }
            Event::InventoryPutObjInContainer { obj_guid, container_guid, .. } => {
                if Some(container_guid) == self.self_guid {
                    self.world.unequip_to_pack(obj_guid);
                }
            }
            Event::WieldObject { obj_guid, location } => {
                self.world.equip_from_pack(obj_guid, location);
            }
            Event::CharacterTitle { current, owned } => {
                self.world.social.titles = owned.iter().map(|&id| id as i32).collect();
                self.world.social.title = current as i32;
            }
            Event::InventoryPutObjectIn3D { obj_guid } => {
                self.world.drop_item(obj_guid);
            }
            Event::InventoryServerSaveFailed { item_guid, code } => {
                self.world
                    .chat
                    .push("Error", &format!("Inventory save failed (item {item_guid}, code {code})"), ChatStyle::Error);
            }
            Event::ApproachVendor(v) => {
                self.world.vendor = crate::world::VendorState {
                    guid: v.vendor_guid,
                    name: self.world.entity_name(v.vendor_guid),
                    buy_mult: v.buy_price,
                    sell_mult: v.sell_price,
                    items: v
                        .items
                        .iter()
                        .map(|i| crate::world::VendorItem {
                            guid: i.guid,
                            name: i.name.clone(),
                            item_type: i.item_type,
                            value: i.value,
                            stack_size: i.stack_size,
                            icon_id: i.icon_id,
                            buy_price: i.buy_price,
                            sell_price: i.sell_price,
                        })
                        .collect(),
                    ..Default::default()
                };
            }
            Event::WeenieError { code } => {
                self.world.chat.push("Error", &format!("Error {code}"), ChatStyle::Error);
            }
            Event::WeenieErrorWithString { code, text } => {
                self.world.chat.push("Error", &format!("{text} ({code})"), ChatStyle::Error);
            }
            Event::RegisterTrade { initiator, partner } => {
                self.world.trade = crate::world::TradeState {
                    partner,
                    partner_name: self.world.entity_name(partner),
                    ..Default::default()
                };
                let _ = initiator;
            }
            Event::CloseTrade { reason } => {
                self.world.trade = Default::default();
                if reason != 0 {
                    self.world
                        .chat
                        .push("Trade", &format!("Trade closed (error {reason})"), ChatStyle::System);
                }
            }
            Event::AddToTrade { item, side, .. } => {
                if side == 0 {
                    self.world.trade.our.push(item);
                } else {
                    self.world.trade.their.push(item);
                }
            }
            Event::RemoveFromTrade { item, side } => {
                if side == 0 {
                    self.world.trade.our.retain(|&g| g != item);
                } else {
                    self.world.trade.their.retain(|&g| g != item);
                }
            }
            Event::AcceptTrade { guid } => {
                if Some(guid) == self.self_guid {
                    self.world.trade.we_accepted = true;
                } else {
                    self.world.trade.they_accepted = true;
                }
            }
            Event::DeclineTrade { guid } => {
                if Some(guid) != self.self_guid {
                    self.world.trade = Default::default();
                    self.world.chat.push("Trade", "The other side declined the trade.", ChatStyle::System);
                }
            }
            Event::ResetTrade { .. } => {
                self.world.trade.our.clear();
                self.world.trade.their.clear();
            }
            Event::TradeFailure { item, code } => {
                self.world.trade.failure = Some(code);
                self.world
                    .chat
                    .push("Trade", &format!("Item {item} rejected (error {code})"), ChatStyle::Error);
            }
            Event::ClearTradeAcceptance => {
                self.world.trade.we_accepted = false;
                self.world.trade.they_accepted = false;
            }
            Event::CommunicationTransientString { text } => {
                self.world.chat.push("System", &text, ChatStyle::System);
            }
            Event::MagicUpdateEnchantment { enchantment } => {
                self.world
                    .flash
                    .replace(format!("Enchanted: spell {:#04x}", enchantment.spell_id));
            }
            Event::UpdateTitle { title_id, set_as_display } => {
                if set_as_display {
                    self.world.social.title = title_id as i32;
                }
            }
            Event::SetSquelchDB { entries, .. } => {
                self.world.social.squelch = entries.iter().map(|e| e.info.name.clone()).collect();
            }
            Event::SendClientContractTrackerTable { contracts } => {
                self.world.social.contracts = contracts
                    .iter()
                    .map(|c| crate::world::Contract { id: c.id as u64, title: format!("Contract {}", c.id), stage: c.stage })
                    .collect();
            }
            Event::SendClientContractTracker { contract } => {
                let c = crate::world::Contract {
                    id: contract.id as u64,
                    title: format!("Contract {}", contract.id),
                    stage: contract.stage,
                };
                if let Some(slot) = self.world.social.contracts.iter_mut().find(|x| x.id == c.id) {
                    *slot = c;
                } else {
                    self.world.social.contracts.push(c);
                }
            }
            Event::AllegianceUpdate(a) => {
                let leader = a.members.first().map(|m| m.name.clone()).unwrap_or_default();
                self.world.social.allegiance = Some(crate::world::Allegiance { leader, level: a.rank });
            }
            Event::FellowshipFullUpdate(f) => {
                let leader = f
                    .fellows
                    .iter()
                    .find(|m| m.guid == f.leader_guid)
                    .map(|m| m.name.clone())
                    .unwrap_or_default();
                let members = f.fellows.iter().map(|m| m.name.clone()).collect();
                self.world.social.fellowship = Some(crate::world::Fellowship { leader, members });
            }
            Event::HouseData(h) => {
                self.world.social.house = Some(crate::world::House {
                    guid: h.dwelling_id,
                    name: format!("House {:#x}", h.dwelling_id),
                    district: h.owner_name.clone(),
                });
            }
            Event::HouseStatus { code } => {
                if code != 0 {
                    self.world.chat.push("House", &format!("House error {code}"), ChatStyle::Error);
                }
            }
            Event::CloseGroundContainer { container_guid } => {
                if container_guid == self.world.container.guid {
                    self.world.container = Default::default();
                }
            }
            Event::ViewContents { container_guid, items } => {
                self.world.container = crate::world::ContainerState {
                    guid: container_guid,
                    name: self.world.entity_name(container_guid),
                    items: items.iter().map(|(g, _)| self.world.item_state(*g)).collect(),
                    sel: 0,
                };
            }
            Event::UseDone { code, text } => {
                if let Some(t) = text {
                    self.world.chat.push("System", &t, ChatStyle::System);
                } else if code != 0 {
                    self.world.flash.replace(format!("Use failed ({code})"));
                }
            }
            Event::AttackDone { code } => {
                if code != 0 {
                    self.world.flash.replace(format!("Attack failed ({code})"));
                }
            }
        }
    }

    // ==================================================================== C2S

    fn send_msg(&mut self, opcode: u32, body: &[u8]) {
        if let Some(net) = self.net.as_mut() {
            if let Err(e) = net.send_game_message(opcode, body) {
                self.status = format!("Send failed: {e}");
            }
        }
    }

    fn send_action(&mut self, action_type: u32, body: &[u8]) {
        self.client_seq += 1;
        if let Some(net) = self.net.as_mut() {
            if let Err(e) = net.send_game_action(self.client_seq, action_type, body) {
                self.status = format!("Send failed: {e}");
            }
        }
    }

    // =============================================================== character

    pub fn select_character(&mut self, idx: usize) {
        if idx >= self.char_list.len() {
            return;
        }
        let guid = self.char_list[idx].guid;
        self.self_guid = Some(guid);
        self.world.self_guid = Some(guid);
        self.self_name = self.char_list[idx].name.clone();
        self.phase = Phase::Entering;
        self.status = "Entering world…".into();
        self.enter_started_at = Some(Instant::now());
        self.send_msg(messages::opcode::CHARACTER_ENTER_WORLD_REQUEST, &[]);
    }

    pub fn delete_character(&mut self, idx: usize) {
        if idx >= self.char_list.len() {
            return;
        }
        let body = messages::character_delete_body(&self.account.to_lowercase(), idx as u32);
        self.send_msg(messages::opcode::CHARACTER_DELETE, &body);
    }

    pub fn logout(&mut self) {
        if self.phase != Phase::InWorld && self.phase != Phase::LoggingOut {
            return;
        }
        self.phase = Phase::LoggingOut;
        self.status = "Logging off…".into();
        self.logoff_sent_at = Some(Instant::now());
        self.send_msg(messages::opcode::CHARACTER_LOGOFF, &[]);
    }

    // =============================================================== combat

    pub fn toggle_combat_mode(&mut self) {
        if self.combat_mode == combat_mode::MELEE {
            self.combat_mode = combat_mode::NON_COMBAT;
            self.world.flash = Some("peace mode".into());
        } else {
            self.combat_mode = combat_mode::MELEE;
            self.world.flash = Some("combat mode".into());
        }
        self.send_action(messages::action::CHANGE_COMBAT_MODE, &messages::body_change_combat_mode(self.combat_mode));
    }

    /// Rebuild the target list for the current kind (0 creature, 1 object,
    /// 2 player), nearest first.
    pub fn refresh_targets(&mut self) {
        self.targets = if let Some(mv) = self.movement.as_ref() {
            self.world.targets_of_kind(mv.cell, &mv.pos, self.target_kind)
        } else {
            Vec::new()
        };
        self.world.clamp_target(self.targets.len());
    }

    /// Cycle to the previous/next target of `kind` and appraise it.
    pub fn cycle_target(&mut self, kind: u8, dir: i32) {
        if self.phase != Phase::InWorld || self.world.dead {
            return;
        }
        self.target_kind = kind;
        self.refresh_targets();
        if self.targets.is_empty() {
            self.world.flash = Some("no targets of that kind in range".into());
            return;
        }
        let len = self.targets.len() as i32;
        self.world.target_index = ((self.world.target_index as i32 + dir).rem_euclid(len)) as usize;
        self.appraise_current();
    }

    /// Target the nearest entity of `kind` and appraise it.
    pub fn nearest_target(&mut self, kind: u8) {
        if self.phase != Phase::InWorld || self.world.dead {
            return;
        }
        self.target_kind = kind;
        self.refresh_targets();
        if self.targets.is_empty() {
            self.world.flash = Some("no targets of that kind in range".into());
            return;
        }
        self.world.target_index = 0;
        self.appraise_current();
    }

    /// Send IdentifyObject + QueryHealth for the current target.
    pub fn appraise_current(&mut self) {
        let Some(guid) = self.current_target() else {
            self.world.flash = Some("no target selected".into());
            return;
        };
        self.world.flash = Some(format!("target: {}", entity_name(&self.world, guid)));
        self.send_action(messages::action::IDENTIFY_OBJECT, &messages::body_identify_object(guid));
        self.send_action(messages::action::QUERY_HEALTH, &messages::body_query_health(guid));
    }

    /// The currently targeted guid, if any.
    pub fn current_target(&self) -> Option<u32> {
        self.targets.get(self.world.target_index).copied()
    }

    // ---------------------------------------------------- world actions

    /// E — examine (IdentifyObject) the current target.
    pub fn examine(&mut self) {
        if self.phase != Phase::InWorld || self.world.dead {
            return;
        }
        self.appraise_current();
    }

    /// Mouse right-click on an item (inventory / vendor / trade / loot):
    /// IdentifyObject — the Appraise response lands in `world.examine`.
    pub fn identify_item(&mut self, guid: u32) {
        if self.phase != Phase::InWorld {
            return;
        }
        self.send_action(messages::action::IDENTIFY_OBJECT, &messages::body_identify_object(guid));
    }

    /// Drag an item onto a world entity and release: USE_WITH_TARGET — the
    /// source item acts on the target (a lockpick unlocks, a spell item
    /// casts, etc.). Containers use `put_item_in_container` instead.
    pub fn use_item_on(&mut self, item: u32, target: u32) {
        if self.phase != Phase::InWorld {
            return;
        }
        self.send_action(
            messages::action::USE_WITH_TARGET,
            &messages::body_use_with_target(item, target),
        );
    }

    /// R — use the current target (retail USE). A non-vendor NPC creature
    /// opens the talk dialog; a player opens trade; a door/container
    /// activates; anything else is a ground-item pickup.
    pub fn use_target(&mut self) {
        if self.phase != Phase::InWorld || self.world.dead {
            return;
        }
        let Some(guid) = self.current_target() else {
            self.world.flash = Some("nothing targeted (+ - ' [ ] \\ , / to target)".into());
            return;
        };
        // Copy the fields out so the entity borrow ends before any
        // mutable self use below.
        let (is_creature, is_player, is_vendor, item_type, name) = match self.world.entities.get(&guid) {
            Some(e) => (e.is_player || e.health.is_some(), e.is_player, e.is_vendor, e.item_type, e.display_name()),
            None => {
                self.world.flash = Some("target out of range".into());
                return;
            }
        };
        if is_creature && !is_player && !is_vendor {
            // A monster: enter the talk dialog (lines go out as TalkDirect).
            self.talk_to = Some(guid);
            self.chat_focused = true;
            self.chat_buf.clear();
            self.world.flash = Some(format!("talking to {name}"));
            return;
        }
        if is_player && !is_vendor {
            self.send_action(messages::action::OPEN_TRADE_NEGOTIATIONS, &messages::body_open_trade(guid));
            self.world.flash = Some(format!("trading with {name}"));
            return;
        }
        if !is_creature {
            if item_type == crate::world::ITEM_TYPE_DOOR || item_type == crate::world::ITEM_TYPE_CONTAINER {
                // A door swings, a container streams its contents (ViewContents).
                self.send_action(messages::action::USE, &messages::body_use(guid));
                self.world.flash = Some(format!("using {name}"));
                return;
            }
            // A ground item: pick it up into the main pack.
            if let Some(selfg) = self.self_guid {
                self.send_action(
                    messages::action::PUT_ITEM_IN_CONTAINER,
                    &messages::body_put_item_in_container(guid, selfg, 0),
                );
                self.world.flash = Some(format!("picking up {name}"));
                return;
            }
        }
        // A vendor (or other creature): Use — the vendor answers with
        // ApproachVendor, a door opens.
        self.send_action(messages::action::USE, &messages::body_use(guid));
        self.world.flash = Some(format!("using {name}"));
    }

    /// F — pick up the current target into the main pack (retail
    /// SelectionPickUp). A container opens instead; a door is a no-op.
    pub fn pickup_target(&mut self) {
        if self.phase != Phase::InWorld || self.world.dead {
            return;
        }
        let Some(guid) = self.current_target() else {
            self.world.flash = Some("nothing targeted (+ - ' [ ] \\ , / to target)".into());
            return;
        };
        let (is_creature, item_type, name) = match self.world.entities.get(&guid) {
            Some(e) => (e.is_player || e.health.is_some(), e.item_type, e.display_name()),
            None => {
                self.world.flash = Some("target out of range".into());
                return;
            }
        };
        if is_creature || item_type == crate::world::ITEM_TYPE_DOOR {
            self.world.flash = Some(format!("can't pick up {name}"));
            return;
        }
        if item_type == crate::world::ITEM_TYPE_CONTAINER {
            // A world chest / corpse can't be carried — open it (loot).
            self.send_action(messages::action::USE, &messages::body_use(guid));
            self.world.flash = Some(format!("opening {name}"));
            return;
        }
        let Some(selfg) = self.self_guid else { return };
        self.send_action(
            messages::action::PUT_ITEM_IN_CONTAINER,
            &messages::body_put_item_in_container(guid, selfg, 0),
        );
        self.world.flash = Some(format!("picking up {name}"));
    }

    // ---------------------------------------------------- vendor / trade

    /// Confirm the vendor buy order (the pending `world.vendor.buying`).
    pub fn vendor_buy(&mut self) {
        let Some(selfg) = self.self_guid else { return };
        let items: Vec<(i32, u32)> = self.world.vendor.buying.iter().map(|&(g, a)| (g as i32, a)).collect();
        if items.is_empty() {
            return;
        }
        self.send_action(messages::action::BUY, &messages::body_buy(selfg, &items));
        self.world.vendor.buying.clear();
        self.world.flash = Some("buy order sent".into());
    }

    /// Confirm the vendor sell order (the pending `world.vendor.selling`).
    pub fn vendor_sell(&mut self) {
        let Some(selfg) = self.self_guid else { return };
        let items: Vec<(i32, u32)> = self.world.vendor.selling.iter().map(|&(g, a)| (g as i32, a)).collect();
        if items.is_empty() {
            return;
        }
        self.send_action(messages::action::SELL, &messages::body_sell(selfg, &items));
        self.world.vendor.selling.clear();
        self.world.flash = Some("sell order sent".into());
    }

    /// Add an inventory item to the open trade.
    pub fn trade_add(&mut self, item: u32, slot: u32) {
        self.send_action(messages::action::ADD_TO_TRADE, &messages::body_add_to_trade(item, slot));
        self.world.trade.our.push(item);
    }

    /// Accept the open trade.
    pub fn trade_accept(&mut self) {
        let partner = self.world.trade.partner;
        self.send_action(messages::action::ACCEPT_TRADE, &messages::body_accept_trade(partner));
        self.world.trade.we_accepted = true;
    }

    /// Decline the open trade.
    pub fn trade_decline(&mut self) {
        self.send_action(messages::action::DECLINE_TRADE, &messages::body_decline_trade());
        self.world.trade = Default::default();
    }

    /// Reset the open trade (clear both sides).
    pub fn trade_reset(&mut self) {
        self.send_action(messages::action::RESET_TRADE, &messages::body_reset_trade());
        self.world.trade.our.clear();
        self.world.trade.their.clear();
    }

    /// Close the trade window.
    pub fn trade_close(&mut self) {
        self.send_action(messages::action::CLOSE_TRADE_NEGOTIATIONS, &messages::body_close_trade());
        self.world.trade = Default::default();
    }

    // ================================================== inventory / items
    // Mouse-driven item actions (retail drag-and-drop / double-click).

    /// Equip (wield) an item from the pack at an EquipMask location, or
    /// unequip to the main pack when `loc` is 0 (GET_AND_WIELD_ITEM).
    pub fn equip_item(&mut self, guid: u32, loc: i32) {
        self.send_action(
            messages::action::GET_AND_WIELD_ITEM,
            &messages::body_get_and_wield_item(guid, loc),
        );
        if loc == 0 {
            self.world.unequip_to_pack(guid);
        } else {
            self.world.equip_from_pack(guid, loc);
        }
    }

    /// Move an item into a container: the player's own guid is the main
    /// pack, a side pack's guid moves it there (PUT_ITEM_IN_CONTAINER).
    pub fn put_item_in_container(&mut self, item: u32, container: u32) {
        self.send_action(
            messages::action::PUT_ITEM_IN_CONTAINER,
            &messages::body_put_item_in_container(item, container, 0),
        );
    }

    /// Take a looted item into the main pack (PUT_ITEM_IN_CONTAINER to
    /// self). The container's contents refresh server-side.
    pub fn loot_take(&mut self, item: u32) {
        let Some(selfg) = self.self_guid else { return };
        self.send_action(
            messages::action::PUT_ITEM_IN_CONTAINER,
            &messages::body_put_item_in_container(item, selfg, 0),
        );
    }

    /// Drop an item at the player's feet (DROP_ITEM).
    pub fn drop_item(&mut self, guid: u32) {
        self.send_action(messages::action::DROP_ITEM, &messages::body_drop_item(guid));
        self.world.drop_item(guid);
    }

    /// Esc on an open vendor: a pending buy/sell order asks for a second
    /// press (retail's confirm dialog); leaving abandons the order.
    pub fn request_close_vendor(&mut self) {
        let pending = !self.world.vendor.buying.is_empty() || !self.world.vendor.selling.is_empty();
        if pending && !self.confirm_close_vendor {
            self.confirm_close_vendor = true;
            self.world.flash = Some("pending order — Esc again to abandon".into());
            return;
        }
        self.world.vendor = Default::default();
        self.confirm_close_vendor = false;
        self.world.flash = Some("vendor closed".into());
    }

    /// Esc on an open ground container: NO_LONGER_VIEWING_CONTENTS.
    pub fn request_close_container(&mut self) {
        let guid = self.world.container.guid;
        if guid == 0 {
            return;
        }
        self.world.container = Default::default();
        self.send_action(
            messages::action::NO_LONGER_VIEWING_CONTENTS,
            &messages::body_no_longer_viewing(guid),
        );
        self.world.flash = Some("loot closed".into());
    }

    pub fn attack(&mut self) {
        if self.phase != Phase::InWorld || self.world.dead {
            return;
        }
        if self.combat_mode != combat_mode::MELEE {
            self.world.flash = Some("peace mode — ` toggles combat".into());
            return;
        }
        let Some(target) = self.current_target() else {
            self.world.flash = Some("no target selected".into());
            return;
        };
        self.send_action(
            messages::action::TARGETED_MELEE_ATTACK,
            &messages::body_targeted_melee_attack(target),
        );
    }

    // ================================================================== chat

    pub fn send_chat(&mut self) {
        let text = self.chat_buf.trim().to_string();
        self.chat_buf.clear();
        self.chat_focused = false;
        let talk_to = self.talk_to.take();
        if text.is_empty() {
            return;
        }
        if let Some(guid) = talk_to {
            // The R talk dialog: the line goes out as a TalkDirect to the NPC
            // (it runs its ReceiveTalkDirect emote set).
            self.send_action(messages::action::TALK_DIRECT, &messages::body_talk_direct(&text, guid));
            self.world
                .chat
                .push(&format!("Tell {}", entity_name(&self.world, guid)), &text, ChatStyle::Tell);
            return;
        }
        if let Some(rest) = text.strip_prefix('/') {
            let mut parts = rest.splitn(2, ' ');
            let cmd = parts.next().unwrap_or("");
            let arg = parts.next().unwrap_or("");
            match cmd {
                "t" => {
                    let mut p = arg.splitn(2, ' ');
                    let name = p.next().unwrap_or("");
                    let msg = p.next().unwrap_or("");
                    if name.is_empty() || msg.is_empty() {
                        self.world.chat.push("Error", "Usage: /t <name> <text>", ChatStyle::Error);
                    } else {
                        self.send_action(messages::action::TELL, &messages::body_tell(msg, name));
                        self.world.chat.push("You", &format!("(tell {name}) {msg}"), ChatStyle::Tell);
                    }
                }
                "ch" => {
                    let mut p = arg.splitn(2, ' ');
                    let chan = p.next().unwrap_or("");
                    let msg = p.next().unwrap_or("");
                    if let Some(channel_id) = parse_channel(chan) {
                        if msg.is_empty() {
                            self.world.chat.push("Error", "Usage: /ch <channel> <text>", ChatStyle::Error);
                        } else {
                            self.send_msg(
                                messages::opcode::TURBINE_CHAT,
                                &messages::turbine_chat_body(channel_id, &self.self_name, msg, 0),
                            );
                            self.world
                                .chat
                                .push("You", &format!("(ch {chan}) {msg}"), ChatStyle::Channel);
                        }
                    } else {
                        self.world
                            .chat
                            .push("Error", &format!("Unknown channel '{chan}'"), ChatStyle::Error);
                    }
                }
                _ => {
                    self.world
                        .chat
                        .push("Error", &format!("Unknown command /{cmd}"), ChatStyle::Error);
                }
            }
        } else {
            self.send_action(messages::action::TALK, &messages::body_talk(&text));
            self.world.chat.push("You", &text, ChatStyle::Own);
        }
    }

    // ================================================================== input

    pub fn on_key(&mut self, ev: &KeyEvent) {
        let press = ev.kind == KeyEventKind::Press;
        if matches!(
            ev.code,
            KeyCode::Modifier(ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift)
        ) {
            self.shift_held = press;
            return;
        }
        // Fallback: infer Shift from the modifier state of any key event.
        self.shift_held = ev.modifiers.contains(KeyModifiers::SHIFT);
        // A one-shot flash is replaced by any key press (it is shown once in
        // the status bar; the action dispatch may set a new one).
        if press {
            self.world.flash = None;
        }
        match self.phase {
            Phase::Idle => {
                if press && ev.code == KeyCode::Esc {
                    self.quit = true;
                }
            }
            Phase::Connecting | Phase::Entering => {
                if press && ev.code == KeyCode::Esc {
                    self.quit = true;
                }
            }
            Phase::CharSelect => self.on_key_charselect(ev, press),
            Phase::CharCreate => self.on_key_charcreate(ev, press),
            Phase::InWorld => self.on_key_inworld(ev, press),
            Phase::LoggingOut => {}
            Phase::Disconnected => {
                if press {
                    match ev.code {
                        // User-initiated reconnect (retail `gmDisconnectedUI`
                        // "Reconnect" button, 0x4E99C0): fresh connect + login.
                        KeyCode::Enter => {
                            // A manual reconnect takes over from any pending
                            // auto-reconnect.
                            self.auto_reconnect = false;
                            self.reconnect_at = None;
                            self.do_connect();
                        }
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => self.quit = true,
                        _ => {}
                    }
                }
            }
        }
    }

    fn on_key_charselect(&mut self, ev: &KeyEvent, press: bool) {
        if !press {
            return;
        }
        match ev.code {
            KeyCode::Up => {
                if !self.char_list.is_empty() {
                    self.char_sel = (self.char_sel + self.char_list.len() - 1) % self.char_list.len();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.char_list.is_empty() {
                    self.char_sel = (self.char_sel + 1) % self.char_list.len();
                }
            }
            KeyCode::Enter => self.select_character(self.char_sel),
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.create_form = CharCreateForm::new();
                self.phase = Phase::CharCreate;
                self.status = "Create a character.".into();
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if self.char_sel < self.char_list.len() {
                    let name = self.char_list[self.char_sel].name.clone();
                    self.delete_character(self.char_sel);
                    self.status = format!("Deleted {name} (soft).");
                }
            }
            KeyCode::Esc => self.quit = true,
            _ => {}
        }
    }

    fn on_key_charcreate(&mut self, ev: &KeyEvent, press: bool) {
        if !press {
            return;
        }
        let f = &mut self.create_form;
        let h = f.heritage();
        match ev.code {
            KeyCode::Esc => {
                self.phase = Phase::CharSelect;
            }
            KeyCode::Enter => {
                // The server compares this to session.Account (lowercased)
                // and silently drops the request on mismatch.
                let body = f.body(&self.account.to_lowercase());
                let name_ok = !f.name.trim().is_empty();
                let sum_ok = f.attr_sum() <= f.heritage().attribute_credits;
                if name_ok && sum_ok {
                    self.phase = Phase::CharSelect;
                    self.status = "Creating character…".into();
                    self.send_msg(messages::opcode::CHARACTER_CREATE, &body);
                } else if !name_ok {
                    f.error = Some("Name required.".into());
                } else {
                    f.error = Some(format!(
                        "Attribute sum {} exceeds {} credits.",
                        f.attr_sum(),
                        f.heritage().attribute_credits
                    ));
                }
            }
            KeyCode::Tab => {
                f.template_idx = (f.template_idx + 1) % h.templates.len();
                f.attrs = h.templates[f.template_idx].attributes;
                f.error = None;
            }
            KeyCode::Up => {
                f.heritage_idx =
                    (f.heritage_idx + acproto::chargen::HERITAGES.len() - 1) % acproto::chargen::HERITAGES.len();
                f.template_idx = 0;
                f.start_area_idx = 0;
                f.attrs = f.heritage().templates[0].attributes;
            }
            KeyCode::Down => {
                f.heritage_idx = (f.heritage_idx + 1) % acproto::chargen::HERITAGES.len();
                f.template_idx = 0;
                f.start_area_idx = 0;
                f.attrs = f.heritage().templates[0].attributes;
            }
            KeyCode::Left => {
                let n = f.heritage().start_areas.len();
                if n > 0 {
                    f.start_area_idx = (f.start_area_idx + n - 1) % n;
                }
            }
            KeyCode::Right => {
                let n = f.heritage().start_areas.len();
                if n > 0 {
                    f.start_area_idx = (f.start_area_idx + 1) % n;
                }
            }
            KeyCode::PageUp => {
                f.gender = if f.gender == 1 { 0 } else { 1 };
            }
            KeyCode::F(n) if (1..=6).contains(&n) => {
                let i = (n - 1) as usize;
                f.attrs[i] = f.attrs[i].saturating_add(5).min(100);
                f.error = None;
            }
            KeyCode::Backspace => {
                f.name.pop();
            }
            // Free text (letters, digits, space, punctuation) goes to the name.
            KeyCode::Char(c) if c.is_ascii_graphic() || c == ' ' => {
                f.name.push(c);
                f.error = None;
            }
            _ => {}
        }
    }

    fn on_key_inworld(&mut self, ev: &KeyEvent, press: bool) {
        // The menu takes priority over everything.
        if self.menu_view.is_some() {
            if press {
                self.menu_key(ev);
            }
            return;
        }
        if self.chat_focused {
            if !press {
                return;
            }
            match ev.code {
                KeyCode::Enter => self.send_chat(),
                KeyCode::Esc => {
                    self.chat_focused = false;
                    self.chat_buf.clear();
                }
                KeyCode::Backspace => {
                    self.chat_buf.pop();
                }
                KeyCode::Char(c) if c.is_ascii_graphic() || c == ' ' => self.chat_buf.push(c),
                _ => {}
            }
            return;
        }
        // Dead: chat + quit only; make sure no motion keys stick.
        if self.world.dead {
            if press {
                match ev.code {
                    KeyCode::Enter | KeyCode::Char('/') => {
                        self.chat_focused = true;
                        self.chat_buf.clear();
                    }
                    KeyCode::Esc => self.menu_view = Some(MenuView::Menu),
                    KeyCode::Char('q') | KeyCode::Char('Q') => self.quit = true,
                    _ => {}
                }
            } else {
                self.keys_down.remove(&ev.code);
            }
            return;
        }
        // Movement keys: track held state.
        if is_move_key(ev.code) {
            if press {
                self.keys_down.insert(ev.code);
            } else {
                self.keys_down.remove(&ev.code);
            }
            return;
        }
        if !press {
            return;
        }
        // F1 opens the key-map help (retail).
        if ev.code == KeyCode::F(1) {
            self.menu_view = Some(MenuView::Help);
            self.help_scroll = 0;
            self.keys_down.clear();
            return;
        }
        // Esc closes an open vendor / trade / loot box. A vendor with a
        // pending order asks for a second press (retail's confirm dialog);
        // trade and loot close on the first.
        if ev.code == KeyCode::Esc {
            if self.world.vendor.guid != 0 {
                self.request_close_vendor();
                return;
            }
            if self.world.trade.partner != 0 {
                self.trade_close();
                self.world.flash = Some("trade closed".into());
                return;
            }
            if self.world.container.guid != 0 {
                self.request_close_container();
                return;
            }
        }
        // Any other key disarms a pending vendor close-confirm.
        if self.confirm_close_vendor {
            self.confirm_close_vendor = false;
        }
        match ev.code {
            // Locomotion.
            KeyCode::Char(' ') => self.pending_jump = true,
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                self.run_lock = !self.run_lock;
                self.world.flash = Some(if self.run_lock { "run: on" } else { "run: off (walk)" }.into());
            }
            // ` toggles combat / peace mode (sent to the server).
            KeyCode::Char('`') => self.toggle_combat_mode(),
            // Targeting (the retail key map): +/- the compass items
            // (creatures), ' the closest creature, L/; the previous/next
            // creature, [/] the previous/next object, \ the closest object,
            // ,/. the previous/next person, / the closest person.
            KeyCode::Char('+') => self.cycle_target(0, 1),
            KeyCode::Char('-') => self.cycle_target(0, -1),
            KeyCode::Char('l') | KeyCode::Char('L') => self.cycle_target(0, -1),
            KeyCode::Char(';') => self.cycle_target(0, 1),
            KeyCode::Char('[') => self.cycle_target(1, -1),
            KeyCode::Char(']') => self.cycle_target(1, 1),
            KeyCode::Char(',') => self.cycle_target(2, -1),
            KeyCode::Char('.') => self.cycle_target(2, 1),
            KeyCode::Char('\'') => self.nearest_target(0),
            KeyCode::Char('\\') => self.nearest_target(1),
            KeyCode::Char('/') => self.nearest_target(2),
            // Retail F/R/E: pick up the target, use it (an NPC opens the
            // talk dialog), examine it.
            KeyCode::Char('f') | KeyCode::Char('F') => self.pickup_target(),
            KeyCode::Char('r') | KeyCode::Char('R') => self.use_target(),
            KeyCode::Char('e') | KeyCode::Char('E') => self.examine(),
            // T attacks the current target (melee; combat stance first).
            KeyCode::Char('t') | KeyCode::Char('T') => self.attack(),
            // Hotkey slots 1-9 (0 = the 10th) and the spell-bar chrome:
            // Del/PgDn the slot, Ins/PgUp the tab, End "casts".
            KeyCode::Char(c @ ('1'..='9')) => {
                self.world.skills.slot = c as usize - '1' as usize;
                self.world.flash = Some(format!("spell slot {c} (empty)"));
            }
            KeyCode::Char('0') => {
                self.world.skills.slot = 9;
                self.world.flash = Some("spell slot 0 (empty)".into());
            }
            KeyCode::Delete => {
                self.world.skills.slot = (self.world.skills.slot + 9) % 10;
                self.world.flash =
                    Some(format!("spell slot {} (empty)", slot_label(self.world.skills.slot)));
            }
            KeyCode::PageDown => {
                self.world.skills.slot = (self.world.skills.slot + 1) % 10;
                self.world.flash =
                    Some(format!("spell slot {} (empty)", slot_label(self.world.skills.slot)));
            }
            KeyCode::Insert => self.world.skills.tab = (self.world.skills.tab + 7) % 8,
            KeyCode::PageUp => self.world.skills.tab = (self.world.skills.tab + 1) % 8,
            KeyCode::End => {
                self.world.flash =
                    Some(format!("cast slot {} (empty)", slot_label(self.world.skills.slot)));
            }
            // Enter / Tab open chat (the retail ToggleChatEntry); Esc the
            // menu; Shift+Esc logs off the world.
            KeyCode::Enter | KeyCode::Tab => {
                self.chat_focused = true;
                self.chat_buf.clear();
            }
            KeyCode::Esc if ev.modifiers.contains(KeyModifiers::SHIFT) => self.logout(),
            KeyCode::Esc => {
                self.menu_view = Some(MenuView::Menu);
                self.menu_sel = 0;
                self.keys_down.clear();
            }
            _ => {}
        }
    }

    /// Esc menu + subviews: j/k (or the arrows) move, Enter activates, Esc
    /// backs out (a subview returns to the menu, the menu closes). The
    /// items mirror the retail menu minus server/account/plugin management
    /// (the profile store and the CLI own that).
    fn menu_key(&mut self, ev: &KeyEvent) {
        use MenuView::{Help, Menu, Themes, Widgets, WorldView};
        match self.menu_view {
            Some(Menu) => {
                const N: usize = 7; // themes, widgets, worldview, glow, help, logout, quit
                match ev.code {
                    KeyCode::Up | KeyCode::Char('k') => self.menu_sel = (self.menu_sel + N - 1) % N,
                    KeyCode::Down | KeyCode::Char('j') => self.menu_sel = (self.menu_sel + 1) % N,
                    KeyCode::Enter => match self.menu_sel {
                        0 => {
                            if let Some(i) = Theme::ALL.iter().position(|&t| t == self.theme) {
                                self.theme_sel = i;
                            }
                            self.menu_view = Some(Themes);
                        }
                        1 => {
                            self.widget_sel = self.multi_sel.min(13);
                            self.menu_view = Some(Widgets);
                        }
                        2 => {
                            self.worldview_sel = usize::from(self.world_view);
                            self.menu_view = Some(WorldView);
                        }
                        3 => {
                            self.glow_actors = !self.glow_actors;
                            self.world.flash =
                                Some(format!("actor glow: {}", if self.glow_actors { "on" } else { "off" }));
                        }
                        4 => {
                            self.menu_view = Some(Help);
                            self.help_scroll = 0;
                        }
                        5 => self.logout(),
                        _ => self.quit = true,
                    },
                    KeyCode::Esc => self.menu_view = None,
                    _ => {}
                }
            }
            Some(Themes) => {
                const N: usize = Theme::ALL.len();
                match ev.code {
                    KeyCode::Up | KeyCode::Char('k') => self.theme_sel = (self.theme_sel + N - 1) % N,
                    KeyCode::Down | KeyCode::Char('j') => self.theme_sel = (self.theme_sel + 1) % N,
                    KeyCode::Enter => {
                        self.theme = Theme::ALL[self.theme_sel];
                        self.world.flash = Some(format!("theme: {}", self.theme.name()));
                    }
                    KeyCode::Esc => self.menu_view = Some(Menu),
                    _ => {}
                }
            }
            Some(Widgets) => {
                const N: usize = 14;
                match ev.code {
                    KeyCode::Up | KeyCode::Char('k') => self.widget_sel = (self.widget_sel + N - 1) % N,
                    KeyCode::Down | KeyCode::Char('j') => self.widget_sel = (self.widget_sel + 1) % N,
                    KeyCode::Enter => {
                        self.multi_sel = self.widget_sel;
                        self.list_scroll = 0;
                    }
                    KeyCode::Esc => self.menu_view = Some(Menu),
                    _ => {}
                }
            }
            Some(WorldView) => match ev.code {
                KeyCode::Up | KeyCode::Char('k') => self.worldview_sel = 0,
                KeyCode::Down | KeyCode::Char('j') => self.worldview_sel = 1,
                KeyCode::Enter => {
                    self.world_view = self.worldview_sel == 1;
                    self.world.flash = Some(if self.world_view {
                        "world view: 3d"
                    } else {
                        "world view: tactical"
                    }.into());
                }
                KeyCode::Esc => self.menu_view = Some(Menu),
                _ => {}
            },
            Some(Help) => match ev.code {
                KeyCode::Up | KeyCode::Char('k') => self.help_scroll = self.help_scroll.saturating_sub(3),
                KeyCode::Down | KeyCode::Char('j') => self.help_scroll += 3,
                KeyCode::Esc => self.menu_view = Some(Menu),
                _ => {}
            },
            None => {}
        }
    }

    /// Build the current held-key set from `keys_down`.
    #[must_use]
    fn held_keys(&self) -> HeldKeys {
        let down = |c: KeyCode| self.keys_down.contains(&c);
        HeldKeys {
            forward: down(KeyCode::Char('w')) || down(KeyCode::Up),
            backward: down(KeyCode::Char('x')) || down(KeyCode::Down),
            turn_left: down(KeyCode::Char('a')) || down(KeyCode::Left),
            turn_right: down(KeyCode::Char('d')) || down(KeyCode::Right),
            strafe_left: down(KeyCode::Char('z')),
            strafe_right: down(KeyCode::Char('c')),
            stop: down(KeyCode::Char('s')),
            jump: false,
        }
    }

    /// Stop all motion input (retail gates input via `PlayerIsDead`; a key
    /// still held in `keys_down` would otherwise keep sending motion after we
    /// die, and a stale jump would fire right after we revive).
    fn kill_input(&mut self) {
        self.keys_down.clear();
        self.pending_jump = false;
    }

    // =================================================================== tick

    /// Per-frame step: retail session timers + fixed-cadence movement.
    ///
    /// The timer set mirrors the retail client (decompile line / image
    /// address): login retransmit 2.0 s × 20 (`LogonUseTime` 372281 /
    /// 0x544A10), referral-cookie retransmit 0.333 s (`ProcessConnection`
    /// 372862 / 0x545450), PAK keepalive every 2.0 s (`EnqueuePak` 371419 /
    /// 0x543B10), 40 s no-data link check every ~4 s and the 110 s
    /// enter-world deadline (`CPlayerSystem::UseTime` 400550 / 0x563110),
    /// logoff fallback ≈ 3 s + 20 s (`RequestLogOff` 400379 / 0x562DD0).
    /// Nothing here reconnects on its own.
    pub fn step(&mut self, now: Instant) {
        if self.quit {
            return;
        }
        // Login-request retransmit: every 2.0 s, up to 20 tries, then give
        // up (retail ends the login state with
        // ID_ConnectionError_ClientTimedOutServer; the user retries).
        if self.phase == Phase::Connecting && self.connect_cookie.is_none() {
            if let Some(sent_at) = self.login_sent_at {
                if self.login_retries >= 20 {
                    self.server_died("Server did not respond to the login request");
                    return;
                }
                if now.duration_since(sent_at) >= Duration::from_secs_f64(2.0) {
                    if let Some(net) = self.net.as_mut() {
                        let _ = net.send_login(&self.account, &self.password);
                    }
                    self.login_retries += 1;
                    self.login_sent_at = Some(now);
                }
            }
        }
        // Referral-cookie retransmit (retail net state 4): every 0.333 s
        // while the handshake is outstanding. No deadline of its own — the
        // 40 s no-data check below is the bound.
        if self.phase == Phase::Connecting && self.connect_cookie.is_some() {
            if let Some(sent_at) = self.connect_sent_at {
                if now.duration_since(sent_at) >= Duration::from_millis(333) {
                    let cookie = self.connect_cookie;
                    if let (Some(cookie), Some(net)) = (cookie, self.net.as_mut()) {
                        let _ = net.retransmit_connect_response(cookie);
                    }
                    self.connect_sent_at = Some(now);
                }
            }
        }
        // Keepalive (retail `EnqueuePak`): an AckSequence optional header
        // (0x4000) carrying the highest S2C packet sequence, at most once
        // per 2.0 s, on every established session. Any valid C2S packet
        // resets the server's 15 s auth / 60 s session timeout
        // (PROTOCOL.md §13) — this is what keeps an idle character-select
        // session alive.
        if self.net_id.is_some()
            && self.last_pak.is_none_or(|t| now.duration_since(t) >= Duration::from_secs_f64(2.0))
        {
            if let Some(net) = self.net.as_mut() {
                match net.send_pak(net.max_s2c_seq()) {
                    Ok(seq) => {
                        self.last_pak = Some(now);
                        self.last_pak_probe = Some((seq, now));
                    }
                    Err(e) => {
                        // A send failure means the transport is dead: treat
                        // it as a server-down (arm auto-reconnect).
                        self.auto_reconnect = true;
                        self.reconnect_delay = RECONNECT_MIN_MS;
                        self.server_died(&format!("Send failed: {e}"));
                        return;
                    }
                }
            }
        }
        // Server-down watchdog (retail `CPlayerSystem::UseTime` / Stockpile
        // parity): no data for 60 s in-world (3 missed 20 s keepalive echoes)
        // or 150 s on the auth/character-select session means the server is
        // gone. Checked every ~4 s.
        if self.net_id.is_some()
            && self.last_link_check.is_none_or(|t| now.duration_since(t) >= Duration::from_secs_f64(4.0))
        {
            self.last_link_check = Some(now);
            let window = match self.phase {
                Phase::CharSelect => Some(150.0),
                Phase::Entering | Phase::InWorld => Some(60.0),
                _ => None,
            };
            if let Some(window) = window {
                if now.duration_since(self.last_rx) >= Duration::from_secs_f64(window) {
                    self.auto_reconnect = true;
                    self.reconnect_delay = RECONNECT_MIN_MS;
                    self.server_died(&format!(
                        "Connection lost — no data from server for {window:.0} s"
                    ));
                    return;
                }
            }
        }
        // Fire the next auto-reconnect attempt when due. The backoff doubles
        // per attempt (capped); `server_died` schedules the next wait.
        if self.phase == Phase::Disconnected {
            if let Some(at) = self.reconnect_at {
                if now >= at {
                    self.reconnect_at = None;
                    self.reconnect_delay =
                        (self.reconnect_delay * 2).min(RECONNECT_MAX_MS);
                    self.status = "Reconnecting…".into();
                    self.do_connect();
                }
            }
        }
        // Enter-world deadline (retail `CPlayerSystem::UseTime`): an
        // outstanding enter-world request older than 110 s is treated as a
        // dead server.
        if self.phase == Phase::Entering {
            if let Some(started) = self.enter_started_at {
                if now.duration_since(started) >= Duration::from_secs_f64(110.0) {
                    self.server_died("Server did not place you in the world");
                    return;
                }
            }
        }
        // Logoff deadline (retail `RequestLogOff`: logOffRequestTime = now +
        // 3 s, + 20 s more if the player was still dirty): if the server
        // never confirms, drop the world locally and return to character
        // select.
        if self.phase == Phase::LoggingOut {
            if let Some(sent) = self.logoff_sent_at {
                if now.duration_since(sent) >= Duration::from_secs_f64(25.0) {
                    self.phase = Phase::CharSelect;
                    self.world = World::new();
                    self.movement = None;
                    self.logoff_sent_at = None;
                    self.status = "Logged out.".into();
                }
            }
        }
        // Movement at a fixed 50 ms cadence.
        if self.phase == Phase::InWorld
            && !self.world.dead
            && now.duration_since(self.last_tick) >= Duration::from_secs_f32(TICK)
        {
            // Gate the cadence: without this the sim would advance one tick
            // per event-loop iteration (faster than 20 Hz under key repeat),
            // running the movement — and the terrain/solid probe — fast.
            self.last_tick = now;
            let mut held = self.held_keys();
            held.jump = self.pending_jump;
            self.pending_jump = false;
            let walking = self.shift_held || !self.run_lock;
            if let Some(mv) = self.movement.as_mut() {
                // Terrain / solid / door collision probe (retail DATs);
                // `None` when the dats are unavailable (flat-plane behavior).
                let probe = self.world3d.as_mut().map(|w| w as &mut dyn WorldProbe);
                let actions = mv.tick(TICK, &mut held, walking, now, probe);
                let lb = (mv.cell >> 16) & 0xFFFF;
                self.send_move_actions(actions);
                // Re-base the 3D static geometry on a landblock change
                // (no-op while unchanged).
                if let Some(w) = self.world3d.as_mut() {
                    w.set_landblock(lb);
                }
                self.refresh_targets();
            }
        }
        // Far-cull distant entities (throttled to at most once per second;
        // the guards keep self, the current target, the trade partner, an
        // open container, and an active vendor from being dropped).
        if self.phase == Phase::InWorld
            && self.last_cull.is_none_or(|t| now.duration_since(t) >= Duration::from_secs_f64(1.0))
        {
            if let Some(mv) = self.movement.as_ref() {
                let mut keep = Vec::new();
                if let Some(g) = self.self_guid {
                    keep.push(g);
                }
                if let Some(t) = self.targets.get(self.world.target_index).copied() {
                    keep.push(t);
                }
                if self.world.trade.partner != 0 {
                    keep.push(self.world.trade.partner);
                }
                if self.world.container.guid != 0 {
                    keep.push(self.world.container.guid);
                }
                if self.world.vendor.guid != 0 {
                    keep.push(self.world.vendor.guid);
                }
                let _ = self.world.far_cull(mv.cell, &mv.pos, &keep, now);
                self.last_cull = Some(now);
            }
        }
    }

    fn send_move_actions(&mut self, actions: Vec<crate::movement::MoveAction>) {
        for a in actions {
            let crate::movement::MoveAction::Send(action_type, body) = a;
            self.send_action(action_type, &body);
        }
    }

    /// Self position for the radar (from the local sim).
    #[must_use]
    pub fn self_position(&self) -> (u32, Vec3) {
        match self.movement.as_ref() {
            Some(mv) => (mv.cell, mv.pos),
            None => (0, Vec3::default()),
        }
    }
}

// ==================================================================== helpers

fn create_error_text(status: u32) -> &'static str {
    match status {
        2 => "Pending",
        3 => "Name in use",
        4 => "Name banned",
        5 => "Corrupt",
        6 => "Database down",
        _ => "Unknown error",
    }
}

/// Human-readable name for a `CharacterError` (0xF659) code (PROTOCOL.md §10).
fn character_error_text(code: u32) -> &'static str {
    match code {
        1 => "Account in use",
        3 => "Account login failed",
        4 => "Server crash (1)",
        5 => "Logoff",
        6 => "Character delete",
        8 => "Server crash (2)",
        9 => "Account invalid",
        0xA => "Account does not exist",
        0xB => "Could not enter game",
        0xC => "Stress account cannot enter game",
        0xD => "Character already in world",
        0xE => "Player account missing",
        0xF => "Character not owned by this account",
        0x10 => "Character already in world (server)",
        0x11 => "Old character",
        0x12 => "Corrupt character",
        0x13 => "Start server down",
        0x14 => "Could not place character",
        0x15 => "Server full",
        0x17 => "Character locked",
        0x18 => "Subscription expired",
        _ => "Unknown error",
    }
}

fn entity_name(world: &World, guid: u32) -> String {
    world
        .entities
        .get(&guid)
        .and_then(|e| e.name.clone())
        .unwrap_or_else(|| format!("#{guid}"))
}

fn channel_name(channel: u32) -> String {
    use acproto::messages::turbine_channel as tc;
    match channel {
        tc::ALLEGIANCE => "Allegiance".to_string(),
        tc::GENERAL => "General".to_string(),
        tc::TRADE => "Trade".to_string(),
        tc::LFG => "LFG".to_string(),
        tc::ROLEPLAY => "Roleplay".to_string(),
        tc::OLTHOI => "Olthoi".to_string(),
        _ => format!("ch{channel}"),
    }
}

fn parse_channel(name: &str) -> Option<u32> {
    use acproto::messages::turbine_channel as tc;
    match name.to_ascii_lowercase().as_str() {
        "a" | "allegiance" => Some(tc::ALLEGIANCE),
        "g" | "general" => Some(tc::GENERAL),
        "t" | "trade" => Some(tc::TRADE),
        "lfg" => Some(tc::LFG),
        "rp" | "roleplay" => Some(tc::ROLEPLAY),
        "olthoi" => Some(tc::OLTHOI),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character_error_packet(code: u32) -> Vec<u8> {
        messages::game_message(messages::opcode::CHARACTER_ERROR, &code.to_le_bytes())
    }

    /// `CharacterError 1` (account in use): this fork's server killed both
    /// the stale and the new session and then goes silent, so the session is
    /// dead. The client must hand the user the disconnected screen (manual
    /// retry) and must NOT arm any timer to reconnect on its own.
    #[test]
    fn character_error_logon_disconnects_without_autoreconnect() {
        let mut app = App::new("127.0.0.1", 9000, "acct", "pass");
        app.phase = Phase::CharSelect;
        app.handle_game_message(&character_error_packet(1));

        assert!(app.net.is_none());
        assert!(matches!(app.phase, Phase::Disconnected));
        assert!(app.status.contains("Account in use"));
        // Nothing may auto-reconnect.
        assert!(app.login_sent_at.is_none());
        assert!(app.connect_sent_at.is_none());
        assert!(app.net_id.is_none());
        assert!(app.reconnect_at.is_none());
        assert!(!app.auto_reconnect);
    }

    /// Any other `CharacterError` code arrives with the session still
    /// logged in (e.g. `EnterGameCharacterInWorld` while entering): stay
    /// connected, return to character select, and show a readable reason.
    #[test]
    fn character_error_other_keeps_session_and_labels() {
        let mut app = App::new("127.0.0.1", 9000, "acct", "pass");
        app.phase = Phase::Entering;
        app.handle_game_message(&character_error_packet(0xD));

        assert!(matches!(app.phase, Phase::CharSelect));
        assert!(app.status.contains("Character already in world"));
        // Entering timers cleared; session still up (net_id unchanged).
        assert!(app.enter_started_at.is_none());
    }

    /// The 110 s enter-world deadline (retail `CPlayerSystem::UseTime`) ends
    /// a stuck entry: disconnected screen, no auto-reconnect.
    #[test]
    fn enter_world_deadline_disconnects() {
        let mut app = App::new("127.0.0.1", 9000, "acct", "pass");
        app.phase = Phase::Entering;
        app.enter_started_at = Some(Instant::now() - Duration::from_secs(111));
        app.last_rx = Instant::now(); // server is still answering, entry stuck
        app.step(Instant::now());

        assert!(matches!(app.phase, Phase::Disconnected));
        assert!(app.status.contains("did not place you"));
    }

    /// The in-world silence watchdog (60 s, checked every ~4 s) declares the
    /// server down and arms the auto-reconnect backoff (Stockpile parity).
    /// The first attempt is scheduled 10 s out, so it has not fired by the
    /// end of the step that armed it.
    #[test]
    fn silence_timeout_arms_autoreconnect() {
        let mut app = App::new("127.0.0.1", 9000, "acct", "pass");
        app.phase = Phase::InWorld;
        app.net_id = Some(7); // session established
        app.last_rx = Instant::now() - Duration::from_secs(61);
        app.last_link_check = Some(Instant::now() - Duration::from_secs(5));
        app.step(Instant::now());

        assert!(matches!(app.phase, Phase::Disconnected));
        assert!(app.status.contains("no data from server"));
        assert!(app.net_id.is_none());
        // Auto-reconnect is armed with the initial 10 s backoff, scheduled out.
        assert!(app.auto_reconnect);
        assert_eq!(app.reconnect_delay, RECONNECT_MIN_MS);
        assert!(app.reconnect_at.is_some());
    }

    /// An in-world silence under the 60 s window does NOT declare the server
    /// down (the session and the transport stay up).
    #[test]
    fn inworld_silence_under_window_keeps_session() {
        let mut app = App::new("127.0.0.1", 9000, "acct", "pass");
        app.phase = Phase::InWorld;
        app.net_id = Some(7);
        app.last_rx = Instant::now() - Duration::from_secs(59);
        app.last_link_check = Some(Instant::now() - Duration::from_secs(5));
        app.step(Instant::now());

        assert!(app.phase == Phase::InWorld);
        assert!(app.net_id.is_some());
        assert!(!app.auto_reconnect);
    }

    /// The char-select silence window is 150 s (the auth session is pinged far
    /// less often than the in-world one), so 149 s of silence keeps the session.
    #[test]
    fn charselect_silence_under_window_keeps_session() {
        let mut app = App::new("127.0.0.1", 9000, "acct", "pass");
        app.phase = Phase::CharSelect;
        app.net_id = Some(7);
        app.last_rx = Instant::now() - Duration::from_secs(149);
        app.last_link_check = Some(Instant::now() - Duration::from_secs(5));
        app.step(Instant::now());

        assert!(app.phase == Phase::CharSelect);
        assert!(app.net_id.is_some());
        assert!(!app.auto_reconnect);
    }

    /// `AccountBoot` drops the session to the disconnected screen (the user
    /// can retry with Enter) rather than to a silent idle.
    #[test]
    fn account_boot_disconnects() {
        let mut app = App::new("127.0.0.1", 9000, "acct", "pass");
        app.phase = Phase::CharSelect;
        let body = messages::game_message(messages::opcode::ACCOUNT_BOOT, &[]);
        app.handle_game_message(&body);

        assert!(matches!(app.phase, Phase::Disconnected));
        assert!(app.status.contains("Booted from server"));
    }

    /// Logging out of the world returns to character select on the live
    /// session and must clear in-world transient state: a stale open menu
    /// made the first Esc of the next world close the phantom menu, so `q`
    /// toggled run instead of quitting and the session died on timeout.
    #[test]
    fn character_logoff_clears_inworld_transient_state() {
        let mut app = App::new("127.0.0.1", 9000, "acct", "pass");
        app.phase = Phase::InWorld;
        app.menu_view = Some(MenuView::Menu);
        app.chat_focused = true;
        app.keys_down.insert(KeyCode::Char('w'));
        app.pending_jump = true;

        app.handle_game_message(&messages::game_message(messages::opcode::CHARACTER_LOGOFF, &[]));

        assert!(matches!(app.phase, Phase::CharSelect));
        assert!(app.menu_view.is_none(), "stale menu would swallow the next world's first Esc");
        assert!(!app.chat_focused);
        assert!(app.keys_down.is_empty());
        assert!(!app.pending_jump);
    }

    #[test]
    fn character_error_text_covers_documented_codes() {
        assert_eq!(character_error_text(1), "Account in use");
        assert_eq!(character_error_text(0xD), "Character already in world");
        assert_eq!(character_error_text(0x18), "Subscription expired");
        assert_eq!(character_error_text(0xFF), "Unknown error");
    }
}
