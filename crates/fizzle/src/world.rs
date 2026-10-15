//! World state: entities (radar), chat log, self vitals, target list.
//!
//! Position source is `UpdatePosition` (0xF748) — the primary authoritative
//! feed. Entities appear via `PlayerCreate`/`ObjectCreate` and vanish via
//! `ObjectDelete`. The landblock is `cell >> 16 & 0xffff`; block-local
//! coordinates are `[0, 192)` with +X east, +Y north.

use acproto::codec::Vec3;
use acproto::messages;

/// A tracked world entity (radar blip + potential target).
#[derive(Clone, Debug)]
pub struct Entity {
    pub guid: u32,
    pub name: Option<String>,
    pub cell: u32,
    pub pos: Vec3,
    pub is_player: bool,
    /// Health from Appraise (creature profile), if known.
    pub health: Option<u32>,
    pub health_max: Option<u32>,
}

impl Entity {
    #[must_use]
    pub fn block(&self) -> u32 {
        (self.cell >> 16) & 0xFFFF
    }
}

/// One line in the chat log.
#[derive(Clone, Debug)]
pub struct ChatLine {
    pub sender: String,
    pub text: String,
    pub style: ChatStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ChatStyle {
    Speech,
    Tell,
    Channel,
    System,
    Combat,
    Emote,
    Error,
    Own,
}

const MAX_CHAT: usize = 400;

#[derive(Clone, Debug, Default)]
pub struct ChatLog {
    lines: std::collections::VecDeque<ChatLine>,
}

impl ChatLog {
    pub fn push(&mut self, sender: &str, text: &str, style: ChatStyle) {
        if self.lines.len() >= MAX_CHAT {
            self.lines.pop_front();
        }
        self.lines.push_back(ChatLine { sender: sender.to_string(), text: text.to_string(), style });
    }

    pub fn lines(&self) -> impl DoubleEndedIterator<Item = &ChatLine> {
        self.lines.iter().rev()
    }
}

/// Self vitals, from `PlayerDescription` and health events.
#[derive(Clone, Debug, Default)]
pub struct Vitals {
    pub health: Option<u32>,
    pub health_max: Option<u32>,
    pub stamina: Option<u32>,
    pub stamina_max: Option<u32>,
    pub mana: Option<u32>,
    pub mana_max: Option<u32>,
    pub attrs: [u32; 6], // Str, End, Coord, Quick, Focus, Self
}

/// The tracked world.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub self_guid: Option<u32>,
    pub entities: std::collections::HashMap<u32, Entity>,
    pub chat: ChatLog,
    pub vitals: Vitals,
    pub target_index: usize,
    pub dead: bool,
}

/// Radar range (block-local units).
pub const RADAR_RANGE: f32 = 96.0;

impl World {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn upsert(&mut self, guid: u32, is_player: bool) -> &mut Entity {
        self.entities.entry(guid).or_insert_with(|| Entity {
            guid,
            name: None,
            cell: 0,
            pos: Vec3::default(),
            is_player,
            health: None,
            health_max: None,
        })
    }

    pub fn player_create(&mut self, guid: u32) {
        self.upsert(guid, true);
    }

    pub fn object_create(&mut self, guid: u32, name: Option<String>, cell: Option<u32>, pos: Option<Vec3>) {
        let e = self.upsert(guid, false);
        if let Some(n) = name {
            e.name = Some(n);
        }
        if let Some(c) = cell {
            e.cell = c;
        }
        if let Some(p) = pos {
            e.pos = p;
        }
    }

    pub fn object_delete(&mut self, guid: u32) {
        self.entities.remove(&guid);
    }

    pub fn update_position(&mut self, guid: u32, cell: u32, pos: Vec3) {
        let is_self = Some(guid) == self.self_guid;
        if is_self {
            // Own position is tracked in `Movement`; the radar skips self.
            return;
        }
        let e = self.upsert(guid, false);
        e.cell = cell;
        e.pos = pos;
    }

    pub fn apply_appraise(&mut self, appraise: &messages::Appraise) {
        if let Some(e) = self.entities.get_mut(&appraise.guid) {
            e.health = Some(appraise.health);
            e.health_max = Some(appraise.health_max);
        }
    }

    /// Self health from a PlayerDescription event.
    pub fn set_vitals_from_description(&mut self, d: &messages::PlayerDescription) {
        let v = &d.vitals;
        let vm = &d.vitals_max;
        self.vitals.health = Some(v[0]);
        self.vitals.health_max = if vm[0] > 0 { Some(vm[0]) } else { Some(v[0]) };
        self.vitals.stamina = Some(v[1]);
        self.vitals.stamina_max = if vm[1] > 0 { Some(vm[1]) } else { Some(v[1]) };
        self.vitals.mana = Some(v[2]);
        self.vitals.mana_max = if vm[2] > 0 { Some(vm[2]) } else { Some(v[2]) };
        self.vitals.attrs = d.attrs;
        if v[0] > 0 {
            self.dead = false;
        }
    }

    /// Self health from UpdateHealth event (f32 0..1 of max).
    pub fn update_health_event(&mut self, guid: u32, pct: f32) {
        if Some(guid) != self.self_guid {
            return;
        }
        if let Some(max) = self.vitals.health_max {
            let h = (pct.clamp(0.0, 1.0) * max as f32) as u32;
            self.vitals.health = Some(h);
            self.dead = h == 0;
        }
    }

    /// Self damage from a DefenderNotification.
    pub fn apply_defender(&mut self, damage: u32) {
        let h = self.vitals.health.unwrap_or(0);
        let new = h.saturating_sub(damage);
        self.vitals.health = Some(new);
        if new == 0 {
            self.dead = true;
        }
    }

    /// Server-authoritative vitals from `PrivateUpdateAttribute2ndLevel`
    /// (0x02E9). The server sends `Health = 0` on death and restores 75% of
    /// max on the death teleport (revive), so both the health bar and the
    /// dead state follow the server rather than local damage tracking.
    pub fn set_vital(&mut self, vital: u32, value: u32) {
        match vital {
            messages::vital::HEALTH => {
                self.vitals.health = Some(value);
                self.dead = value == 0;
            }
            messages::vital::STAMINA => self.vitals.stamina = Some(value),
            messages::vital::MANA => self.vitals.mana = Some(value),
            _ => {}
        }
    }

    /// Targets: non-self entities in the same landblock within radar range,
    /// nearest first.
    #[must_use]
    pub fn targets(&self, self_cell: u32, self_pos: &Vec3) -> Vec<u32> {
        let block = (self_cell >> 16) & 0xFFFF;
        let mut v: Vec<(f32, u32)> = self
            .entities
            .values()
            .filter(|e| Some(e.guid) != self.self_guid && e.block() == block)
            .map(|e| {
                let d = ((e.pos.x - self_pos.x).powi(2) + (e.pos.y - self_pos.y).powi(2) + (e.pos.z - self_pos.z).powi(2))
                    .sqrt();
                (d, e.guid)
            })
            .filter(|(d, _)| *d <= RADAR_RANGE)
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter().map(|(_, g)| g).collect()
    }

    /// Clamp the target index into range (call after entity changes).
    pub fn clamp_target(&mut self, count: usize) {
        if self.target_index >= count {
            self.target_index = count.saturating_sub(1);
        }
    }
}
