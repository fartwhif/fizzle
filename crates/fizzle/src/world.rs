//! World state: entities (radar/targets), chat log, self vitals, inventory,
//! vendor/trade/loot, social (friends/squelch/contracts/fellowship/allegiance/
//! house), examine, skills/spells, and the door + far-cull bookkeeping.
//!
//! Position source is `UpdatePosition` (0xF748) — the primary authoritative
//! feed. Entities appear via `PlayerCreate`/`ObjectCreate` and vanish via
//! `ObjectDelete`. The landblock is `cell >> 16 & 0xffff`; block-local
//! coordinates are `[0, 192)` with +X east, +Y north.
//!
//! This module stores only fizzle-side state (no `acproto` types), so the
//! session layer (`app.rs`) maps the wire messages into these structures.

use std::time::Instant;

use acproto::codec::Vec3;
use acproto::messages;

// Retail motion commands for doors (door On / Off).
pub const DOOR_MOTION_ON: u32 = 0x4000000b;
pub const DOOR_MOTION_OFF: u32 = 0x4000000c;
/// `Physics::Ethereal` bit (a door that can be walked through when open).
pub const PHYSICS_ETHEREAL: u32 = 0x00020000;
/// Item type for a Door.
pub const ITEM_TYPE_DOOR: u32 = 128;
/// Item type for a Container (chest / corpse / side pack).
pub const ITEM_TYPE_CONTAINER: u32 = 512;
/// Item type for a Creature (retail `ItemType::Creature` = 16).
pub const ITEM_TYPE_CREATURE: u32 = 16;

/// The single unified target: exactly one thing is targeted at a time — a
/// world entity or an item in a pack/shop/trade/opened container. It is
/// shown in exactly one place (the status bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A world entity (creature, door, container, ground item, player).
    World(u32),
    /// An item: `(guid, source)` where `source` is one of the
    /// `TARGET_ITEM_*` constants.
    Item(u32, u8),
}

/// The source list of a targeted item (`Target::Item`'s `source`).
pub const TARGET_ITEM_PACK: u8 = 1;
pub const TARGET_ITEM_SHOP: u8 = 2;
pub const TARGET_ITEM_TRADE: u8 = 3;
pub const TARGET_ITEM_CONTAINER: u8 = 4;
/// Main pack capacity (retail).
pub const MAIN_PACK_CAPACITY: usize = 102;

/// Far-cull: entities farther than `CULL_DISTANCE` and unseen for
/// `CULL_GRACE_SECS` are dropped (throttled to ≤1/s by the session).
pub const CULL_DISTANCE: f32 = 192.0;
pub const CULL_GRACE_SECS: f64 = 25.0;

/// A world item (self inventory, vendor, trade, or loot). Fizzle-side copy of
/// the `ObjectCreate` / inventory item fields the UI shows. Some fields are
/// not yet consumed by the UI.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct ItemState {
    pub guid: u32,
    pub name: String,
    pub item_type: u32,
    pub value: u32,
    pub stack_size: u32,
    pub items_capacity: u32,
    /// Current equip loc (an `EquipMask`).
    pub cur_loc: i32,
    /// Valid equip locs bitmask.
    pub valid_locs: i32,
    pub container_id: u32,
    pub wielder_id: u32,
    pub mtable_id: Option<u32>,
    pub setup_id: Option<u32>,
    pub model_parts: Option<Vec<(usize, u32)>>,
    pub icon_id: u32,
    /// Coin value (for a coin stack).
    pub coin_value: i64,
    /// Retail motion command (door On/Off etc.); 0 if none.
    pub motion_cmd: u32,
    pub physics_state: u32,
    pub is_vendor: bool,
}

/// A tracked world entity (radar blip + potential target).
#[derive(Clone, Debug)]
pub struct Entity {
    pub guid: u32,
    pub name: Option<String>,
    pub cell: u32,
    pub pos: Vec3,
    /// Orientation quaternion (w, x, y, z); identity until the server sends
    /// one. Feeds the 3D renderer's `Actor.quat`.
    pub rot: [f32; 4],
    pub is_player: bool,
    /// Health from Appraise (creature profile), if known.
    pub health: Option<u32>,
    pub health_max: Option<u32>,
    pub mana: Option<u32>,
    pub mana_max: Option<u32>,
    // Item / door fields (from ObjectCreate).
    pub item_type: u32,
    pub setup_id: Option<u32>,
    pub mtable_id: Option<u32>,
    pub model_parts: Option<Vec<(usize, u32)>>,
    pub motion_cmd: u32,
    pub physics_state: u32,
    pub is_vendor: bool,
    /// Last time this entity was seen (for the far-cull pass).
    pub last_seen: Instant,
}

impl Entity {
    #[must_use]
    pub fn block(&self) -> u32 {
        (self.cell >> 16) & 0xFFFF
    }

    #[must_use]
    pub fn is_door(&self) -> bool {
        self.item_type == ITEM_TYPE_DOOR
    }

    /// Whether a door is currently open (motion door-ON, or the Ethereal
    /// physics bit is set — an open door can be walked through).
    #[must_use]
    pub fn door_open(&self) -> bool {
        self.motion_cmd == DOOR_MOTION_ON || (self.physics_state & PHYSICS_ETHEREAL) != 0
    }

    #[must_use]
    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| format!("#{}", self.guid))
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
    /// Monotonic count of every line ever pushed (unbounded, unlike the
    /// capped `lines` deque): plain mode uses it to print only new lines.
    pub total: u64,
}

impl ChatLog {
    pub fn push(&mut self, sender: &str, text: &str, style: ChatStyle) {
        if self.lines.len() >= MAX_CHAT {
            self.lines.pop_front();
        }
        self.lines.push_back(ChatLine { sender: sender.to_string(), text: text.to_string(), style });
        self.total += 1;
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Chronological iterator over the retained lines (oldest first).
    pub fn iter(&self) -> impl Iterator<Item = &ChatLine> {
        self.lines.iter()
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
    /// Total experience (PropertyInt64 TOTAL_EXPERIENCE).
    pub xp: i64,
    /// Vitae death-penalty multiplier (1.0 = no penalty), from the
    /// enchantment registry.
    pub vitae: Option<f32>,
}

/// The player's inventory (main pack + equipped + purse). Some fields are not yet
#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct Inventory {
    /// Items in the main pack.
    pub pack: Vec<ItemState>,
    /// Equipped items (by `cur_loc`).
    pub equipped: Vec<ItemState>,
    /// Side packs (secondary container guids).
    pub packs: Vec<u32>,
    pub coin_value: i64,
    pub strength: i32,
    pub encumbrance: i32,
    /// Whether the paper-doll/pack view is open.
    pub open: bool,
    /// Selected pack grid cell (mouse).
    pub sel: usize,
    /// Selected paper-doll slot (mouse).
    pub doll_sel: usize,
}

/// One line in the vendor list. Some fields are not yet consumed by the UI.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct VendorItem {
    pub guid: u32,
    pub name: String,
    pub item_type: u32,
    pub value: u32,
    pub stack_size: u32,
    pub icon_id: u32,
    pub buy_price: f32,
    pub sell_price: f32,
}

/// The open vendor shop. Some fields are not yet consumed by the UI.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct VendorState {
    pub guid: u32,
    pub name: String,
    pub buy_mult: f32,
    pub sell_mult: f32,
    pub items: Vec<VendorItem>,
    /// Purchases staged: (vendor item guid, amount).
    pub buying: Vec<(u32, u32)>,
    /// Sales staged: (player item guid, amount).
    pub selling: Vec<(u32, u32)>,
    /// Tab: 0 = items, 1 = buy, 2 = sell.
    pub tab: usize,
    /// Item-type filter for the items tab (0 = all).
    pub category: u32,
    pub page: usize,
    /// Selected row in the active tab's list (mouse).
    pub sel: usize,
    /// First category the bottom row's window shows (‹/›/wheel).
    pub cat_scroll: usize,
}

/// An open trade negotiation.
#[derive(Clone, Debug, Default)]
pub struct TradeState {
    pub partner: u32,
    pub partner_name: String,
    /// Our side item guids.
    pub our: Vec<u32>,
    /// Their side item guids.
    pub their: Vec<u32>,
    pub we_accepted: bool,
    pub they_accepted: bool,
    /// Last `TradeFailure` weenie-error code.
    pub failure: Option<u32>,
    /// Selected row in the "our" list (mouse).
    pub sel: usize,
}

/// An open ground container (lootable corpse / chest).
#[derive(Clone, Debug, Default)]
pub struct ContainerState {
    pub guid: u32,
    pub name: String,
    pub items: Vec<ItemState>,
    /// Selected row (mouse).
    pub sel: usize,
}

#[derive(Clone, Debug)]
pub struct Contract {
    pub id: u64,
    pub title: String,
    pub stage: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Fellowship {
    pub leader: String,
    pub members: Vec<String>,
    /// Whether the fellowship accepts recruits (openness).
    pub open: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Allegiance {
    pub leader: String,
    pub level: u32,
}

#[derive(Clone, Debug, Default)]
pub struct House {
    pub guid: u32,
    pub name: String,
    pub district: String,
}

/// The player's social relationships.
#[derive(Clone, Debug, Default)]
pub struct Social {
    pub friends: Vec<String>,
    pub squelch: Vec<String>,
    pub contracts: Vec<Contract>,
    pub fellowship: Option<Fellowship>,
    pub allegiance: Option<Allegiance>,
    pub house: Option<House>,
    /// Known title ids.
    pub titles: Vec<i32>,
    /// The currently-wielded title id.
    pub title: i32,
}

/// The appraised/examined target.
#[derive(Clone, Debug, Default)]
pub struct Examine {
    pub guid: u32,
    pub name: String,
    pub description: String,
    pub item_type: u32,
    pub level: i32,
    pub value: u32,
    pub encumbrance: i32,
    pub bonded: bool,
    pub attuned: bool,
    pub max_health: i32,
    pub health: i32,
    pub attrs: [i32; 6],
    pub vitals: [i32; 3],
    pub damage_min: i32,
    pub damage_var: i32,
    pub skill: u32,
    pub resist: [i32; 10],
    pub armor_levels: [i32; 8],
    /// Whether this examine is for the current target (vs. an item).
    pub valid: bool,
}

/// The retail "AlwaysTrained" skills every character owns (ACE
/// `Player_Skills.AlwaysTrained`): Arcane Lore, Magic Defense, Jump, Run,
/// Loyalty, Salvaging. The server omits some of these and sends the rest at
/// rank 0, so seed the absent ones to keep the Skills panel honest.
const INNATE_SKILLS: [u32; 6] = [14, 15, 22, 24, 36, 40];

/// Skills, spells, and the 8-tab spell bar.
#[derive(Clone, Debug, Default)]
pub struct Skills {
    /// (skill id, rank) for every known retail skill.
    pub skills: Vec<(u32, i32)>,
    /// Known spell ids.
    pub spells: Vec<u32>,
    /// Up to 8 spell-bar tabs; each a list of spell ids (0 = empty slot).
    pub spell_bar: Vec<Vec<u32>>,
    /// Active spell filter ids.
    pub filters: Vec<u32>,
    /// Currently selected tab (0..8).
    pub tab: usize,
    /// Currently selected slot within the tab.
    pub slot: usize,
}

/// Combat + stance state, session-managed and shown in the You pane.
///
/// `mode` is the retail `CombatMode` bitmask (see `acproto::messages::combat_mode`);
/// the default mode is derived from the equipped weapon's `cur_loc`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Combat {
    /// Combat-mode bitmask: 1 NonCombat / 2 Melee / 4 Missile / 8 Magic.
    pub mode: u32,
    /// Attack height: 1 Low / 2 Medium / 3 High.
    pub height: u32,
    /// Attack power (0.0..=1.0); reused as missile accuracy.
    pub power: f32,
    /// Current stance motion command (`motion::command::*`), 0 when none.
    pub stance: u32,
}

impl Combat {
    /// A fresh combat state: Medium height, 0.5 power, NonCombat, no stance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            mode: acproto::messages::combat_mode::NON_COMBAT,
            height: 2,
            power: 0.5,
            stance: 0,
        }
    }
}

/// One page of an open book (text + author, as fetched).
#[derive(Clone, Debug, Default)]
pub struct BookPage {
    pub index: u32,
    pub author: String,
    pub text: String,
}

/// An open book (from `BookDataResponse`), with a read/write cursor.
#[derive(Clone, Debug, Default)]
pub struct BookState {
    pub guid: u32,
    pub name: String,
    pub max_pages: u32,
    pub max_chars: u32,
    pub pages: Vec<BookPage>,
    pub inscription: String,
    pub author: String,
    /// Currently displayed page index.
    pub current: usize,
    /// Whether the write/edit mode is active.
    pub editing: bool,
    /// The page text being edited.
    pub buffer: String,
    pub open: bool,
}

/// The tracked world.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub self_guid: Option<u32>,
    /// The self character's name (set at world entry; used for targeting
    /// readouts and the You pane).
    pub self_name: String,
    pub entities: std::collections::HashMap<u32, Entity>,
    pub chat: ChatLog,
    pub vitals: Vitals,
    pub target_index: usize,
    pub dead: bool,
    // NEW fizzle-side state:
    pub inventory: Inventory,
    pub skills: Skills,
    pub social: Social,
    pub examine: Examine,
    pub vendor: VendorState,
    pub trade: TradeState,
    pub container: ContainerState,
    /// Combat/stance/height/power state.
    pub combat: Combat,
    /// Live target health (from the `QueryHealth`/`UpdateHealth` heartbeat).
    pub target_health: Option<u32>,
    pub target_health_max: Option<u32>,
    /// Last measured ping RTT in milliseconds (None until first response).
    pub ping_rtt_ms: Option<f64>,
    /// The name of the last player to `/t`ell us (for reply prefill).
    pub last_teller: Option<String>,
    /// The open book (if any).
    pub book: Option<BookState>,
    /// One-shot status flash (cleared after it is shown once).
    pub flash: Option<String>,
}

/// Radar range (block-local units).
pub const RADAR_RANGE: f32 = 96.0;

impl World {
    #[must_use]
    pub fn new() -> Self {
        let mut w = Self::default();
        w.combat = Combat::new();
        w
    }

    fn upsert(&mut self, guid: u32, is_player: bool) -> &mut Entity {
        self.entities.entry(guid).or_insert_with(|| Entity {
            guid,
            name: None,
            cell: 0,
            pos: Vec3::default(),
            rot: [1.0, 0.0, 0.0, 0.0],
            is_player,
            health: None,
            health_max: None,
            mana: None,
            mana_max: None,
            item_type: 0,
            setup_id: None,
            mtable_id: None,
            model_parts: None,
            motion_cmd: 0,
            physics_state: 0,
            is_vendor: false,
            last_seen: Instant::now(),
        })
    }

    pub fn player_create(&mut self, guid: u32) {
        self.upsert(guid, true).last_seen = Instant::now();
    }

    /// Establish `guid` as the self entity: a targetable player (the radar,
    /// tactical map and far-cull skip it; the 3D view renders it from the
    /// movement state), named `name`. Idempotent — called at world entry.
    pub fn set_self(&mut self, guid: u32, name: String) {
        self.self_guid = Some(guid);
        self.self_name = name.clone();
        let e = self.upsert(guid, true);
        e.is_player = true;
        e.name = Some(name);
        e.last_seen = Instant::now();
    }

    pub fn object_create(&mut self, guid: u32, name: Option<String>, cell: Option<u32>, pos: Option<Vec3>, item: Option<&ItemState>, rot: Option<[f32; 4]>) {
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
        if let Some(r) = rot {
            e.rot = r;
        }
        if let Some(i) = item {
            e.item_type = i.item_type;
            e.setup_id = i.setup_id;
            e.mtable_id = i.mtable_id;
            e.model_parts = i.model_parts.clone();
            e.motion_cmd = i.motion_cmd;
            e.physics_state = i.physics_state;
            e.is_vendor = i.is_vendor;
        }
        e.last_seen = Instant::now();
    }

    pub fn object_delete(&mut self, guid: u32) {
        self.entities.remove(&guid);
        // A closed door/vendor/trade partner removal should also drop related
        // session state; the session does that.
    }

    pub fn update_position(&mut self, guid: u32, cell: u32, pos: Vec3, rot: Option<[f32; 4]>) {
        let is_self = Some(guid) == self.self_guid;
        if is_self {
            return;
        }
        let e = self.upsert(guid, false);
        e.cell = cell;
        e.pos = pos;
        if let Some(r) = rot {
            e.rot = r;
        }
        e.last_seen = Instant::now();
    }

    pub fn apply_appraise(&mut self, a: &messages::Appraise) {
        if let Some(e) = self.entities.get_mut(&a.guid) {
            e.health = Some(a.health);
            e.health_max = Some(a.health_max);
            e.mana = a.item_mana.map(|v| v as u32);
            e.mana_max = a.item_mana_max.map(|v| v as u32);
        }
        if !a.success {
            return;
        }
        let x = &mut self.examine;
        x.guid = a.guid;
        x.item_type = self
            .entities
            .get(&a.guid)
            .map(|e| e.item_type)
            .or_else(|| {
                self.inventory
                    .pack
                    .iter()
                    .chain(self.inventory.equipped.iter())
                    .find(|i| i.guid == a.guid)
                    .map(|i| i.item_type)
            })
            .or_else(|| self.vendor.items.iter().find(|i| i.guid == a.guid).map(|i| i.item_type))
            .or_else(|| self.container.items.iter().find(|i| i.guid == a.guid).map(|i| i.item_type))
            .unwrap_or(0);
        // The name must never be blank: the Appraise carries no name, so it
        // comes from the entity map. If the entity is absent at Appraise
        // time (a pack/vendor/loot item whose ObjectCreate is still in
        // flight), fall back to the fizzle-side copy in the main pack,
        // equipped, vendor shop, or open container, then to the guid.
        x.name = self
            .entities
            .get(&a.guid)
            .map(|e| e.display_name())
            .or_else(|| {
                self.inventory
                    .pack
                    .iter()
                    .chain(self.inventory.equipped.iter())
                    .find(|i| i.guid == a.guid)
                    .map(|i| i.name.clone())
            })
            .or_else(|| self.vendor.items.iter().find(|i| i.guid == a.guid).map(|i| i.name.clone()))
            .or_else(|| self.container.items.iter().find(|i| i.guid == a.guid).map(|i| i.name.clone()))
            .unwrap_or_else(|| format!("#{}", a.guid));
        x.description = a.description.clone();
        x.level = a.level.unwrap_or(0);
        x.value = a.value.unwrap_or(0) as u32;
        x.encumbrance = a.encumbrance.unwrap_or(0);
        x.bonded = a.bonded;
        x.attuned = a.attuned;
        if let Some(c) = a.creature {
            x.max_health = c.max_health;
            x.health = c.health;
            x.attrs = c.attrs;
            x.vitals = [c.vitals[0], c.vitals[1], 0];
        }
        if let Some(w) = a.weapon {
            x.damage_min = w.damage_min;
            x.damage_var = w.damage_var;
            x.skill = w.skill;
        }
        if let Some(arm) = a.armor {
            let mut r = [0i32; 10];
            r[..8].copy_from_slice(&arm.resist.map(|v| v as i32));
            x.resist = r;
        }
        if let Some(al) = a.armor_levels {
            x.armor_levels = al[..8].try_into().unwrap_or_default();
        }
        x.valid = true;
    }

    /// Skills, spells, spell bar, and burden/coin from PlayerDescription.
    pub fn apply_player_description_tail(&mut self, d: &messages::PlayerDescription) {
        self.skills.skills = d.skills.clone();
        // Seed the AlwaysTrained innate skills the server omits (fresh
        // characters own these at rank 0; a fresh character's whole list is
        // rank 0, so the panel must not hide them).
        for innate in INNATE_SKILLS {
            if !self.skills.skills.iter().any(|(id, _)| *id == innate) {
                self.skills.skills.push((innate, 0));
            }
        }
        self.skills.spells = d.spells.clone();
        self.skills.spell_bar = d.spell_bar.clone();
        self.skills.filters = d.spell_filters.clone();
        self.inventory.coin_value = d.coin_value;
        self.inventory.strength = d.strength;
        self.inventory.encumbrance = d.encumbrance;
        // Total experience (PropertyInt64 key 1).
        self.vitals.xp = d
            .int64s
            .iter()
            .find(|(k, _)| *k == acproto::messages::property_int64::TOTAL_EXPERIENCE)
            .map(|(_, v)| *v)
            .unwrap_or(0);
        // Vitae death-penalty multiplier (spell 666 in the enchantment registry).
        self.vitals.vitae = d
            .enchantments
            .iter()
            .find(|e| e.spell_id == 666)
            .map(|e| e.stat_mod_value);
    }

    /// Display name of a tracked entity (empty when unknown).
    #[must_use]
    pub fn entity_name(&self, guid: u32) -> String {
        if Some(guid) == self.self_guid && !self.self_name.is_empty() {
            return self.self_name.clone();
        }
        self.entities.get(&guid).map(|e| e.display_name()).unwrap_or_default()
    }

    /// Best-effort item state for a guid (from entity data, if tracked).
    #[must_use]
    pub fn item_state(&self, guid: u32) -> ItemState {
        match self.entities.get(&guid) {
            Some(e) => ItemState {
                guid,
                name: e.display_name(),
                item_type: e.item_type,
                mtable_id: e.mtable_id,
                setup_id: e.setup_id,
                model_parts: e.model_parts.clone(),
                motion_cmd: e.motion_cmd,
                physics_state: e.physics_state,
                is_vendor: e.is_vendor,
                ..Default::default()
            },
            None => ItemState { guid, ..Default::default() },
        }
    }

    /// Insert/refresh an item in the main pack (deduped by guid).
    pub fn upsert_pack_item(&mut self, item: ItemState) {
        self.inventory.pack.retain(|i| i.guid != item.guid);
        self.inventory.pack.push(item);
    }

    /// Insert/refresh an equipped item (deduped by guid).
    pub fn upsert_equipped_item(&mut self, item: ItemState) {
        self.inventory.equipped.retain(|i| i.guid != item.guid);
        self.inventory.equipped.push(item);
    }

    /// Unequip an item (move it from the paper doll to the main pack).
    pub fn unequip_to_pack(&mut self, guid: u32) {
        if let Some(pos) = self.inventory.equipped.iter().position(|i| i.guid == guid) {
            let mut item = self.inventory.equipped.remove(pos);
            item.cur_loc = 0;
            self.inventory.pack.push(item);
        }
    }

    /// Equip an item from the main pack to an EquipMask location.
    pub fn equip_from_pack(&mut self, guid: u32, location: i32) {
        if let Some(pos) = self.inventory.pack.iter().position(|i| i.guid == guid) {
            let mut item = self.inventory.pack.remove(pos);
            item.cur_loc = location;
            self.inventory.equipped.push(item);
        }
    }

    /// Remove an item from the inventory (dropped into the 3D world).
    pub fn drop_item(&mut self, guid: u32) {
        self.inventory.pack.retain(|i| i.guid != guid);
        self.inventory.equipped.retain(|i| i.guid != guid);
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

    /// Health from an UpdateHealth event (f32 0..1 of max). Applies to self,
    /// or — when the guid is the current target — to the live target-health
    /// display (driven by the `QueryHealth` heartbeat).
    pub fn update_health_event(&mut self, guid: u32, pct: f32, target: Option<u32>) {
        let pct = pct.clamp(0.0, 1.0);
        if Some(guid) == self.self_guid {
            if let Some(max) = self.vitals.health_max {
                let h = (pct * max as f32) as u32;
                self.vitals.health = Some(h);
                self.dead = h == 0;
            }
            return;
        }
        if Some(guid) == target {
            if let Some(max) = self.target_health_max {
                self.target_health = Some((pct * max as f32) as u32);
            }
            // Also refresh the tracked entity's health (examine / radar).
            if let Some(e) = self.entities.get_mut(&guid) {
                if let Some(emax) = e.health_max {
                    e.health = Some((pct * emax as f32) as u32);
                }
            }
        }
    }

    /// The retail default combat mode from the equipped weapon
    /// (`GetDefaultCombatMode`): a missile weapon in hand → Missile, a wand
    /// (Held slot) → Magic, otherwise Melee.
    #[must_use]
    pub fn default_combat_mode(&self) -> u32 {
        use acproto::messages::combat_mode as cm;
        const MISSILE_WEAPON: i32 = 0x0040_0000;
        const HELD: i32 = 0x0100_0000;
        let mut missile = false;
        let mut magic = false;
        for item in &self.inventory.equipped {
            if item.cur_loc & MISSILE_WEAPON != 0 {
                missile = true;
            } else if item.cur_loc & HELD != 0 {
                magic = true;
            }
        }
        if missile {
            cm::MISSILE
        } else if magic {
            cm::MAGIC
        } else {
            cm::MELEE
        }
    }

    /// Whether a chat sender name is in the squelch (filter) list.
    #[must_use]
    pub fn squelched(&self, name: &str) -> bool {
        !name.is_empty() && self.social.squelch.iter().any(|s| s == name)
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
    /// (0x02E9).
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

    /// Targets filtered by kind, nearest first. `kind`:
    /// 0 = all creatures, 1 = objects (non-creature), 2 = players.
    #[must_use]
    pub fn targets_of_kind(&self, self_cell: u32, self_pos: &Vec3, kind: u8) -> Vec<u32> {
        let block = (self_cell >> 16) & 0xFFFF;
        let is_creature = |e: &Entity| e.is_player || e.item_type == ITEM_TYPE_CREATURE || e.item_type == 0;
        let mut v: Vec<(f32, u32)> = self
            .entities
            .values()
            .filter(|e| e.block() == block)
            .filter(|e| {
                let is_self = Some(e.guid) == self.self_guid;
                match kind {
                    // Objects: never self.
                    1 => !is_creature(e) && !is_self,
                    // Players: include self (you can target yourself).
                    2 => e.is_player || is_self,
                    // Creatures: include self (retail cycles you in the compass).
                    _ => is_creature(e) || is_self,
                }
            })
            .map(|e| {
                let d = if Some(e.guid) == self.self_guid {
                    // Self is at zero range (its entity position is the spawn
                    // snapshot, not live; the movement state owns it).
                    0.0
                } else {
                    ((e.pos.x - self_pos.x).powi(2) + (e.pos.y - self_pos.y).powi(2) + (e.pos.z - self_pos.z).powi(2))
                        .sqrt()
                };
                (d, e.guid)
            })
            .filter(|(d, _)| *d <= RADAR_RANGE)
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter().map(|(_, g)| g).collect()
    }

    // ---------------------------------------------------------- far culling

    /// Global (block-decomposed) distance between self and an entity, in
    /// world units. Same-block uses the local offset; cross-block adds the
    /// 192 u block offset.
    #[must_use]
    fn dist(&self, self_cell: u32, self_pos: &Vec3, e: &Entity) -> f32 {
        let sb = (self_cell >> 16) & 0xFFFF;
        let eb = e.block();
        if sb == eb {
            return ((e.pos.x - self_pos.x).powi(2) + (e.pos.y - self_pos.y).powi(2) + (e.pos.z - self_pos.z).powi(2)).sqrt();
        }
        let sbx = (sb & 0xFFFF) as f32;
        let sby = (sb >> 16) as f32;
        let ebx = (eb & 0xFFFF) as f32;
        let eby = (eb >> 16) as f32;
        let dx = (sbx - ebx) * 192.0 + (e.pos.x - self_pos.x);
        let dy = (sby - eby) * 192.0 + (e.pos.y - self_pos.y);
        let dz = e.pos.z - self_pos.z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// Drop entities beyond `CULL_DISTANCE` that have been unseen for
    /// `CULL_GRACE_SECS`. `keep` are guards (self, trade partner, open
    /// container, shopped vendor) that must never be culled. Returns the
    /// removed guids.
    #[must_use]
    pub fn far_cull(&mut self, self_cell: u32, self_pos: &Vec3, keep: &[u32], now: Instant) -> Vec<u32> {
        let mut removed = Vec::new();
        for (g, e) in self.entities.iter() {
            if Some(*g) == self.self_guid || keep.iter().any(|k| *k == *g) {
                continue;
            }
            if self.dist(self_cell, self_pos, e) > CULL_DISTANCE
                && now.duration_since(e.last_seen).as_secs_f64() > CULL_GRACE_SECS
            {
                removed.push(*g);
            }
        }
        for g in &removed {
            self.entities.remove(g);
        }
        removed
    }
}
