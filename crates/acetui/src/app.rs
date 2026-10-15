//! The application state machine: auth, character selection/creation, and the
//! in-world session (movement, combat, chat, keepalive, reconnect).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, ModifierKeyCode};

use acproto::codec::Vec3;
use acproto::messages::{self, combat_mode, Event, GameEvent, Incoming};
use crate::movement::{HeldKeys, Movement, TICK};
use crate::net::{Net, NetEvent};
use crate::world::{ChatStyle, World};

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
    // Timers / liveness
    pub last_echo: Instant,
    pub last_rx: Instant,
    pub last_tick: Instant,
    pub pending_jump: bool,
    pub pending_teleport: bool,
    pub client_seq: u32,
    pub ping_ms: Option<f32>,
    pub crypto_bad: u32,
    pub reconnect_at: Option<Instant>,
    pub backoff: u32,
    /// `ConnectResponse` cookie still awaiting a matching `CharacterList`.
    /// The server sets the session's auth state asynchronously after the
    /// `ConnectRequest` is flushed, so the first (immediate) response can be
    /// dropped; we retransmit until the handshake completes. `None` once done.
    pub connect_cookie: Option<u64>,
    /// When the `ConnectResponse` was last (re)sent.
    pub connect_sent_at: Option<Instant>,
    /// Hard deadline for the auth handshake.
    pub connect_deadline: Option<Instant>,
    /// Anchor for the client echo clock: elapsed seconds since here are
    /// sent as the echo client time. Unix epoch seconds as f32 quantize to
    /// 128 s steps, which the server's VerifyEcho anti-speedhack check
    /// reads as a large, growing clock drift.
    pub echo_clock: Instant,
    pub quit: bool,
    pub menu: bool,
    /// If set, auto-select this character name on the character list.
    pub auto_character: Option<String>,
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
            last_echo: Instant::now(),
            last_rx: Instant::now(),
            last_tick: Instant::now(),
            pending_jump: false,
            pending_teleport: false,
            client_seq: 0,
            ping_ms: None,
            crypto_bad: 0,
            reconnect_at: None,
            backoff: 0,
            connect_cookie: None,
            connect_sent_at: None,
            connect_deadline: None,
            echo_clock: Instant::now(),
            quit: false,
            menu: false,
            auto_character: None,
        }
    }

    /// Open a connection and send the login.
    pub fn start(&mut self) -> anyhow::Result<()> {
        self.do_connect()
    }

    fn do_connect(&mut self) -> anyhow::Result<()> {
        let net = Net::connect(&self.host, self.game_port)?;
        self.net = Some(net);
        self.phase = Phase::Connecting;
        self.status = "Logging in…".into();
        self.last_rx = Instant::now();
        self.last_echo = Instant::now();
        self.reconnect_at = None;
        self.connect_cookie = None;
        self.connect_sent_at = None;
        self.connect_deadline = None;
        self.echo_clock = Instant::now();
        if let Some(net) = self.net.as_mut() {
            net.send_login(&self.account, &self.password)?;
        }
        Ok(())
    }

    /// Drop the connection and schedule a reconnect.
    fn drop_connection(&mut self, reason: &str) {
        if let Some(net) = self.net.take() {
            net.close();
        }
        self.movement = None;
        self.world = World::new();
        self.targets.clear();
        self.combat_mode = combat_mode::NON_COMBAT;
        self.keys_down.clear();
        self.chat_buf.clear();
        self.chat_focused = false;
        self.pending_teleport = false;
        self.connect_cookie = None;
        self.connect_sent_at = None;
        self.connect_deadline = None;
        let delay = Duration::from_secs((1u64).saturating_mul(1u64 << self.backoff.min(4)));
        self.backoff = self.backoff.saturating_add(1);
        self.reconnect_at = Some(Instant::now() + delay);
        self.phase = Phase::Connecting;
        self.status = format!("Disconnected ({reason}); reconnecting in {}s…", delay.as_secs());
    }

    // ============================================================= net events

    pub fn handle_net_event(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::ConnectRequest { cookie, net_id, .. } => {
                if self.phase != Phase::Connecting {
                    return;
                }
                self.backoff = 0;
                self.connect_cookie = Some(cookie);
                self.connect_sent_at = Some(Instant::now());
                self.connect_deadline = Some(Instant::now() + Duration::from_secs(15));
                if let Some(net) = self.net.as_mut() {
                    if let Err(e) = net.send_connect_response(cookie, net_id) {
                        self.status = format!("ConnectResponse failed: {e}");
                        self.drop_connection("connect response failed");
                        return;
                    }
                }
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
            NetEvent::Control { echo_response, .. } => {
                self.last_rx = Instant::now();
                if let Some((client_t, _delta)) = echo_response {
                    let elapsed =
                        Instant::now().duration_since(self.echo_clock).as_secs_f32();
                    self.ping_ms = Some(((elapsed - client_t) * 1000.0).max(0.0));
                }
            }
            NetEvent::CryptoMismatch { .. } => {
                self.last_rx = Instant::now();
                self.crypto_bad += 1;
            }
            NetEvent::Debug(msg) => {
                self.last_rx = Instant::now();
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
                self.world.object_create(
                    o.guid,
                    o.name,
                    position.map(|p| p.cell),
                    position.map(|p| p.pos),
                );
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
                    self.world.update_position(up.guid, up.cell, up.pos);
                }
            }
            Incoming::PlayerTeleport(guid) => {
                if Some(guid) == self.self_guid {
                    self.pending_teleport = true;
                }
            }
            Incoming::VectorUpdate(_) => {}
            Incoming::Motion(_) => {}
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
                self.world.chat.push("Error", &format!("CharacterError {e}"), ChatStyle::Error);
                self.status = format!("CharacterError {e}");
                if self.phase == Phase::Entering {
                    self.phase = Phase::CharSelect;
                }
            }
            Incoming::AccountBoot { reason } => {
                self.status = format!("Booted from server: {}", reason.unwrap_or_default());
                self.phase = Phase::Idle;
                if let Some(net) = self.net.take() {
                    net.close();
                }
            }
            Incoming::AccountBanned { seconds, reason } => {
                self.status = format!(
                    "Banned for {seconds}s: {}",
                    reason.unwrap_or_default()
                );
                self.phase = Phase::Idle;
                if let Some(net) = self.net.take() {
                    net.close();
                }
            }
            Incoming::CharacterLogOff => {
                self.phase = Phase::Idle;
                self.status = "Logged off.".into();
                if let Some(net) = self.net.take() {
                    net.close();
                }
            }
            Incoming::CharacterDelete => {}
            Incoming::Other(op) => {
                self.status = format!("S2C opcode {op:#06x}");
            }
        }
    }

    fn handle_game_event(&mut self, ge: GameEvent) {
        match ge.event {
            Event::PlayerDescription(pd) => {
                self.world.set_vitals_from_description(&pd);
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
        self.send_msg(messages::opcode::CHARACTER_LOGOFF, &[]);
    }

    // =============================================================== combat

    fn toggle_combat_mode(&mut self) {
        if self.combat_mode == combat_mode::MELEE {
            self.combat_mode = combat_mode::NON_COMBAT;
            self.status = "Non-combat mode.".into();
        } else {
            self.combat_mode = combat_mode::MELEE;
            self.status = "Melee mode.".into();
        }
        self.send_action(messages::action::CHANGE_COMBAT_MODE, &messages::body_change_combat_mode(self.combat_mode));
    }

    fn refresh_targets(&mut self) {
        self.targets = if let Some(mv) = self.movement.as_ref() {
            self.world.targets(mv.cell, &mv.pos)
        } else {
            Vec::new()
        };
        self.world.clamp_target(self.targets.len());
    }

    fn cycle_target(&mut self) {
        if self.phase != Phase::InWorld {
            return;
        }
        if self.world.dead {
            return;
        }
        if self.targets.is_empty() {
            self.status = "No targets in range.".into();
            return;
        }
        self.world.target_index = (self.world.target_index + 1) % self.targets.len();
        let guid = self.targets[self.world.target_index];
        self.status = format!("Target: {}", entity_name(&self.world, guid));
        // Appraise: request health via IdentifyObject.
        self.send_action(messages::action::IDENTIFY_OBJECT, &messages::body_identify_object(guid));
    }

    fn attack(&mut self) {
        if self.phase != Phase::InWorld {
            return;
        }
        if self.world.dead {
            self.status = "You are dead.".into();
            return;
        }
        let target = match self.targets.get(self.world.target_index).copied() {
            Some(g) => g,
            None => {
                self.status = "No target selected (Tab).".into();
                return;
            }
        };
        self.send_action(messages::action::TARGETED_MELEE_ATTACK, &messages::body_targeted_melee_attack(target));
    }

    // ================================================================== chat

    fn send_chat(&mut self) {
        let text = self.chat_buf.trim().to_string();
        self.chat_buf.clear();
        self.chat_focused = false;
        if text.is_empty() {
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
                let body = f.body(&self.account);
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
        if self.menu {
            if press {
                match ev.code {
                    KeyCode::Esc => self.menu = false,
                    KeyCode::Char('q') | KeyCode::Char('Q') => self.quit = true,
                    KeyCode::Char('l') | KeyCode::Char('L') => self.logout(),
                    _ => {}
                }
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
                    KeyCode::Esc => self.menu = true,
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
        match ev.code {
            KeyCode::Char(' ') => self.pending_jump = true,
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                self.run_lock = !self.run_lock;
                self.status = if self.run_lock { "Run: on" } else { "Run: off (walk)" }.into();
            }
            KeyCode::Char('`') => self.toggle_combat_mode(),
            KeyCode::Char('t') | KeyCode::Char('T') => self.attack(),
            KeyCode::Tab => self.cycle_target(),
            KeyCode::Esc => {
                self.menu = !self.menu;
                if self.menu {
                    self.keys_down.clear();
                }
            }
            KeyCode::Enter => {
                self.chat_focused = true;
                self.chat_buf.clear();
            }
            KeyCode::Char('/') => {
                self.chat_focused = true;
                self.chat_buf.clear();
                self.chat_buf.push('/');
            }
            KeyCode::Char(c) if c.is_ascii_graphic() => {
                self.chat_focused = true;
                self.chat_buf.clear();
                self.chat_buf.push(c);
            }
            _ => {}
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

    // =================================================================== tick

    /// Per-frame step: liveness timers + fixed-cadence movement.
    pub fn step(&mut self, now: Instant) {
        if self.quit {
            return;
        }
        // Reconnect when due.
        if self.net.is_none() && matches!(self.phase, Phase::Connecting) {
            if let Some(at) = self.reconnect_at {
                if now >= at {
                    if let Err(e) = self.do_connect() {
                        self.backoff = self.backoff.saturating_add(1);
                        let delay = Duration::from_secs(1u64 << self.backoff.min(4));
                        self.reconnect_at = Some(Instant::now() + delay);
                        self.status =
                            format!("Reconnect failed: {e} (retry in {}s)", delay.as_secs());
                    }
                }
            }
        }
        // Auth handshake: the server marks the session ready asynchronously
        // (after a slow DB write that runs past the ConnectRequest flush), so
        // retransmit the ConnectResponse until it matches, then bail out on a
        // hard deadline.
        if self.phase == Phase::Connecting
            && self.net.is_some()
            && self.connect_cookie.is_some()
        {
            let sent_at = self.connect_sent_at;
            let deadline = self.connect_deadline;
            let cookie = self.connect_cookie;
            if let (Some(sent_at), Some(deadline)) = (sent_at, deadline) {
                if now >= deadline {
                    self.drop_connection("handshake timeout (no character list)");
                } else if now.duration_since(sent_at) >= Duration::from_millis(500) {
                    if let (Some(cookie), Some(net)) = (cookie, self.net.as_mut()) {
                        let _ = net.retransmit_connect_response(cookie);
                    }
                    self.connect_sent_at = Some(now);
                }
            }
        }
        // Keepalive + silence detection.
        if self.net.is_some()
            && matches!(self.phase, Phase::CharSelect | Phase::Entering | Phase::InWorld)
        {
            if now.duration_since(self.last_echo) >= Duration::from_secs(10) {
                if let Some(net) = self.net.as_mut() {
                    let client_time =
                        now.duration_since(self.echo_clock).as_secs_f32();
                    let _ = net.send_echo_request(client_time);
                }
                self.last_echo = now;
            }
            if now.duration_since(self.last_rx) >= Duration::from_secs(20) {
                self.drop_connection("no response from server");
            }
        }
        // Movement at a fixed 50 ms cadence.
        if self.phase == Phase::InWorld
            && now.duration_since(self.last_tick) >= Duration::from_secs_f32(TICK)
        {
            let mut held = self.held_keys();
            held.jump = self.pending_jump;
            self.pending_jump = false;
            let walking = self.shift_held || !self.run_lock;
            if let Some(mv) = self.movement.as_mut() {
                let actions = mv.tick(TICK, &mut held, walking, now);
                self.send_move_actions(actions);
                self.refresh_targets();
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
