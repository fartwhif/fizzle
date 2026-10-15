// Static character-generation data.
//
// Extracted from the live ACE server's `client_portal.dat`, CharGen record
// 0x0E000002 (a custom-fork dat, so names/credits differ from retail AC).
// Provenance: /tmp/extract_chargen.py -> /tmp/chargen.json (verified against
// ACE.DatLoader @ 47edade). Regenerate with that script if the dat changes.

/// A character template ("class preset") with starting attributes.
#[derive(Clone, Copy, Debug)]
pub struct Template {
    /// Display name, e.g. "Soldier".
    pub name: &'static str,
    /// Starting attributes [Strength, Endurance, Coordination, Quickness, Focus, Self].
    pub attributes: [u32; 6],
}

/// A heritage (race) offered at character creation.
#[derive(Clone, Copy, Debug)]
pub struct Heritage {
    /// HeritageGroup id sent in CharacterCreate (1..=13).
    pub id: u32,
    pub name: &'static str,
    /// Maximum sum of the six starting attributes.
    pub attribute_credits: u32,
    /// Skill points banked when no skills are trained.
    pub skill_credits: u32,
    /// Starter-area indexes (into [`STARTER_AREAS`]) offered for this heritage.
    pub start_areas: &'static [u32],
    pub templates: &'static [Template],
}

/// A starter area the server can place a new character in.
#[derive(Clone, Copy, Debug)]
pub struct StarterArea {
    pub name: &'static str,
    /// First location cell id (display only; the server chooses the exact spot).
    pub cell: u32,
}

pub static STARTER_AREAS: &[StarterArea] = &[
    StarterArea { name: "Holtburg", cell: 0x860201AD },
    StarterArea { name: "Shoushi", cell: 0x7F0301AD },
    StarterArea { name: "Yaraq", cell: 0x8C0401AD },
    StarterArea { name: "Sanamar", cell: 0x720201AD },
    StarterArea { name: "OlthoiLair", cell: 0xE6D3001E },
];

// Template lists live in their own statics so `Heritage.templates` can borrow them.
static TPLS_1: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
    Template { name: "Bow Hunter", attributes: [40, 30, 100, 100, 50, 10] },
    Template { name: "Swashbuckler", attributes: [100, 40, 100, 50, 20, 20] },
    Template { name: "Life Caster", attributes: [40, 50, 10, 30, 100, 100] },
    Template { name: "War Mage", attributes: [50, 40, 10, 30, 100, 100] },
    Template { name: "Wayfarer", attributes: [30, 30, 100, 100, 60, 10] },
    Template { name: "Soldier", attributes: [100, 60, 100, 50, 10, 10] },
];
static TPLS_2: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
    Template { name: "Bow Hunter", attributes: [40, 30, 100, 100, 50, 10] },
    Template { name: "Swashbuckler", attributes: [100, 40, 100, 50, 20, 20] },
    Template { name: "Life Caster", attributes: [40, 50, 10, 30, 100, 100] },
    Template { name: "War Mage", attributes: [50, 40, 10, 30, 100, 100] },
    Template { name: "Wayfarer", attributes: [30, 30, 100, 100, 60, 10] },
    Template { name: "Soldier", attributes: [100, 60, 100, 50, 10, 10] },
];
static TPLS_3: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
    Template { name: "Bow Hunter", attributes: [40, 30, 100, 100, 50, 10] },
    Template { name: "Swashbuckler", attributes: [100, 40, 100, 50, 20, 20] },
    Template { name: "Life Caster", attributes: [40, 50, 10, 30, 100, 100] },
    Template { name: "War Mage", attributes: [50, 40, 10, 30, 100, 100] },
    Template { name: "Wayfarer", attributes: [30, 30, 100, 100, 60, 10] },
    Template { name: "Soldier", attributes: [100, 60, 100, 50, 10, 10] },
];
static TPLS_4: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
    Template { name: "Bow Hunter", attributes: [40, 30, 100, 100, 50, 10] },
    Template { name: "Swashbuckler", attributes: [100, 40, 100, 50, 20, 20] },
    Template { name: "Life Caster", attributes: [40, 50, 10, 30, 100, 100] },
    Template { name: "War Mage", attributes: [50, 40, 10, 30, 100, 100] },
    Template { name: "Wayfarer", attributes: [30, 30, 100, 100, 60, 10] },
    Template { name: "Soldier", attributes: [100, 60, 100, 50, 10, 10] },
];
static TPLS_5: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
    Template { name: "Bow Hunter", attributes: [40, 30, 100, 100, 50, 10] },
    Template { name: "Swashbuckler", attributes: [100, 40, 100, 50, 20, 20] },
    Template { name: "Life Caster", attributes: [40, 50, 10, 30, 100, 100] },
    Template { name: "War Mage", attributes: [50, 40, 10, 30, 100, 100] },
    Template { name: "Wayfarer", attributes: [30, 30, 100, 100, 60, 10] },
    Template { name: "Soldier", attributes: [100, 60, 100, 50, 10, 10] },
];
static TPLS_6: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
    Template { name: "Bow Hunter", attributes: [40, 30, 100, 100, 50, 10] },
    Template { name: "Swashbuckler", attributes: [100, 40, 100, 50, 20, 20] },
    Template { name: "Life Caster", attributes: [40, 50, 10, 30, 100, 100] },
    Template { name: "War Mage", attributes: [50, 40, 10, 30, 100, 100] },
    Template { name: "Wayfarer", attributes: [30, 30, 100, 100, 60, 10] },
    Template { name: "Soldier", attributes: [100, 60, 100, 50, 10, 10] },
];
static TPLS_7: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
    Template { name: "Bow Hunter", attributes: [40, 30, 100, 100, 50, 10] },
    Template { name: "Swashbuckler", attributes: [100, 40, 100, 50, 20, 20] },
    Template { name: "Life Caster", attributes: [40, 50, 10, 30, 100, 100] },
    Template { name: "War Mage", attributes: [50, 40, 10, 30, 100, 100] },
    Template { name: "Wayfarer", attributes: [30, 30, 100, 100, 60, 10] },
    Template { name: "Soldier", attributes: [100, 60, 100, 50, 10, 10] },
];
static TPLS_8: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
    Template { name: "Bow Hunter", attributes: [40, 30, 100, 100, 50, 10] },
    Template { name: "Swashbuckler", attributes: [100, 40, 100, 50, 20, 20] },
    Template { name: "Life Caster", attributes: [40, 50, 10, 30, 100, 100] },
    Template { name: "War Mage", attributes: [50, 40, 10, 30, 100, 100] },
    Template { name: "Wayfarer", attributes: [30, 30, 100, 100, 60, 10] },
    Template { name: "Soldier", attributes: [100, 60, 100, 50, 10, 10] },
];
static TPLS_9: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
    Template { name: "Bow Hunter", attributes: [40, 30, 100, 100, 50, 10] },
    Template { name: "Swashbuckler", attributes: [100, 40, 100, 50, 20, 20] },
    Template { name: "Life Caster", attributes: [40, 50, 10, 30, 100, 100] },
    Template { name: "War Mage", attributes: [50, 40, 10, 30, 100, 100] },
    Template { name: "Wayfarer", attributes: [30, 30, 100, 100, 60, 10] },
    Template { name: "Soldier", attributes: [100, 60, 100, 50, 10, 10] },
];
static TPLS_10: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
    Template { name: "Bow Hunter", attributes: [40, 30, 100, 100, 50, 10] },
    Template { name: "Swashbuckler", attributes: [100, 40, 100, 50, 20, 20] },
    Template { name: "Life Caster", attributes: [40, 50, 10, 30, 100, 100] },
    Template { name: "War Mage", attributes: [50, 40, 10, 30, 100, 100] },
    Template { name: "Wayfarer", attributes: [30, 30, 100, 100, 60, 10] },
    Template { name: "Soldier", attributes: [100, 60, 100, 50, 10, 10] },
];
static TPLS_11: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
    Template { name: "Bow Hunter", attributes: [40, 30, 100, 100, 50, 10] },
    Template { name: "Swashbuckler", attributes: [100, 40, 100, 50, 20, 20] },
    Template { name: "Life Caster", attributes: [40, 50, 10, 30, 100, 100] },
    Template { name: "War Mage", attributes: [50, 40, 10, 30, 100, 100] },
    Template { name: "Wayfarer", attributes: [30, 30, 100, 100, 60, 10] },
    Template { name: "Soldier", attributes: [100, 60, 100, 50, 10, 10] },
];
static TPLS_12: &[Template] = &[
    Template { name: "Ripper", attributes: [10, 10, 10, 10, 10, 10] },
];
static TPLS_13: &[Template] = &[
    Template { name: "Adventurer", attributes: [10, 10, 10, 10, 10, 10] },
];

pub static HERITAGES: &[Heritage] = &[
    Heritage { id: 1, name: "Aluvian", attribute_credits: 330, skill_credits: 52, start_areas: &[0, 1, 2, 3], templates: TPLS_1 },
    Heritage { id: 2, name: "Gharu'ndim", attribute_credits: 330, skill_credits: 52, start_areas: &[2, 0, 1, 3], templates: TPLS_2 },
    Heritage { id: 3, name: "Sho", attribute_credits: 330, skill_credits: 52, start_areas: &[1, 0, 2, 3], templates: TPLS_3 },
    Heritage { id: 4, name: "Viamontian", attribute_credits: 330, skill_credits: 52, start_areas: &[3, 0, 1, 2], templates: TPLS_4 },
    Heritage { id: 5, name: "Umbraen", attribute_credits: 330, skill_credits: 52, start_areas: &[0, 1, 2, 3], templates: TPLS_5 },
    Heritage { id: 6, name: "Gear", attribute_credits: 330, skill_credits: 52, start_areas: &[2, 1, 0, 3], templates: TPLS_6 },
    Heritage { id: 7, name: "Tumerok", attribute_credits: 330, skill_credits: 52, start_areas: &[0, 1, 2, 3], templates: TPLS_7 },
    Heritage { id: 8, name: "Lugian", attribute_credits: 330, skill_credits: 52, start_areas: &[0, 1, 2, 3], templates: TPLS_8 },
    Heritage { id: 9, name: "Empyrean", attribute_credits: 330, skill_credits: 52, start_areas: &[0, 1, 2, 3], templates: TPLS_9 },
    Heritage { id: 10, name: "Penumbraen", attribute_credits: 330, skill_credits: 52, start_areas: &[0, 1, 2, 3], templates: TPLS_10 },
    Heritage { id: 11, name: "Undead", attribute_credits: 330, skill_credits: 52, start_areas: &[0, 1, 2, 3], templates: TPLS_11 },
    Heritage { id: 12, name: "Olthoi", attribute_credits: 60, skill_credits: 68, start_areas: &[4], templates: TPLS_12 },
    Heritage { id: 13, name: "OlthoiAcid", attribute_credits: 330, skill_credits: 52, start_areas: &[4], templates: TPLS_13 },
];

/// Look up a heritage by its wire id.
pub fn heritage_by_id(id: u32) -> Option<&'static Heritage> {
    HERITAGES.iter().find(|h| h.id == id)
}

/// Look up a heritage by (case-insensitive) name.
pub fn heritage_by_name(name: &str) -> Option<&'static Heritage> {
    HERITAGES.iter().find(|h| h.name.eq_ignore_ascii_case(name))
}

/// Starter-area name for an index, if valid.
pub fn starter_area_name(idx: u32) -> Option<&'static str> {
    STARTER_AREAS.get(idx as usize).map(|a| a.name)
}
