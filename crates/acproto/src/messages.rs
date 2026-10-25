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
    /// `EmoteText` (0x01E0) / `SoulEmote` (0x01E2): broadcast emote /
    /// soul-emote chat. Body: `u32 senderId, String16L senderName,
    /// String16L emoteText` (identical layout for both).
    pub const EMOTE_TEXT: u32 = 0x01E0;
    pub const SOUL_EMOTE_MSG: u32 = 0x01E2;
    pub const PLAYER_KILLED: u32 = 0x019E;
    pub const PRIVATE_UPDATE_ATTRIBUTE_2ND_LEVEL: u32 = 0x02E9;
    /// `PrivateUpdateVital` (0x02E7): full vital state for self (ranks/
    /// starting/xp/current). The server may send either this or 0x02E9.
    pub const PRIVATE_UPDATE_VITAL: u32 = 0x02E7;
    /// `PrivateUpdatePropertyInt` (0x02CD): a self PropertyInt update
    /// (e.g. CoinValue after a buy/sell, CombatMode echoes).
    pub const PRIVATE_UPDATE_PROPERTY_INT: u32 = 0x02CD;
    /// `HearRangedSpeech` (0x02BC): out-of-earshot speech with a range.
    pub const HEAR_RANGED_SPEECH: u32 = 0x02BC;
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
    pub const PUT_ITEM_IN_CONTAINER: u32 = 0x0019;
    pub const GET_AND_WIELD_ITEM: u32 = 0x001A;
    pub const DROP_ITEM: u32 = 0x001B;
    pub const USE_WITH_TARGET: u32 = 0x0035;
    pub const USE: u32 = 0x0036;
    pub const BUY: u32 = 0x005F;
    pub const SELL: u32 = 0x0060;
    pub const OPEN_TRADE_NEGOTIATIONS: u32 = 0x01F6;
    pub const CLOSE_TRADE_NEGOTIATIONS: u32 = 0x01F7;
    pub const ADD_TO_TRADE: u32 = 0x01F8;
    pub const ACCEPT_TRADE: u32 = 0x01FA;
    pub const DECLINE_TRADE: u32 = 0x01FB;
    pub const RESET_TRADE: u32 = 0x0204;
    pub const NO_LONGER_VIEWING_CONTENTS: u32 = 0x0195;
    pub const FELLOWSHIP_UPDATE_REQUEST: u32 = 0x00A6;
    pub const ALLEGIANCE_UPDATE_REQUEST: u32 = 0x001F;
    pub const HOUSE_QUERY: u32 = 0x021E;
    pub const TARGETED_MISSILE_ATTACK: u32 = 0x000A;
    pub const ADD_FRIEND: u32 = 0x0018;
    pub const REMOVE_FRIEND: u32 = 0x0017;
    pub const SWEAR_ALLEGIANCE: u32 = 0x001D;
    pub const BREAK_ALLEGIANCE: u32 = 0x001E;
    pub const CAST_UNTARGETED_SPELL: u32 = 0x0048;
    pub const CAST_TARGETED_SPELL: u32 = 0x004A;
    pub const FELLOWSHIP_CREATE: u32 = 0x00A2;
    pub const FELLOWSHIP_QUIT: u32 = 0x00A3;
    pub const FELLOWSHIP_DISMISS: u32 = 0x00A4;
    pub const FELLOWSHIP_RECRUIT: u32 = 0x00A5;
    pub const BOOK_DATA: u32 = 0x00AA;
    pub const BOOK_MODIFY_PAGE: u32 = 0x00AB;
    pub const BOOK_ADD_PAGE: u32 = 0x00AC;
    pub const BOOK_DELETE_PAGE: u32 = 0x00AD;
    pub const BOOK_PAGE_DATA: u32 = 0x00AE;
    pub const REMOVE_SPELL: u32 = 0x01A8;
    pub const EMOTE: u32 = 0x01DF;
    pub const SOUL_EMOTE: u32 = 0x01E1;
    pub const ADD_SPELL_FAVORITE: u32 = 0x01E3;
    pub const REMOVE_SPELL_FAVORITE: u32 = 0x01E4;
    pub const FELLOWSHIP_ASSIGN_NEW_LEADER: u32 = 0x0290;
    pub const FELLOWSHIP_CHANGE_OPENNESS: u32 = 0x0291;
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
    pub const POPUP_STRING: u32 = 0x0004;
    pub const FRIENDS_LIST_UPDATE: u32 = 0x0021;
    pub const INVENTORY_PUT_OBJ_IN_CONTAINER: u32 = 0x0022;
    pub const WIELD_OBJECT: u32 = 0x0023;
    pub const CHARACTER_TITLE: u32 = 0x0029;
    pub const INVENTORY_PUT_OBJECT_IN_3D: u32 = 0x019A;
    pub const INVENTORY_SERVER_SAVE_FAILED: u32 = 0x00A0;
    pub const APPROACH_VENDOR: u32 = 0x0062;
    pub const WEENIE_ERROR: u32 = 0x028A;
    pub const WEENIE_ERROR_WITH_STRING: u32 = 0x028B;
    pub const REGISTER_TRADE: u32 = 0x01FD;
    pub const CLOSE_TRADE: u32 = 0x01FF;
    pub const ADD_TO_TRADE: u32 = 0x0200;
    pub const REMOVE_FROM_TRADE: u32 = 0x0201;
    pub const ACCEPT_TRADE: u32 = 0x0202;
    pub const DECLINE_TRADE: u32 = 0x0203;
    pub const RESET_TRADE: u32 = 0x0205;
    pub const TRADE_FAILURE: u32 = 0x0207;
    pub const CLEAR_TRADE_ACCEPTANCE: u32 = 0x0208;
    pub const COMMUNICATION_TRANSIENT_STRING: u32 = 0x02EB;
    pub const MAGIC_UPDATE_ENCHANTMENT: u32 = 0x02C2;
    pub const MAGIC_REMOVE_ENCHANTMENT: u32 = 0x02C3;
    pub const UPDATE_TITLE: u32 = 0x002B;
    pub const SET_SQUELCH_DB: u32 = 0x01F4;
    pub const SEND_CLIENT_CONTRACT_TRACKER_TABLE: u32 = 0x0314;
    pub const SEND_CLIENT_CONTRACT_TRACKER: u32 = 0x0315;
    pub const ALLEGIANCE_UPDATE: u32 = 0x0020;
    pub const FELLOWSHIP_FULL_UPDATE: u32 = 0x02BE;
    pub const HOUSE_DATA: u32 = 0x0225;
    pub const HOUSE_STATUS: u32 = 0x0226;
    pub const CLOSE_GROUND_CONTAINER: u32 = 0x0052;
    pub const VIEW_CONTENTS: u32 = 0x0196;
    pub const USE_DONE: u32 = 0x01C7;
    pub const ATTACK_DONE: u32 = 0x01A7;
    pub const BOOK_DATA_RESPONSE: u32 = 0x00B4;
    pub const BOOK_MODIFY_PAGE_RESPONSE: u32 = 0x00B5;
    pub const BOOK_ADD_PAGE_RESPONSE: u32 = 0x00B6;
    pub const BOOK_DELETE_PAGE_RESPONSE: u32 = 0x00B7;
    pub const BOOK_PAGE_DATA_RESPONSE: u32 = 0x00B8;
    pub const MAGIC_REMOVE_SPELL: u32 = 0x01A8;
    pub const MAGIC_UPDATE_SPELL: u32 = 0x02C1;
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

/// `Vital` ids as written in `PRIVATE_UPDATE_ATTRIBUTE_2ND_LEVEL` (0x02E9).
pub mod vital {
    pub const HEALTH: u32 = 2;
    pub const STAMINA: u32 = 4;
    pub const MANA: u32 = 6;
}

/// Retail skill ID -> name (`ACE.Entity.Enum.Skill`, IDs 0..=55).
pub const SKILL_NAMES: &[&str] = &[
    "None", "Axe", "Bow", "Crossbow", "Dagger", "Mace", "Melee Defense",
    "Missile Defense", "Sling", "Spear", "Staff", "Sword", "Thrown Weapon",
    "Unarmed Combat", "Arcane Lore", "Magic Defense", "Mana Conversion",
    "Spellcraft", "Item Tinkering", "Assess Person", "Deception", "Healing",
    "Jump", "Lockpick", "Run", "Awareness", "Arms & Armor Repair",
    "Assess Creature", "Weapon Tinkering", "Armor Tinkering",
    "Magic Item Tinkering", "Creature Enchantment", "Item Enchantment",
    "Life Magic", "War Magic", "Leadership", "Loyalty", "Fletching",
    "Alchemy", "Cooking", "Salvaging", "Two-Handed Combat", "Gearcraft",
    "Void Magic", "Heavy Weapons", "Light Weapons", "Finesse Weapons",
    "Missile Weapons", "Shield", "Dual Wield", "Recklessness", "Sneak Attack",
    "Dirty Fighting", "Challenge", "Summoning",
];

/// `WeenieError` codes the client surfaces.
pub mod weenie_error {
    pub const TRADE_MAX_DISTANCE_EXCEEDED: u32 = 0x044E;
    pub const TRADE_ALREADY_TRADING: u32 = 0x044F;
    pub const TRADE_COMPLETE: u32 = 0x0529;
    pub const LOCK_ALREADY_UNLOCKED: u32 = 0x04AF;
    pub const YOU_CANNOT_LOCK_OR_UNLOCK_THAT: u32 = 0x04B0;
    pub const YOU_CANNOT_LOCK_WHAT_IS_OPEN: u32 = 0x04B1;
    pub const KEY_DOESNT_FIT_THIS_LOCK: u32 = 0x04B2;
    pub const LOCK_USED_TOO_RECENTLY: u32 = 0x04B3;
    pub const YOU_ARENT_TRAINED_IN_LOCKPICKING: u32 = 0x04B4;
    pub const ATTUNED_ITEM: u32 = 0x0426;

    /// Retail-style display text for the codes above (the wire carries only
    /// the numeric code).
    pub fn name(code: u32) -> Option<&'static str> {
        Some(match code {
            TRADE_MAX_DISTANCE_EXCEEDED => "That trade partner is too far away.",
            TRADE_ALREADY_TRADING => "You are already trading with another character.",
            TRADE_COMPLETE => "The trade is complete.",
            LOCK_ALREADY_UNLOCKED => "The lock is already unlocked.",
            YOU_CANNOT_LOCK_OR_UNLOCK_THAT => "You can't lock or unlock that!",
            YOU_CANNOT_LOCK_WHAT_IS_OPEN => "You can't lock or unlock what is open!",
            KEY_DOESNT_FIT_THIS_LOCK => "The key doesn't fit this lock.",
            LOCK_USED_TOO_RECENTLY => "The lock has been used too recently.",
            YOU_ARENT_TRAINED_IN_LOCKPICKING => "You aren't trained in lockpicking!",
            ATTUNED_ITEM => "You can't drop attuned items.",
            _ => return None,
        })
    }
}

/// `UseDone` (0x01C7) WeenieError codes the server does NOT also send as a
/// transient chat string (the key/lockpick results): code -> text to show.
/// (retail's lock error strings)
pub const USE_DONE_TEXT: &[(u32, &str)] = &[
    (weenie_error::LOCK_ALREADY_UNLOCKED, "The lock is already unlocked."),
    (weenie_error::YOU_CANNOT_LOCK_OR_UNLOCK_THAT, "You can't lock or unlock that!"),
    (weenie_error::YOU_CANNOT_LOCK_WHAT_IS_OPEN, "You can't lock or unlock what is open!"),
    (weenie_error::KEY_DOESNT_FIT_THIS_LOCK, "The key doesn't fit this lock."),
    (weenie_error::LOCK_USED_TOO_RECENTLY, "The lock has been used too recently."),
    (weenie_error::YOU_ARENT_TRAINED_IN_LOCKPICKING, "You aren't trained in lockpicking!"),
];

/// Retail character title ids (`ACE.Entity.Enum.CharacterTitle`, 0-based
/// sequential; ids are enum values).
pub const TITLE_NAMES: &[&str] = &[
    "Invalid", "Adventurer", "Archer", "Blademaster", "Enchanter", "LifeMage", "Sorcerer", "Vagabond",
    "Warrior", "BowHunter", "LifeCaster", "Soldier", "Swashbuckler", "WarMage", "Wayfarer", "AbhorrentWarrior",
    "Alchemist", "Annihilator", "Apothecary", "ArcticAdventurer", "ArcticMattekarAnnihilator", "Artifex", "AxeWarrior", "Ballisteer",
    "BaneoftheRemoran", "BloodShrethButcher", "Bloodletter", "BrooduKiller", "BrowerkKiller", "BugButcher", "Bugstomper", "BunnyMaster",
    "CarenziSlayer", "ChainBreaker", "ChampionofDereth", "ChampionofSanamar", "ChampionofSilyun", "Chef", "CreatureAdept", "DaggerFighter",
    "Deadeye", "DeathcapDefeater", "DefenderofDereth", "Deliverer", "Diplomat", "DireMattekarDispatcher", "Duelist", "EbonGromnieEradicator",
    "EngorgedScourge", "Evoker", "Explorer", "Exterminator", "Fisherman", "Fletcher", "FloesharkFlogger", "FriendofSanamar",
    "FriendofSilyun", "GaerlanSlayer", "GardenerWeeder", "GlendenWoodKnight", "GlendenWoodMilitia", "GoldenGod", "GraveRobber", "GuardianofDereth",
    "Gumshoe", "GuppyMaster", "HeartRipper", "HeroofDereth", "HeroofSanamar", "HeroofSilyun", "HonorarySnowman", "HonoraryWindreave",
    "Hunter", "Impaler", "InsatiableSlayer", "InvadersBane", "IronChef", "IronSpinedChittickImmolator", "ItemAdept", "KeeperofDereth",
    "KeerikKiller", "Kingslayer", "KireeKiller", "KnathAndrasAssassinator", "Lakeman", "LifeAdept", "Lightbringer", "MaceWarrior",
    "ManAtArmsThrasher", "MasterFletcher", "MasterofSlaughter", "MasterofStaves", "Miner", "MoarsmanDefiler", "MoraleSmasher", "MosswartWorshipperWhipper",
    "MottledCarenziMauler", "MudSlinger", "NaughtySkeletonSnuffer", "OlthoiRipperReducer", "ParagonofDeath", "Philanthropist", "PlateArmoredilloPunisher", "PlatinumProwler",
    "PolarUrsuinPounder", "PolardilloPummeler", "PondScum", "ProFisherman", "Projectilist", "Pugilist", "Queenslayer", "RavenousKiller",
    "RedFury", "ReeshanKiller", "RehirKiller", "RepugnantEaterRipper", "ResistanceFighter", "ResistanceRecruit", "ServantofTheDeep", "SezzhereiSlayer",
    "ShadowStalker", "ShallowsSharkNemesis", "Sharpshooter", "ShellfishHater", "ShieldofGlenden", "Shredder", "SilverSerf", "SiraluunSlasher",
    "Skipper", "Skullcrusher", "Skullsplitter", "Slicer", "Sniper", "SoldierSlaughterer", "SpearWarrior", "SpringCleaner",
    "StaffWarrior", "Stonebreaker", "Sureshot", "Swordfighter", "Theurgist", "ThrungusReaper", "Timberman", "Tracker",
    "Trapper", "TukoraLieutenantTrouncer", "UnarmedBrawler", "ViolatorGrievverVetoer", "VoraciousFlayer", "VoraciousHunter", "WarAdept", "WardenofDereth",
    "Warlock", "WarlordofDereth", "WarriorVanquisher", "WickedSkeletonWalloper", "WindreaveStalker", "WorkerObliterator", "DryreachMilitia", "HonoraryShadowHunter",
    "PerforatedKnight", "UNKNOWNGUESSEDNAME161", "PestControl", "RatReaper", "NymphManiac", "SimiuS", "SecondPlaceLoreQuizNight", "ACVeteran",
    "Aintafraidofnoghost", "Aintafraidofnoghosttwo", "Aintafraidofnoghostthree", "AnkleBiter", "AnnoyingFurryTalkingAnimal", "ArcherAssassin", "Archmage", "AsheronsCallGod",
    "AvatarofKain", "AzureAssailant", "BabyBunnyMaster", "BaronOddity", "BeginningoftheEnd", "BestMeleeinDT", "BigBlock", "BigRocx",
    "Blood", "BountyHunter", "BritishTootsie", "BugBait", "BunnyKiller", "CarebearStare", "CertifiedGanksta", "ChickenNugget",
    "ChickenSelect", "ChimaerasChampion", "CoMonarchofTheChosen", "ContainmentUnit", "CoolestTitleEver", "CorrosiveSoul", "CowKiller", "Cowboy",
    "CrystalLordKing", "CuddlyKitten", "CurmudgeonsFriend", "CursedAdventurer", "DaggerMaster", "DarkSwordLady", "DarktidesFinest", "DarktidesMostWanted",
    "DeadMeat", "DeadlyEggburt", "DeathbyDefault", "DeathbyPumpkin", "DeathbySappho", "DeathSpecialist", "DefenderofMrP", "DerethsGladiator",
    "DerethsMasterDodger", "DevEvader", "DevKiller", "DevSlayer", "DoubleFashionKing", "DoubleGolemKiller", "DoubleSurvivalChampion", "DudeofDOOM",
    "EctoCooler", "EctoCoolertwo", "EnvoySlayer", "EscapeeKiller", "EvilToothfairy", "FancyTitle", "FashionKing", "FashionPenguinKing",
    "FashionQueen", "FashionVictim", "FearlessFighter", "FlagBoy", "FrelornSlayer", "FriendtoMrP", "GangstaStyle", "Gangster",
    "Ghostbuster", "Gimp", "GimpGoddess", "GimpyMageofMight", "GladiatorChampion", "GlowingArcher", "GodoftheSwordsman", "GolemKiller",
    "GolemSlayer", "GreatPookieSlayer", "GuardianAngel", "GuardianofArwic", "HamsterHerder", "HamsterVitae", "HaveBunniesWillTravel", "HaveTrikeWillBabble",
    "HideandSeekSpecialist", "HollywoodBud", "HoltburgSurvivalChampion", "HoltburgsSurvivorChampion", "HonoraryPinkEep", "HungryHungryHippo", "HusbandofLynnie", "HusbandsHealer",
    "IamBigPIMPIN", "Ilikeddying", "JessicaHeart", "KingofallthatisPumpkin", "KingofCaul", "KingofDarktide", "KingofPierceDiggys", "KnightRider",
    "LadyoftheFourTowers", "LangeGang", "LevelMaster", "Leviathan", "LeviathanSlayer", "LibeEventsManager", "LifePreserver", "LiveEventCoordinator",
    "LiveEventsManager", "LiveOpCoordinator", "LoreSecondPlace", "LoreSecondPlaceQuizNight", "LoreChampionQuizNight1", "LoreChampionQuizNight2", "LoreMaster", "LoreMasterQuizNight1",
    "LoreMasterQuizNight2", "LoreMasterSecondPlace", "LoreMasterofStaves", "LotsofVitae", "LuckyLady", "MadCowSlayer", "MagesAssitant", "MasterTinkerer",
    "MayorofAyanBaqur", "MintyFresh", "MisunderstoodPengy", "Mom", "MommyPengy", "MosswartAlly", "MrNiceGuy", "Murderer",
    "Murderertwo", "NewbyTitle", "NiceBlueLady", "Numberonedraftpick", "OldSchool", "ParadoxSlayer", "PatronSaintoftheLifestone", "Peacebear",
    "PenguinKiller", "PigFarmer", "PinkBunnyKiller", "PinkBunnyMaster", "PinkGoddess", "PinkLady", "PKQueen", "PlayaKillaKing",
    "PokemonSlayer", "PrettyPumpkin", "Prisoner", "ProtectoroftheCheese", "ProtonPack", "PumpkinSlayer", "PumpkinStalker", "PurplePengy",
    "PwnerofNewbs", "QueenofBrats", "QueensCrafter", "QueensCraftertwo", "QueensPackRat", "QueensPackrat", "QueensTinker", "Rawr",
    "RaynesMagePet", "ReggaeRenegade", "RingBearer", "RingringringBananaphone", "RoadKill", "RollingDeathDestroyer", "RoyalChef", "RoyalCook",
    "Rugged", "SanitationEngineer", "ScarecrowSlayer", "ScaryPumkpkinDominator", "ScaryPumpkinSlayer", "Sentinal", "Sentry", "ShadowChild",
    "ShadowHunter", "ShadowSlayer", "SlayeroftheGreatPookie", "Slimer", "SnazzyDresser", "SoulMan", "SpookyTitle", "StaffMasta",
    "StaypuffedMallowGirl", "SuperGimpedAdventurer", "Supercalifragilisticexpialidocious", "Superman", "SurvivorChampion", "SwordScholar", "TehNewb", "TehSaint",
    "TestingGuy", "TheBait", "TheChosenKing", "TheComeBackKid", "TheForgotten", "TheGimped", "TheGreatandPowerful", "TheHalfFox",
    "TheNoob", "ThePurpleFez", "ThePwnerer", "TheUnknownPk", "ThunderChickenSlayer", "TMKiller", "ToughGuy", "TradesmanofAuberean",
    "TreasureHunter", "TremendousMonougaMaster", "TripleGolemKiller", "TurbineSlayer", "TwinsofTerror", "UberestNoobonFrostfell", "UnderworldThugg", "ViamontianSlayer",
    "VitaeKing", "VitaeVixen", "VixenofVitae", "WabbitBait", "WabbitKiller", "WanderingFool", "WardensAssistant", "WardensThug",
    "Wimp", "WolfpackCrafter", "WonderWoman", "Yagotserved", "GIMPcaps", "WardensAssistanttwo", "SupremeBunnyMaster", "GuardianoftheDark",
    "AprilsFool", "UlgrimsEep", "MukkirMasher", "HarbingersBane", "MasteroftheElements", "KillcranesAlly", "ApprenticeShadowHunter", "SquireoftheGoldenFlame",
    "KnightoftheGoldenFlame", "KnightoftheWhisperingBlade", "TheBlackSpear", "UNKNOWNGUESSEDNAME419", "CarraidasAvenger", "ParagonoftheWhisperingBlade", "ParagonoftheRossuMorta", "ChosenofVaricci",
    "ChosenofElysa", "SmallMukkirSquasher", "BloodWarrior", "Strategist", "WarBeast", "ChampionofDaemal", "HerooftheGoldenFlame", "SlayeroftheBlackSpear",
    "UNKNOWNGUESSEDNAME432", "ObviouslyBored", "ZefirZapper", "TuskerBlight", "ElementalEradicator", "ExpertBanishedHunter", "ExpertFallenHunter", "EliteHeadHunter",
    "FriendoftheFrostglaive", "SquireofNewViamont", "Reanimator", "ZombieIncursionSurvivor", "UlgrimsDrinkingBuddy", "MasterofthePorcelainAltar", "MasteroftheMysticalMug", "ColosseumChampion",
    "Titan", "Colossus", "MasterGladiator", "Fearless", "LordoftheArena", "RingMaster", "PitFighter", "Myrmidon",
    "HostileCombatant", "Survivalist", "WarrioroftheSeventhCircle", "Gladiator", "Contender", "DrudgeDread", "ArenaCustodian", "ArenaRat",
    "Scrapper", "ChampionRingIII", "ChampionRingV", "ChampionRingVI", "ChampionRingVII", "ChampionRingXII", "MasterChampionRingX", "RuukAlly",
    "SeekerofAsheron", "SeekerofTorgluuk", "PortalRitualist", "RangerRuuk", "BasherSlasher", "GurukHunter", "DeadEyefortheColossi", "FiendishHuntsman",
    "MonsterKiller", "BoshBoshKibosh", "MushroomCollector", "FungalFarmer", "MushroomMan", "Fungi", "MushroomKing", "Thrungamandius",
    "InitiateoftheBlade", "TorgluuksLiberator", "BurunLiberator", "AsheronsSavior", "AsheronsDefender", "UNKNOWNGUESSEDNAME493", "FuzzyBunnySlayer", "MoarHunter",
    "SpiketoothSlayer", "TowerGuardianArchitect", "ArtisanoftheFlameandLight", "GuardianoftheTower", "CragstoneMilitia", "CragstoneKnight", "GuardianofCragstone", "KnightoftheWindmill",
    "HeroOfHouseMhoire", "LadyTairlasChampion", "RoyalInvestigator", "WangaFighter", "HoogansHero", "CragstoneFirefighter", "UNKNOWNGUESSEDNAME510", "StalkerStalker",
    "Ravenous", "AlteredHunter", "AugmentedHunter", "DrudgeDoom", "DefenderofKryst", "ParadoxtouchedQueenslayer", "AmeliasCaretaker", "ProdigalTuskerSlayer",
    "MudmouthsBane", "OolutangasBane", "ProdigalShadowSlayer", "ProdigalHarbingerSlayer", "BloodSeeker", "StormRider", "Meddler", "VileBetrayer",
    "Spy", "SlithisSlayer", "ChosenoftheRift", "RiftShifter", "RiftWalker", "ReefBuilder", "ReefBreaker", "CryptRat",
    "GraveStalker", "BoneBreaker", "CryptCreeper", "TombRaider", "FolloweroftheVine", "Harvester", "HarvesterHarvester", "HarvesterHarvesterHarvester",
    "PumpkinThroneUsurper", "DeathKnight", "NullTitle", "TrackerGuardian", "DojiroSangisSavior", "MasteroftheHunt", "InitiateoftheHunt", "ApprenticeoftheHunt",
    "DiscipleoftheHunt", "SeekeroftheHunt", "ChampionoftheHunt", "GuardianofLinvakTukal", "UnwittingParticipant", "FriendofRheaga", "SlayerofRheaga", "ProtectorofthePast",
    "WarriorofthePast", "Anthropologist", "GoldFarmer", "ThirdEyeBlinder", "DireDrudgeDecapitator", "RenegadeHunter", "BanderlingBully", "Xenophobic",
    "ShadowSunderer", "DireHuntsman", "GameWarden", "HonoraryReaoftheAun", "HopeofthePast", "BeaconofHope", "AerbaxsBane", "AerbaxSlayer",
    "JestersEmancipator", "JestersFool", "JestersAccomplice", "HonoraryBurunScout", "MarbleWrangler", "SecretKeeper", "JestersLittleHelper", "HardToKill",
    "Expendable", "NoviceoftheSea", "AcolyteoftheSea", "MinisteroftheSea", "DefenderoftheSea", "ChampionoftheSea", "MasteroftheSea", "DefeaterOfTheBlight",
    "ChampionoftheAerlintheNode", "ChampionoftheAmunNode", "ChampionoftheEsperNode", "ChampionoftheHalaetanNode", "ChampionoftheLinvakNode", "ChampionoftheObsidianNode", "UNKNOWNGUESSEDNAME598", "UlgrimsHappyHundredth",
    "PluckerofEyes", "WardenoftheBurningWood", "ToughTough", "Archaeologist", "HeroofWoe", "DefenderofSilyun", "MemorialChampion", "ThaneofColier",
    "GuardianoftheKeep", "WarrioroftheCraterLake", "UNKNOWNGUESSEDNAME610", "Arwician", "DesertWarrior", "MayoiProtector", "StoneCollector", "RockStar",
    "RockHound", "WarriorofWoe", "Haswaytoomuchtime", "ChampionImmemorial", "ColierMiner", "Legionnaire", "Volcanologist", "HilltopDefender",
    "ArwicNoble", "Zharalim", "Samurai", "GuardianinthePatriarchRaids", "WardeninthePatriarchRaids", "HeroofthePatriarchRaids", "MarkedbythePatriarchs", "UNKNOWNGUESSEDNAME631",
    "UNKNOWNGUESSEDNAME632", "UNKNOWNGUESSEDNAME633", "UNKNOWNGUESSEDNAME634", "UNKNOWNGUESSEDNAME635", "UNKNOWNGUESSEDNAME636", "CathedralsChampion", "AsheronsMilitia", "MinikRasBane",
    "KazykRisBane", "NivinizksBane", "MinikRasNemesis", "KazykRisNemesis", "NivinizksNemesis", "AubereansSentinel", "DerethsElite", "DefenderoftheDeru",
    "BaneoftheBlessed", "Negotiator", "BaneoftheBlight", "Gifted", "SeasonalSeeker", "AmateurExplorer", "ExperiencedExplorer", "AdeptExplorer",
    "MasterExplorer", "EliteExplorer", "MountainClimber", "Daredevil", "SkyDiver", "Spelunker", "PartyGoer", "CraterCrasher",
    "TacticalAid", "TrueTactician", "TacticalFantastical", "FireoftheTanada", "CrystallineAdventurer", "WispWhipper", "BurningSoul", "FrozenFighter",
    "Dedicated", "Assassin", "DeathDealer", "BridgeJumper", "SureStep", "PlayerSlayer", "BathedinBlood", "IAmDarktide",
    "DarknessintheLight", "BeginningsEnd", "NexusCrawler", "SelflessSoul", "TimelessAdventurer", "VirindiInformer", "NoCageCouldHoldMe", "AllforOne",
    "OneforAll", "JackOAllTrades", "KnightoftheNortheastTower", "KnightoftheNorthwestTower", "KnightoftheSoutheastTower", "KnightoftheSouthwestTower", "KnightoftheMhoireThrone", "HistorianoftheMhoireThrone",
    "ChampionofHouseMhoire", "SwordbearerofHouseMhoire", "ArchmageofHouseMhoire", "SeekerofCastleMhoire", "StewardofCastleMhoire", "GoldenGearCrafter", "AllyoftheGoldPrimus", "GearKnightAssassin",
    "GearKnightRecruiter", "GearKnightEmissary", "MenhirSeeker", "ManaFieldFinder", "AssistantsAssistant", "ArcanumAdventurer", "GearKnightDefender", "Ripper",
    "AcidSpitter", "BloodstoneHunter", "GuidingLight", "CloudedSoul", "UndercoverofDarkness", "BroughttoLight", "UNKNOWNGUESSEDNAME718", "IlluminatingtheShadows",
    "ExploringArchaeologist", "ContractKiller", "ShadowPuppet", "Hopebringer", "IntheDark", "UNKNOWNGUESSEDNAME725", "SearchingShadows", "ShadowSoldier",
    "BrightKnight", "MercifulKiller", "SoulSiphon", "TheTouTouTerror", "DoubleAgent", "DuleingwiththeDark", "CreatureofChaos", "Conspirator",
    "HerooftheNight", "MasteroftheTwistedWord", "TheNightmareLord", "TheNightmareMage", "TheTwistedWeaver", "LordofDarkDreams", "UNKNOWNGUESSEDNAME742", "TheDarkDreamwalker",
    "UNKNOWNGUESSEDNAME744", "TheDreamslayer", "TheNightmareStalker", "UNKNOWNGUESSEDNAME747", "TheThoughtSpiral", "UNKNOWNGUESSEDNAME749", "UNKNOWNGUESSEDNAME750", "MasteroftheOubliette",
    "UNKNOWNGUESSEDNAME751", "TheRestless", "FierySpirit", "IcyVeins", "ShockingDisposition", "AcidicSoul", "GroundedMorals", "DarkenedHeart",
    "TheTrueEmperor", "BearerofDarkness", "UNKNOWNGUESSEDNAME762", "HealerRitualist", "VanquisheroftheTitan", "DerethianNewbie", "NoviceWanderer", "NotableCitizen",
    "AdeptAdventurer", "IntrepidExplorer", "SupremeSoldier", "EpicWarrior", "ParagonofNewAluvia", "AnAubereanLegend", "Heretic", "Captain",
    "PetSavior", "AvalancheAvoider", "LiberatorofUziz", "OutofTune", "HiveQueenslayer", "EvisceratorDecimator", "UltimateWarrior", "StoneColdKiller",
    "QuellerofRage", "EnderofTorment", "ControllerofEmotions", "RynthidRavager", "LothusServitor", "AllyoftheLothus", "AllyoftheCouncil", "EmotionalWreck",
    "MinionMarauder", "RynthidWrecker", "NightOwl", "MenderoftheRift", "SorcererSlayer", "RageQuitter", "Purifier", "FanOfAsheron",
    "Asherons1", "Shutterbug", "TheFocused", "DerethRecruiter", "FowlFollower", "FollowerofAsheron", "FollowsDirections", "CantFollowDirections",
    "LikesAsheron", "LikesGettingTitles", "LikesWhereThisIsGoing", "LikesUlgrim", "Likable", "Captivating", "MasterofCeremony", "RoadWarrior",
    "Thriller", "HauntsYourDreams", "AsheronsCollectCaller", "TheInquisitive", "PortalStormer", "BringerofPain", "HarbingerofMadness", "MarkedByLuck",
    "MarkedByFate", "MarkedByFame", "MarkedByGlamour", "MarkedByBacon", "Puzzlemaster", "RiddleRavager", "TheCoolKidsClub", "EpicWin",
    "EpicFail", "AlwaysPickedFirst", "AlwaysPickedLast", "ForeverAlone", "NinetyNineProblemsOlthoiAte1", "HungryforMoar", "ICanHazTitleNao", "Snowbunny",
    "ColdAsIce", "PenguinPlunge", "FlowerSniffer", "SpringsIntoAction", "HeatsThingsUp", "CruelSummer", "HotintheCity", "RunsThroughSprinklers",
    "TheKnowledgeable", "WellTrained", "PumpkinLord", "Haunted", "Possessed", "Doomsayer", "Accursed", "ColdTurkey",
    "JiveTurkey", "PresentProtector", "HollyJollyHelper", "LossPrevention", "ChristmasCourier", "HolidayHero", "CrownOfTheDeru", "ViridianDreamer",
    "ViridianLord", "ViridianKnight", "ViridianStalker", "ViridianSlayer", "ViridianPurifier", "KnightOfThorns", "HunterOfThorns", "MageOfThorns",
    "Thornstalker", "ChampionOfTheViridianTree", "CorruptorOfTheRoot", "KillerAmongShadows", "ServantOfTheVile", "KnightOfCorruptedAmber", "GauntletChampion", "Bloodthirsty",
    "GauntletGladiator", "WilyWarrior", "SoldierOfFortune", "SocietySavage", "CoinCollector", "BrutalBarbarian", "Idolized", "MasterOfPuppets",
    "TitanSlayer", "Awakened", "Enlightened", "Illuminated", "Transcended", "CosmicConscious", "LastManStanding",
];

/// `ItemLoadFlag` bits -> retail item category. Checked in this exact order
/// (first matching flag wins), matching the retail client's `itemTypeName`.
pub const ITEM_TYPES: &[(u32, &str)] = &[
    (0x1, "melee"), (0x100, "missile"), (0x2, "armor"), (0x4, "clothing"),
    (0x8, "jewelry"), (0x10, "creature"), (0x20, "food"), (0x40, "money"),
    (0x80, "misc"), (0x200, "container"), (0x400, "useless"), (0x800, "gem"),
    (0x1000, "spellcomp"), (0x2000, "writable"), (0x4000, "key"),
    (0x8000, "caster"), (0x10000, "portal"), (0x20000, "lockable"),
    (0x40000, "note"), (0x80000, "manastone"), (0x100000, "service"),
    (0x200000, "magic"), (0x400000, "cook"), (0x800000, "alchemy"),
    (0x2000000, "fletch"), (0x4000000, "alchemy+"), (0x8000000, "fletch+"),
    (0x10000000, "lifestone"), (0x20000000, "tool"), (0x40000000, "material"),
    (0x80000000, "gameboard"),
];

/// Retail item category name for an `ItemType` flag word (first matching
/// flag in `ITEM_TYPES` wins).
#[must_use]
pub fn item_type_name(item_type: u32) -> Option<&'static str> {
    ITEM_TYPES.iter().find(|(flag, _)| item_type & *flag != 0).map(|(_, n)| *n)
}

/// `TradeSide` ids in trade events (1 = self's side, 2 = partner's).
pub mod trade_side {
    pub const SELF: u32 = 1;
    pub const PARTNER: u32 = 2;
}

/// Motion command ids for door open/close (from MotionCommand u16 0x0B/0x0C).
pub const MOTION_CMD_DOOR_ON: u32 = 0x4000_000B;
pub const MOTION_CMD_DOOR_OFF: u32 = 0x4000_000C;

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

/// `TALK_DIRECT` (0x0032) body: `String16L text, u32 targetGuid` (message
/// first). A player target receives a Tell; a creature runs its
/// ReceiveTalkDirect emote set (the "talk to NPC" dialog).
#[must_use]
pub fn body_talk_direct(text: &str, target_guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.string16l(text);
    w.u32(target_guid);
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
/// `u32 target, u32 height (1 Low / 2 Medium / 3 High), f32 power (0..=1)`.
/// Retail defaults: height Medium (2), power 0.5.
#[must_use]
pub fn body_targeted_melee_attack(target: u32, height: u32, power: f32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(target);
    w.u32(height);
    w.f32(power);
    w.out
}

/// `TARGETED_MISSILE_ATTACK` (0x000A) body:
/// `u32 target, u32 height (1|2|3), f32 accuracy (0..=1)`.
#[must_use]
pub fn body_targeted_missile_attack(target: u32, height: u32, accuracy: f32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(target);
    w.u32(height);
    w.f32(accuracy);
    w.out
}

/// `PUT_ITEM_IN_CONTAINER` (0x0019) body:
/// `u32 itemGuid, u32 containerGuid, i32 placement`. The player's own guid
/// unequips to the main pack; a pack's guid moves it into that pack.
#[must_use]
pub fn body_put_item_in_container(item: u32, container: u32, placement: i32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(item);
    w.u32(container);
    w.i32(placement);
    w.out
}

/// `GET_AND_WIELD_ITEM` (0x001A) body: `u32 itemGuid, i32 location` (EquipMask).
#[must_use]
pub fn body_get_and_wield_item(item: u32, equip_mask: i32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(item);
    w.i32(equip_mask);
    w.out
}

/// `DROP_ITEM` (0x001B) body: `u32 itemGuid`. Drops to the ground at the
/// player's feet; the server answers with an ObjectDelete + a positioned
/// ObjectCreate.
#[must_use]
pub fn body_drop_item(item: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(item);
    w.out
}

/// `USE_WITH_TARGET` (0x0035) body: `u32 sourceGuid, u32 targetGuid`. The
/// source must be in own inventory/equipped; the target may be a landblock.
#[must_use]
pub fn body_use_with_target(src: u32, tgt: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(src);
    w.u32(tgt);
    w.out
}

/// `USE` (0x0036) body: `u32 targetGuid`. Use a world object (e.g. a vendor
/// statue): the server resolves it in the landblock and walks to it.
#[must_use]
pub fn body_use(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `BUY` (0x005F) body: `u32 vendorGuid, u32 numItems, per item
/// i32 amount + u32 objectId`. `objectId` is the vendor item's instance guid
/// (the guid sent in the ApproachVendor event).
#[must_use]
pub fn body_buy(vendor_guid: u32, items: &[(i32, u32)]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(vendor_guid);
    w.u32(items.len() as u32);
    for (amount, object_id) in items {
        w.i32(*amount);
        w.u32(*object_id);
    }
    w.out
}

/// `SELL` (0x0060) body: `u32 vendorGuid, u32 numItems, per item
/// i32 amount + u32 objectGuid` (the player inventory instance guid).
#[must_use]
pub fn body_sell(vendor_guid: u32, items: &[(i32, u32)]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(vendor_guid);
    w.u32(items.len() as u32);
    for (amount, object_guid) in items {
        w.i32(*amount);
        w.u32(*object_guid);
    }
    w.out
}

/// `OPEN_TRADE_NEGOTIATIONS` (0x01F6) body: `u32 partnerGuid`.
#[must_use]
pub fn body_open_trade(partner: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(partner);
    w.out
}

/// `CLOSE_TRADE_NEGOTIATIONS` (0x01F7) body: empty.
#[must_use]
pub fn body_close_trade() -> Vec<u8> {
    Vec::new()
}

/// `ADD_TO_TRADE` (0x01F8) body: `u32 itemGuid, u32 slot`.
#[must_use]
pub fn body_add_to_trade(item: u32, slot: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(item);
    w.u32(slot);
    w.out
}

/// `ACCEPT_TRADE` (0x01FA) body: 28 bytes — `i32 partnerGuid, f64 0.0,
/// u32 0, u32 partnerGuid, u32 1, u32 1`. The server reads all six fields
/// but only uses the partner guid.
#[must_use]
pub fn body_accept_trade(partner: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.i32(partner as i32);
    w.f64(0.0);
    w.u32(0);
    w.u32(partner);
    w.u32(1);
    w.u32(1);
    w.out
}

/// `DECLINE_TRADE` (0x01FB) body: empty.
#[must_use]
pub fn body_decline_trade() -> Vec<u8> {
    Vec::new()
}

/// `RESET_TRADE` (0x0204) body: empty.
#[must_use]
pub fn body_reset_trade() -> Vec<u8> {
    Vec::new()
}

/// `NO_LONGER_VIEWING_CONTENTS` (0x0195) body: `u32 containerGuid`. The
/// server closes the ground container (S2C CloseGroundContainer follows).
#[must_use]
pub fn body_no_longer_viewing(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `FELLOWSHIP_UPDATE_REQUEST` (0x00A6) body: `u32 panelOpen (1|0)`.
#[must_use]
pub fn body_fellowship_update_request(open: bool) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(u32::from(open));
    w.out
}

/// `ALLEGIANCE_UPDATE_REQUEST` (0x001F) body: `u32 panelOpen (1|0)`. The
/// server always replies (rank 0 + empty profile when not in one).
#[must_use]
pub fn body_allegiance_update_request(open: bool) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(u32::from(open));
    w.out
}

/// `HOUSE_QUERY` (0x021E) body: empty. The server replies with HouseData
/// (owner) or HouseStatus (u32 weenieError, non-zero = not the owner).
#[must_use]
pub fn body_house_query() -> Vec<u8> {
    Vec::new()
}

/// `CAST_UNTARGETED_SPELL` (0x0048) body: `u32 spellId`.
#[must_use]
pub fn body_cast_untargeted_spell(spell_id: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(spell_id);
    w.out
}

/// `CAST_TARGETED_SPELL` (0x004A) body: `u32 targetGuid, u32 spellId`.
#[must_use]
pub fn body_cast_targeted_spell(target: u32, spell_id: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(target);
    w.u32(spell_id);
    w.out
}

/// `REMOVE_SPELL` (0x01A8) body: `u32 spellId` (forgets one spell).
#[must_use]
pub fn body_remove_spell(spell_id: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(spell_id);
    w.out
}

/// `SWEAR_ALLEGIANCE` (0x001D) body: `u32 targetGuid` (the allegiance's
/// current leader).
#[must_use]
pub fn body_swear_allegiance(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `BREAK_ALLEGIANCE` (0x001E) body: `u32 targetGuid`.
#[must_use]
pub fn body_break_allegiance(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `ADD_FRIEND` (0x0018) body: `String16L name` (player name, not guid).
#[must_use]
pub fn body_add_friend(name: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.string16l(name);
    w.out
}

/// `REMOVE_FRIEND` (0x0017) body: `u32 guid`.
#[must_use]
pub fn body_remove_friend(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `FELLOWSHIP_CREATE` (0x00A2) body: `String16L name, u32 shareXP`.
#[must_use]
pub fn body_fellowship_create(name: &str, share_xp: bool) -> Vec<u8> {
    let mut w = Writer::new();
    w.string16l(name);
    w.u32(u32::from(share_xp));
    w.out
}

/// `FELLOWSHIP_QUIT` (0x00A3) body: `u32 disband` (1 = also disband).
#[must_use]
pub fn body_fellowship_quit(disband: bool) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(u32::from(disband));
    w.out
}

/// `FELLOWSHIP_DISMISS` (0x00A4) body: `u32 guid` (a member).
#[must_use]
pub fn body_fellowship_dismiss(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `FELLOWSHIP_RECRUIT` (0x00A5) body: `u32 guid` (a player).
#[must_use]
pub fn body_fellowship_recruit(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `FELLOWSHIP_ASSIGN_NEW_LEADER` (0x0290) body: `u32 guid`.
#[must_use]
pub fn body_fellowship_new_leader(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `FELLOWSHIP_CHANGE_OPENNESS` (0x0291) body: `u32 open` (1|0).
#[must_use]
pub fn body_fellowship_openness(open: bool) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(u32::from(open));
    w.out
}

/// `BOOK_DATA` (0x00AA) body: `u32 bookGuid`. Requests the full book.
#[must_use]
pub fn body_book_data(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `BOOK_MODIFY_PAGE` (0x00AB) body: `u32 bookGuid, i32 page, String16L text`.
#[must_use]
pub fn body_book_modify_page(guid: u32, page: i32, text: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.i32(page);
    w.string16l(text);
    w.out
}

/// `BOOK_ADD_PAGE` (0x00AC) body: `u32 bookGuid`.
#[must_use]
pub fn body_book_add_page(guid: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.out
}

/// `BOOK_DELETE_PAGE` (0x00AD) body: `u32 bookGuid, i32 page`.
#[must_use]
pub fn body_book_delete_page(guid: u32, page: i32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.i32(page);
    w.out
}

/// `BOOK_PAGE_DATA` (0x00AE) body: `u32 bookGuid, i32 page` (0-based).
#[must_use]
pub fn body_book_page_data(guid: u32, page: i32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(guid);
    w.i32(page);
    w.out
}

/// `ADD_SPELL_FAVORITE` (0x01E3) body:
/// `u32 spellId, u32 position (tab slot), u32 tab`.
#[must_use]
pub fn body_add_spell_favorite(spell_id: u32, position: u32, tab: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(spell_id);
    w.u32(position);
    w.u32(tab);
    w.out
}

/// `REMOVE_SPELL_FAVORITE` (0x01E4) body: `u32 spellId, u32 tab`.
#[must_use]
pub fn body_remove_spell_favorite(spell_id: u32, tab: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(spell_id);
    w.u32(tab);
    w.out
}

/// `EMOTE` (0x01DF) body: `String16L text` (the `/emote` chat path).
#[must_use]
pub fn body_emote(text: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.string16l(text);
    w.out
}

/// `SOUL_EMOTE` (0x01E1) body: `String16L text` (the `/pose` chat path).
#[must_use]
pub fn body_soul_emote(text: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.string16l(text);
    w.out
}

/// `PING_REQUEST` (0x01E9) body: empty.
#[must_use]
pub fn body_ping_request() -> Vec<u8> {
    Vec::new()
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
    /// `ItemType` flag word (0 if the tail was truncated before it).
    pub item_type: u32,
    /// `PropertyInt` Value (0x8 WeenieHeader flag).
    pub value: u32,
    /// `StackSize` (0x1000 flag).
    pub stack_size: u32,
    /// `ItemsCapacity` (0x2 flag).
    pub items_capacity: u32,
    /// `ContainerId` (0x4000 flag).
    pub container_id: u32,
    /// `WielderId` (0x8000 flag).
    pub wielder_id: u32,
    /// `ValidLocations` (0x10000 flag), as an EquipMask.
    pub valid_locs: i32,
    /// `CurrentlyWieldedLocation` (0x20000 flag), as an EquipMask.
    pub cur_loc: i32,
    /// ModelData MotionTable id (PhysicsData flag 0x2).
    pub mtable_id: Option<u32>,
    /// ModelData Setup id (PhysicsData flag 0x1).
    pub setup_id: Option<u32>,
    /// ModelData model parts: (partIndex, modelId).
    pub model_parts: Option<Vec<(usize, u32)>>,
    /// `ObjectDescriptionFlags` bit 0x00000200 (the object is a vendor).
    pub is_vendor: bool,
    /// The movement section's `forward` MotionCommand (door open/close
    /// tracking: `MOTION_CMD_DOOR_ON` / `MOTION_CMD_DOOR_OFF`).
    pub motion_cmd: u32,
    /// PhysicsData state word (bit 0x4 = Ethereal: door open).
    pub physics_state: u32,
    /// `PropertyFloat` UseRadius (0x20 weenie flag): the distance at which
    /// the object can be used — the approach stop distance and the
    /// vendor/loot window auto-close radius. `None` if absent.
    pub use_radius: Option<f32>,
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
    /// `PropertyInt` 20 (CoinValue), from the int table.
    pub coin_value: i64,
    /// Strength attribute (base + ranks).
    pub strength: i32,
    /// `PropertyInt` 5 (Encumbrance), from the int table.
    pub encumbrance: i32,
    /// (skill id, rank) from the Skill table.
    pub skills: Vec<(u32, i32)>,
    /// spell ids from the Spell table.
    pub spells: Vec<u32>,
    /// The 8 spell-bar tabs (spell ids per tab), from SpellLists8.
    pub spell_bar: Vec<Vec<u32>>,
    /// The spellbookFilters bitmask (one u32, always present).
    pub spell_filters: Vec<u32>,
    /// Enchantment registry entries (the Vitae entry is spell 666).
    pub enchantments: Vec<Enchantment>,
    /// (item guid, containerType) from the inventory list.
    pub inventory: Vec<(u32, u32)>,
    /// (item guid, wieldLocation, clothingPriority) from the equipped list.
    pub equipped: Vec<(u32, u32, u32)>,
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
    /// `PropertyInt` 25 (level). None for non-creatures.
    pub level: Option<i32>,
    /// `PropertyInt` 19 (Value).
    pub value: Option<i32>,
    /// `PropertyInt` 5 (Encumbrance).
    pub encumbrance: Option<i32>,
    /// `PropertyInt` 107 (ItemMana) / 108 (ItemManaMax).
    pub item_mana: Option<i32>,
    pub item_mana_max: Option<i32>,
    /// `PropertyInt` 33 present (Bonded).
    pub bonded: bool,
    /// `PropertyInt` 114 present (Attuned).
    pub attuned: bool,
    /// First non-empty description string (keys 16, 15, 14, 5).
    pub description: String,
    /// CreatureProfile (flag 0x100).
    pub creature: Option<AppraiseCreature>,
    /// WeaponProfile (flag 0x20).
    pub weapon: Option<AppraiseWeapon>,
    /// ArmorProfile (flag 0x80).
    pub armor: Option<AppraiseArmor>,
    /// ArmorLevels (flag 0x4000): Head, Chest, Forearms, Hands, Legs,
    /// Feet, Shield, Cloak, TwoHanded (9 u32 values).
    pub armor_levels: Option<[i32; 9]>,
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

/// One entry of the `FriendsListUpdate` (0x0021) roster.
#[derive(Clone, Debug)]
pub struct Friend {
    pub id: u32,
    /// Online (0 / 1 on the wire; "appear offline" is a separate 0 field).
    pub online: bool,
    pub name: String,
}

/// One merchandise line of the `ApproachVendor` (0x0062) event. The item
/// state comes from the per-item ObjectCreate-style tail; the vendor item's
/// instance guid is that tail's guid.
#[derive(Clone, Debug)]
pub struct VendorItem {
    pub guid: u32,
    pub name: String,
    pub item_type: u32,
    pub value: u32,
    /// The vendor's sell price (base value x the vendor's sell mod).
    pub sell_price: f32,
    /// The price the player pays (base value x the vendor's buy mod).
    pub buy_price: f32,
    pub stack_size: u32,
    pub icon_id: u32,
}

/// A single `Enchantment` registry entry (60 bytes + 4 if `has_spell_set_id`
/// is set): `u16 spellId, u16 layer, u16 spellCategory, u16 hasSpellSetId,
/// u32 powerLevel, f64 startTime, f64 duration, u32 casterGuid,
/// f32 degradeModifier, f32 degradeLimit, f64 lastTimeDegraded,
/// u32 statModType, u32 statModKey, f32 statModValue, [u32 spellSetId]`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Enchantment {
    pub spell_id: u16,
    pub layer: u16,
    pub spell_category: u16,
    pub has_spell_set_id: u16,
    pub power_level: u32,
    pub start_time: f64,
    pub duration: f64,
    pub caster_guid: u32,
    pub degrade_modifier: f32,
    pub degrade_limit: f32,
    pub last_time_degraded: f64,
    pub stat_mod_type: u32,
    pub stat_mod_key: u32,
    pub stat_mod_value: f32,
    pub spell_set_id: u32,
}

/// One page of a book (from `BookDataResponse` 0x00B4 or a separate
/// `BookPageData` fetch).
#[derive(Clone, Debug, Default)]
pub struct BookPage {
    pub page: u32,
    pub author: String,
    pub text: String,
}

/// A squelch (chat filter) info block: `u32 nFilters, nFilters x u32,
/// String16L name, u32 account`.
#[derive(Clone, Debug, Default)]
pub struct SquelchInfo {
    /// Filtered chat-channel ids (e.g. 2 = General, 3 = Trade).
    pub filters: Vec<u32>,
    pub name: String,
    /// Whether the entry is an account (true) or character (false).
    pub account: bool,
}

/// A character entry of the `SetSquelchDB` (0x01F4) table: `u32 id` +
/// a `SquelchInfo`.
#[derive(Clone, Debug, Default)]
pub struct SquelchEntry {
    pub id: u32,
    pub info: SquelchInfo,
}

/// A contract tracker row: `u32 version, u32 contractId, u32 stage,
/// f64 timeWhenDone, f64 timeWhenRepeats`. Stage: 0 = none, 1 = Available,
/// 2 = InProgress, 3 = DoneOrPending, 4 = Counter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Contract {
    pub id: u32,
    pub version: u32,
    pub stage: u32,
    pub time_when_done: f64,
    pub time_when_repeats: f64,
}

/// One allegiance member record of the `AllegianceUpdate` (0x0020) event
/// (AllegianceData, version 0xB).
#[derive(Clone, Debug)]
pub struct AllegianceMember {
    pub guid: u32,
    pub level: u32,
    pub rank: u16,
    pub loyalty: u16,
    pub leadership: u16,
    /// The bitfield's online bit (bit 0x1 = "in game" flag 0x02).
    pub online: bool,
    pub name: String,
}

/// One fellow of the `FellowshipFullUpdate` (0x02BE) roster.
#[derive(Clone, Debug)]
pub struct Fellow {
    pub guid: u32,
    pub level: u32,
    /// (max, current)
    pub health: (u32, u32),
    /// (max, current)
    pub stamina: (u32, u32),
    /// (max, current)
    pub mana: (u32, u32),
    pub name: String,
}

/// A house purchase/rent payment line (HouseData 0x0225): `i32 num,
/// i32 paid, u32 weenieId, String16L name, String16L pluralName`.
#[derive(Clone, Debug, Default)]
pub struct HousePayment {
    pub num: i32,
    pub paid: i32,
    pub weenie_id: u32,
    pub name: String,
    pub plural_name: String,
}

/// `AppraiseInfo` creature block (CreatureProfile, flag 0x100).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AppraiseCreature {
    pub max_health: i32,
    pub health: i32,
    /// Strength, Endurance, Quickness, Coordination, Focus, Self.
    pub attrs: [i32; 6],
    /// Stamina, Mana, StaminaMax, ManaMax.
    pub vitals: [i32; 4],
}

/// `AppraiseInfo` weapon block (WeaponProfile, flag 0x20).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AppraiseWeapon {
    pub damage_min: i32,
    /// Damage variance (the wire carries an f64; stored truncated).
    pub damage_var: i32,
    /// The weapon skill (e.g. 1 = Axe, 11 = Sword).
    pub skill: u32,
}

/// `AppraiseInfo` armor block (ArmorProfile, flag 0x80): 8 f32 resist
/// values — Slashing, Piercing, Bludgeoning, Cold, Fire, Acid, Nether,
/// Lightning.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppraiseArmor {
    pub resist: [f32; 8],
}

/// `ApproachVendor` (0x0062): a vendor shop opened for the player.
///
/// Body (after the GameEvent prefix): `u32 vendorGuid, u32
/// merchandiseItemTypes, u32 minValue, u32 maxValue, u32 dealMagicalItems,
/// f32 buyPrice, f32 sellPrice, u32 altCurrencyWcid,
/// [wcid != 0: u32 count + String16L name | u32 0 + String16L ""],
/// u32 numItems, per item: u32 stackPrefix + u32 itemGuid + object desc
/// tail`.
#[derive(Clone, Debug, Default)]
pub struct ApproachVendorInfo {
    pub vendor_guid: u32,
    /// `ItemType` bitmask of what the vendor buys back (sells-tab filter).
    pub merchandise_item_types: u32,
    pub min_value: u32,
    pub max_value: u32,
    /// The vendor deals in magical items too.
    pub deal_magical_items: bool,
    /// The vendor's BUY rate (what it pays per unit of value on a sale).
    pub buy_price: f32,
    /// The vendor's SELL rate (what you pay per unit of value on a buy).
    pub sell_price: f32,
    /// Alternate-currency wcid (0 = pyreals).
    pub alt_currency_wcid: u32,
    /// The player's current alternate-currency holding (0 when pyreals).
    pub alt_currency_count: u32,
    pub alt_currency_name: String,
    pub items: Vec<VendorItem>,
}

/// `AllegianceUpdate` (0x0020): rank + the allegiance member hierarchy.
#[derive(Clone, Debug, Default)]
pub struct AllegianceUpdateInfo {
    /// This player's rank within their allegiance (0 when not in one).
    pub rank: u32,
    pub total_members: u32,
    pub total_vassals: u32,
    pub name: String,
    pub chat_room: u32,
    pub locked: bool,
    /// Monarch first, then the remaining records in tree order.
    pub members: Vec<AllegianceMember>,
}

/// `FellowshipFullUpdate` (0x02BE): the full fellowship roster.
#[derive(Clone, Debug, Default)]
pub struct FellowshipFullUpdateInfo {
    pub fellows: Vec<Fellow>,
    pub name: String,
    pub leader_guid: u32,
    pub share_xp: bool,
    pub even_share: bool,
    pub open: bool,
    pub locked: bool,
}

/// `HouseData` (0x0225): house panel info (retail layout).
#[derive(Clone, Debug, Default)]
pub struct HouseDataInfo {
    pub dwelling_id: u32,
    pub owner_id: u32,
    /// 1=cottage, 2=villa, 3=mansion, 4=apartment.
    pub house_type: u32,
    pub maintenance_free: bool,
    pub owner_name: String,
    pub buy: Vec<HousePayment>,
    pub rent: Vec<HousePayment>,
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
    PopupString {
        text: String,
    },
    /// `FriendsListUpdate` (0x0021).
    FriendsListUpdate {
        friends: Vec<Friend>,
        update_type: u32,
    },
    /// `InventoryPutObjInContainer` (0x0022): item moved (or unequipped to
    /// the main pack, when `container_guid` is the player's own guid).
    InventoryPutObjInContainer {
        obj_guid: u32,
        container_guid: u32,
        placement: u32,
        container_type: u32,
    },
    /// `WieldObject` (0x0023): item equipped. `location` is the EquipMask.
    WieldObject {
        obj_guid: u32,
        location: i32,
    },
    /// `CharacterTitle` (0x0029): current title + owned title ids.
    CharacterTitle {
        current: u32,
        owned: Vec<u32>,
    },
    /// `InventoryPutObjectIn3D` (0x019A): item dropped to the 3D world.
    InventoryPutObjectIn3D {
        obj_guid: u32,
    },
    /// `InventoryServerSaveFailed` (0x00A0): the server failed to persist an
    /// inventory change (u32 itemGuid, u32 WeenieError).
    InventoryServerSaveFailed {
        item_guid: u32,
        code: u32,
    },
    /// `ApproachVendor` (0x0062): a vendor shop opened for the player.
    ApproachVendor(ApproachVendorInfo),
    /// `WeenieError` (0x028A): a bare error code.
    WeenieError {
        code: u32,
    },
    /// `WeenieErrorWithString` (0x028B): an error code with a message.
    WeenieErrorWithString {
        code: u32,
        text: String,
    },
    /// `RegisterTrade` (0x01FD): a trade session started.
    RegisterTrade {
        initiator: u32,
        partner: u32,
    },
    /// `CloseTrade` (0x01FF): the trade was closed (`reason` = WeenieError).
    CloseTrade {
        reason: u32,
    },
    /// `AddToTrade` (0x0200): an item was placed in the trade window.
    AddToTrade {
        item: u32,
        side: u32,
        slot: u32,
    },
    /// `RemoveFromTrade` (0x0201): an item was taken out of the trade window.
    RemoveFromTrade {
        item: u32,
        side: u32,
    },
    /// `AcceptTrade` (0x0202): the other side accepted the trade.
    AcceptTrade {
        guid: u32,
    },
    /// `DeclineTrade` (0x0203): the other side declined the trade.
    DeclineTrade {
        guid: u32,
    },
    /// `ResetTrade` (0x0205): the trade was reset (items returned).
    ResetTrade {
        guid: u32,
    },
    /// `TradeFailure` (0x0207): a proposed trade item was rejected.
    TradeFailure {
        item: u32,
        code: u32,
    },
    /// `ClearTradeAcceptance` (0x0208): trade acceptance state cleared.
    ClearTradeAcceptance,
    /// `CommunicationTransientString` (0x02EB): a transient chat string.
    CommunicationTransientString {
        text: String,
    },
    /// `MagicUpdateEnchantment` (0x02C2): one enchantment changed on self.
    MagicUpdateEnchantment {
        enchantment: Enchantment,
    },
    /// `MagicRemoveEnchantment` (0x02C3): one enchantment removed from self.
    MagicRemoveEnchantment {
        spell_id: u16,
        layer: u16,
    },
    /// `UpdateTitle` (0x002B): a title was (un)assigned or set as display.
    UpdateTitle {
        title_id: u32,
        set_as_display: bool,
    },
    /// `SetSquelchDB` (0x01F4): the full squelch (chat filter) state.
    SetSquelchDB {
        entries: Vec<SquelchEntry>,
        global: SquelchInfo,
    },
    /// `SendClientContractTrackerTable` (0x0314): all tracked contracts.
    SendClientContractTrackerTable {
        contracts: Vec<Contract>,
    },
    /// `SendClientContractTracker` (0x0315): one contract tracker updated.
    SendClientContractTracker {
        contract: Contract,
    },
    /// `AllegianceUpdate` (0x0020): rank + the allegiance member hierarchy.
    AllegianceUpdate(AllegianceUpdateInfo),
    /// `FellowshipFullUpdate` (0x02BE): the full fellowship roster.
    FellowshipFullUpdate(FellowshipFullUpdateInfo),
    /// `HouseData` (0x0225): house panel info (retail layout).
    HouseData(HouseDataInfo),
    /// `HouseStatus` (0x0226): house query answer (u32 WeenieError).
    HouseStatus {
        code: u32,
    },
    /// `CloseGroundContainer` (0x0052): a ground container was closed.
    CloseGroundContainer {
        container_guid: u32,
    },
    /// `ViewContents` (0x0196): a ground container's contents listed.
    ViewContents {
        container_guid: u32,
        /// (itemGuid, containerType)
        items: Vec<(u32, u32)>,
    },
    /// `UseDone` (0x01C7): a Use / UseWithTarget finished. `text` is the
    /// retail lock-error string for known error codes (None otherwise).
    UseDone {
        code: u32,
        text: Option<String>,
    },
    /// `AttackDone` (0x01A7): an attack finished (u32 WeenieError).
    AttackDone {
        code: u32,
    },
    /// `MagicUpdateSpell` (0x02C1): a spell was learned/updated in the
    /// spellbook.
    MagicUpdateSpell {
        spell_id: u32,
        layer: u32,
    },
    /// `MagicRemoveSpell` (0x01A8): a spell was removed from the spellbook.
    MagicRemoveSpell {
        spell_id: u32,
        layer: u32,
    },
    /// `BookDataResponse` (0x00B4): the full book (page metadata + text).
    BookDataResponse {
        book_guid: u32,
        max_pages: u32,
        max_chars: u32,
        pages: Vec<BookPage>,
        inscription: String,
        author: String,
    },
    /// `BookPageDataResponse` (0x00B8): a single page of a book.
    BookPageDataResponse {
        book_guid: u32,
        page: u32,
        author: String,
        text: String,
    },
    /// `BookModifyPageResponse` (0x00B5) / `BookAddPageResponse` (0x00B6) /
    /// `BookDeletePageResponse` (0x00B7): a book mutation result.
    BookMutationResponse {
        book_guid: u32,
        page: u32,
        success: bool,
    },
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
    /// `EmoteText` (0x01E0) / `SoulEmote` (0x01E2): a broadcast emote or
    /// soul emote from a player (including self).
    EmoteText {
        sender_id: u32,
        sender: String,
        text: String,
    },
    /// `PlayerKilled` (0x019E): broadcast when a player dies, including to self.
    PlayerKilled {
        message: String,
        victim: u32,
        killer: u32,
    },
    /// `PrivateUpdateAttribute2ndLevel` (0x02E9): server-authoritative vitals for self.
    VitalUpdate {
        vital: u32,
        value: u32,
    },
    /// `HearRangedSpeech` (0x02BC): out-of-earshot speech with a range.
    HearRangedSpeech {
        text: String,
        sender: String,
        sender_id: u32,
        distance: f32,
        chat_type: u32,
    },
    /// `PrivateUpdatePropertyInt` (0x02CD): a self PropertyInt update
    /// (e.g. CoinValue after a buy/sell).
    PrivateUpdatePropertyInt {
        prop: u32,
        value: i32,
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
        opcode::PLAYER_KILLED => match crate::strings::read_string16l(r.buf, r.pos) {
            Some((message, pos)) => {
                r.pos = pos;
                if r.remaining() < 8 {
                    return Some(Incoming::Other(op));
                }
                let victim = r.u32();
                let killer = r.u32();
                Some(Incoming::PlayerKilled { message: message.to_string(), victim, killer })
            }
            None => Some(Incoming::Other(op)),
        },
        opcode::PRIVATE_UPDATE_ATTRIBUTE_2ND_LEVEL => {
            if r.remaining() < 9 {
                return Some(Incoming::Other(op));
            }
            r.u8(); // per-vital sequence (ByteSequence)
            let vital = r.u32();
            let value = r.u32();
            Some(Incoming::VitalUpdate { vital, value })
        }
        opcode::PRIVATE_UPDATE_VITAL => {
            // u32 seq, u32 vital, u32 ranks, u32 starting, u32 xpSpent,
            // u32 current. The server may send either this or 0x02E9.
            if r.remaining() < 24 {
                return Some(Incoming::Other(op));
            }
            r.u32(); // seq
            let vital = r.u32();
            r.u32(); // ranks
            r.u32(); // starting value
            r.u32(); // xp spent
            let value = r.u32(); // current
            Some(Incoming::VitalUpdate { vital, value })
        }
        opcode::PRIVATE_UPDATE_PROPERTY_INT => {
            // u32 seq, u32 property, i32 value.
            if r.remaining() < 12 {
                return Some(Incoming::Other(op));
            }
            r.u32(); // seq
            let prop = r.u32();
            let value = r.i32();
            Some(Incoming::PrivateUpdatePropertyInt { prop, value })
        }
        opcode::HEAR_RANGED_SPEECH => {
            // String16L text, String16L sender, u32 senderID, f32 range,
            // u32 chatType.
            if r.remaining() < 4 {
                return Some(Incoming::Other(op));
            }
            let text = r.string16l();
            let sender = r.string16l();
            if r.remaining() < 12 {
                return Some(Incoming::Other(op));
            }
            let sender_id = r.u32();
            let distance = r.f32();
            let chat_type = r.u32();
            Some(Incoming::HearRangedSpeech { text, sender, sender_id, distance, chat_type })
        }
        opcode::EMOTE_TEXT | opcode::SOUL_EMOTE_MSG => {
            // u32 senderId, String16L senderName, String16L emoteText.
            if r.remaining() < 4 {
                return Some(Incoming::Other(op));
            }
            let sender_id = r.u32();
            let sender = r.string16l();
            let text = r.string16l();
            Some(Incoming::EmoteText { sender_id, sender, text })
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
        event::POPUP_STRING => match try_string16l(r) {
            Some(text) => Event::PopupString { text },
            None => Event::Unknown,
        },
        event::FRIENDS_LIST_UPDATE => {
            let (friends, update_type) = parse_friends_list(r);
            Event::FriendsListUpdate { friends, update_type }
        }
        event::INVENTORY_PUT_OBJ_IN_CONTAINER => {
            if r.remaining() < 16 {
                Event::Unknown
            } else {
                let obj_guid = r.u32();
                let container_guid = r.u32();
                let placement = r.u32();
                let container_type = r.u32();
                Event::InventoryPutObjInContainer {
                    obj_guid,
                    container_guid,
                    placement,
                    container_type,
                }
            }
        }
        event::WIELD_OBJECT => {
            if r.remaining() < 8 {
                Event::Unknown
            } else {
                let obj_guid = r.u32();
                let location = r.i32();
                Event::WieldObject { obj_guid, location }
            }
        }
        event::CHARACTER_TITLE => {
            if r.remaining() < 12 {
                Event::Unknown
            } else {
                r.u32(); // leading 1
                let current = r.u32();
                let count = r.u32() as usize;
                if r.remaining() < count.saturating_mul(4) {
                    Event::Unknown
                } else {
                    let mut owned = Vec::with_capacity(count);
                    for _ in 0..count {
                        owned.push(r.u32());
                    }
                    Event::CharacterTitle { current, owned }
                }
            }
        }
        event::INVENTORY_PUT_OBJECT_IN_3D => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                Event::InventoryPutObjectIn3D { obj_guid: r.u32() }
            }
        }
        event::INVENTORY_SERVER_SAVE_FAILED => {
            if r.remaining() < 8 {
                Event::Unknown
            } else {
                let item_guid = r.u32();
                let code = r.u32();
                Event::InventoryServerSaveFailed { item_guid, code }
            }
        }
        event::APPROACH_VENDOR => Event::ApproachVendor(parse_approach_vendor(r)),
        event::WEENIE_ERROR => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                Event::WeenieError { code: r.u32() }
            }
        }
        event::WEENIE_ERROR_WITH_STRING => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                let code = r.u32();
                match try_string16l(r) {
                    Some(text) => Event::WeenieErrorWithString { code, text },
                    None => Event::WeenieError { code },
                }
            }
        }
        event::REGISTER_TRADE => {
            if r.remaining() < 16 {
                Event::Unknown
            } else {
                let initiator = r.u32();
                let partner = r.u32();
                r.u64(); // trailing 0
                Event::RegisterTrade { initiator, partner }
            }
        }
        event::CLOSE_TRADE => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                Event::CloseTrade { reason: r.u32() }
            }
        }
        event::ADD_TO_TRADE => {
            if r.remaining() < 12 {
                Event::Unknown
            } else {
                let item = r.u32();
                let side = r.u32();
                let slot = r.u32();
                Event::AddToTrade { item, side, slot }
            }
        }
        event::REMOVE_FROM_TRADE => {
            if r.remaining() < 8 {
                Event::Unknown
            } else {
                let item = r.u32();
                let side = r.u32();
                Event::RemoveFromTrade { item, side }
            }
        }
        event::ACCEPT_TRADE => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                Event::AcceptTrade { guid: r.u32() }
            }
        }
        event::DECLINE_TRADE => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                Event::DeclineTrade { guid: r.u32() }
            }
        }
        event::RESET_TRADE => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                Event::ResetTrade { guid: r.u32() }
            }
        }
        event::TRADE_FAILURE => {
            if r.remaining() < 8 {
                Event::Unknown
            } else {
                let item = r.u32();
                let code = r.u32();
                Event::TradeFailure { item, code }
            }
        }
        event::CLEAR_TRADE_ACCEPTANCE => Event::ClearTradeAcceptance,
        event::COMMUNICATION_TRANSIENT_STRING => match try_string16l(r) {
            Some(text) => Event::CommunicationTransientString { text },
            None => Event::Unknown,
        },
        event::MAGIC_UPDATE_ENCHANTMENT => {
            if r.remaining() < 60 {
                Event::Unknown
            } else {
                Event::MagicUpdateEnchantment { enchantment: parse_enchantment(r) }
            }
        }
        event::MAGIC_REMOVE_ENCHANTMENT => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                let spell_id = r.u16();
                let layer = r.u16();
                Event::MagicRemoveEnchantment { spell_id, layer }
            }
        }
        event::UPDATE_TITLE => {
            if r.remaining() < 8 {
                Event::Unknown
            } else {
                let title_id = r.u32();
                let set_as_display = r.u32() != 0;
                Event::UpdateTitle { title_id, set_as_display }
            }
        }
        event::SET_SQUELCH_DB => {
            let (entries, global) = parse_squelch_db(r);
            Event::SetSquelchDB { entries, global }
        }
        event::SEND_CLIENT_CONTRACT_TRACKER_TABLE => {
            Event::SendClientContractTrackerTable { contracts: parse_contract_table(r) }
        }
        event::SEND_CLIENT_CONTRACT_TRACKER => Event::SendClientContractTracker {
            contract: parse_contract(r),
        },
        event::ALLEGIANCE_UPDATE => Event::AllegianceUpdate(parse_allegiance_update(r)),
        event::FELLOWSHIP_FULL_UPDATE => Event::FellowshipFullUpdate(parse_fellowship_full_update(r)),
        event::HOUSE_DATA => Event::HouseData(parse_house_data(r)),
        event::HOUSE_STATUS => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                Event::HouseStatus { code: r.u32() }
            }
        }
        event::CLOSE_GROUND_CONTAINER => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                Event::CloseGroundContainer { container_guid: r.u32() }
            }
        }
        event::VIEW_CONTENTS => {
            let (container_guid, items) = parse_view_contents(r);
            Event::ViewContents { container_guid, items }
        }
        event::USE_DONE => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                let code = r.u32();
                let text = USE_DONE_TEXT
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map(|(_, t)| t.to_string());
                Event::UseDone { code, text }
            }
        }
        event::ATTACK_DONE => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                Event::AttackDone { code: r.u32() }
            }
        }
        event::MAGIC_UPDATE_SPELL => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                let spell_id = r.u16() as u32;
                let layer = r.u16() as u32;
                Event::MagicUpdateSpell { spell_id, layer }
            }
        }
        event::MAGIC_REMOVE_SPELL => {
            if r.remaining() < 4 {
                Event::Unknown
            } else {
                let spell_id = r.u16() as u32;
                let layer = r.u16() as u32;
                Event::MagicRemoveSpell { spell_id, layer }
            }
        }
        event::BOOK_DATA_RESPONSE => parse_book_data_response(r),
        event::BOOK_PAGE_DATA_RESPONSE => parse_book_page_data_response(r),
        event::BOOK_MODIFY_PAGE_RESPONSE
        | event::BOOK_ADD_PAGE_RESPONSE
        | event::BOOK_DELETE_PAGE_RESPONSE => {
            if r.remaining() < 12 {
                Event::Unknown
            } else {
                let book_guid = r.u32();
                let page = r.u32();
                let success = r.u32() != 0;
                Event::BookMutationResponse { book_guid, page, success }
            }
        }
        _ => Event::Unknown,
    }
}

/// `BookDataResponse` (0x00B4): `u32 bookID, u32 maxPages, u32 numPages,
/// u32 maxChars, u32 pageCount, pageCount x page, String16L inscription,
/// u32 authorId, String16L authorName`. Each page is `u32 authorId,
/// String16L authorName, String16L authorAccount, u32 flags(0xFFFF0002),
/// u32 textIncluded [, u32 ignoreAuthor, String16L text]`.
fn parse_book_data_response(r: &mut Reader) -> Event {
    if r.remaining() < 20 {
        return Event::Unknown;
    }
    let book_guid = r.u32();
    let max_pages = r.u32();
    let _num_pages = r.u32();
    let max_chars = r.u32();
    let page_count = r.u32() as usize;
    let mut pages = Vec::with_capacity(page_count);
    for page in 0..page_count {
        if r.remaining() < 8 {
            break;
        }
        let _author_id = r.u32();
        let author = r.string16l();
        let _author_account = r.string16l();
        r.u32(); // flags (0xFFFF0002)
        if r.remaining() < 4 {
            break;
        }
        let text_included = r.u32();
        let mut text = String::new();
        if text_included != 0 {
            if r.remaining() < 4 {
                break;
            }
            r.u32(); // ignoreAuthor
            text = r.string16l();
        }
        pages.push(BookPage { page: page as u32, author, text });
    }
    let inscription = r.string16l();
    if r.remaining() < 8 {
        return Event::Unknown;
    }
    let _author_id = r.u32();
    let author = r.string16l();
    Event::BookDataResponse {
        book_guid,
        max_pages,
        max_chars,
        pages,
        inscription,
        author,
    }
}

/// `BookPageDataResponse` (0x00B8): `u32 bookID, u32 pageIndex, u32
/// authorId, String16L authorName, String16L authorAccount, u32 flags,
/// u32 textIncluded, u32 ignoreAuthor, String16L text`.
fn parse_book_page_data_response(r: &mut Reader) -> Event {
    if r.remaining() < 12 {
        return Event::Unknown;
    }
    let book_guid = r.u32();
    let page = r.u32();
    let _author_id = r.u32();
    let author = r.string16l();
    let _author_account = r.string16l();
    r.u32(); // flags (0xFFFF0002)
    if r.remaining() < 8 {
        return Event::Unknown;
    }
    let text_included = r.u32();
    if text_included == 0 {
        return Event::Unknown;
    }
    r.u32(); // ignoreAuthor
    let text = r.string16l();
    Event::BookPageDataResponse {
        book_guid,
        page,
        author,
        text,
    }
}

/// `PlayerDescription` (0x0013): the property tables, attribute/vital
/// blocks, and the variable-length tail (skills, spells, enchantment
/// registry, options, spell bar, inventory, equipped). Every section is
/// bounds-checked; a truncated message returns the partial parse so far.
fn parse_player_description(r: &mut Reader) -> PlayerDescription {
    let mut pd = PlayerDescription::default();

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

    // Skills (vector flag 0x0002): PackableHashTable, 32-byte entries.
    if vector_flags & 0x0002 != 0 {
        let count = read_hash_header(r).0;
        for _ in 0..count {
            if r.remaining() < 32 {
                break;
            }
            let id = r.u32();
            let ranks = r.u16();
            r.u16(); // always 1
            r.u32(); // advancementClass
            r.u32(); // xpSpent
            r.u32(); // initLevel
            r.u32(); // always 0
            r.f64(); // always 0
            pd.skills.push((id, ranks as i32));
        }
    }

    // Spells (vector flag 0x0100): 8-byte entries.
    if vector_flags & 0x0100 != 0 {
        let count = read_hash_header(r).0;
        for _ in 0..count {
            if r.remaining() < 8 {
                break;
            }
            pd.spells.push(r.u32());
            r.f32(); // cast time
        }
    }

    // Enchantment registry (vector flag 0x0200): mask-gated lists. The
    // Vitae entry (mask bit 0x4, spell 666) carries the death-penalty
    // multiplier in stat_mod_value.
    if vector_flags & 0x0200 != 0 {
        let mask = r.u32();
        for &bit in &[0x1u32, 0x2, 0x8] {
            if mask & bit != 0 {
                let n = r.u32() as usize;
                for _ in 0..n {
                    if !enchantment_complete(r) {
                        return finalize_pd(&mut pd);
                    }
                    pd.enchantments.push(parse_enchantment(r));
                }
            }
        }
        if mask & 0x4 != 0 {
            if !enchantment_complete(r) {
                return finalize_pd(&mut pd);
            }
            pd.enchantments.push(parse_enchantment(r));
        }
    }

    let option_flags = r.u32();
    r.u32(); // characterOptions1

    if option_flags & 0x0001 != 0 {
        // Shortcuts: 16-byte entries.
        let count = r.u32() as usize;
        if !skip_section(r, count.saturating_mul(16)) {
            return finalize_pd(&mut pd);
        }
    }
    if option_flags & 0x0400 != 0 {
        // SpellLists8 (always set by the server): 8 tabs.
        for _ in 0..8 {
            let count = r.u32() as usize;
            let mut tab = Vec::with_capacity(count);
            for _ in 0..count {
                if r.remaining() < 4 {
                    break;
                }
                tab.push(r.u32());
            }
            pd.spell_bar.push(tab);
        }
    } else {
        r.u32();
        for _ in 0..8 {
            pd.spell_bar.push(Vec::new());
        }
    }
    if option_flags & 0x0008 != 0 {
        // DesiredComps: PackableHashTable, 8-byte entries.
        if !skip_table(r, 8) {
            return finalize_pd(&mut pd);
        }
    }
    // spellbookFilters (always present).
    pd.spell_filters.push(r.u32());
    if option_flags & 0x0040 != 0 {
        r.raw(4); // CharacterOptions2
    }
    if option_flags & 0x0200 != 0 {
        // GameplayOptions: raw byte[] with no length prefix — it cannot be
        // skipped reliably; stop here (inventory/equipped stay empty).
        return finalize_pd(&mut pd);
    }
    let c_inv = r.u32() as usize;
    for _ in 0..c_inv {
        if r.remaining() < 8 {
            break;
        }
        pd.inventory.push((r.u32(), r.u32()));
    }
    let c_eq = r.u32() as usize;
    for _ in 0..c_eq {
        if r.remaining() < 12 {
            break;
        }
        pd.equipped.push((r.u32(), r.u32(), r.u32()));
    }
    finalize_pd(&mut pd)
}

/// Fill the derived PlayerDescription fields (coin_value, encumbrance,
/// strength) from the already-parsed int table and attributes, then take the
/// struct out of its `&mut` binding.
fn finalize_pd(pd: &mut PlayerDescription) -> PlayerDescription {
    for &(key, val) in &pd.ints {
        match key {
            20 => pd.coin_value = val as i64, // CoinValue
            5 => pd.encumbrance = val, // Encumbrance
            _ => {}
        }
    }
    pd.strength = pd.attrs[0] as i32;
    std::mem::take(pd)
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
        // Derived int fields.
        for &(key, val) in &a.ints {
            match key {
                25 => a.level = Some(val), // Level
                19 => a.value = Some(val), // Value
                5 => a.encumbrance = Some(val), // Encumbrance
                107 => a.item_mana = Some(val), // ItemMana
                108 => a.item_mana_max = Some(val), // ItemManaMax
                33 => a.bonded = true, // Bonded
                114 => a.attuned = true, // Attuned
                _ => {}
            }
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
        let mut strs: Vec<(u32, String)> = Vec::with_capacity(count);
        for _ in 0..count {
            if r.remaining() < 6 {
                break;
            }
            let key = r.u32();
            match try_string16l(r) {
                Some(s) => strs.push((key, s)),
                None => break,
            }
        }
        // description = first non-empty of Description(16), Biography(15),
        // Mention(14), Name(5).
        for &key in &[16u32, 15, 14, 5] {
            if let Some((_, s)) = strs.iter().find(|(k, _)| *k == key) {
                if !s.is_empty() {
                    a.description = s.clone();
                    break;
                }
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
    if flags & 0x80 != 0 {
        if r.remaining() < 32 {
            return a;
        }
        let mut resist = [0.0f32; 8];
        for v in resist.iter_mut() {
            *v = r.f32();
        }
        a.armor = Some(AppraiseArmor { resist });
    }
    // CreatureProfile (0x100): flags, health, health_max,
    // [0x8: 10 × u32 attributes/vitals], [0x1: 2 × u16 buff masks]
    if flags & 0x100 != 0 {
        if r.remaining() < 12 {
            return a;
        }
        let cflags = r.u32();
        a.health = r.u32();
        a.health_max = r.u32();
        let mut cre = AppraiseCreature {
            max_health: a.health_max as i32,
            health: a.health as i32,
            attrs: [0; 6],
            vitals: [0; 4],
        };
        if cflags & 0x8 != 0 {
            if r.remaining() < 40 {
                return a;
            }
            // Str, End, Quick, Coord, Focus, Self, Stamina, Mana,
            // StaminaMax, ManaMax.
            let mut vals = [0u32; 10];
            for v in vals.iter_mut() {
                *v = r.u32();
            }
            cre.attrs = [
                vals[0] as i32,
                vals[1] as i32,
                vals[2] as i32,
                vals[3] as i32,
                vals[4] as i32,
                vals[5] as i32,
            ];
            cre.vitals = [
                vals[6] as i32,
                vals[7] as i32,
                vals[8] as i32,
                vals[9] as i32,
            ];
        }
        if cflags & 0x1 != 0 && !skip_section(r, 4) {
            return a;
        }
        a.creature = Some(cre);
    }
    // WeaponProfile (0x20): 4 × u32 ++ 5 × f64 ++ u32 = 60 bytes
    if flags & 0x20 != 0 {
        if r.remaining() < 60 {
            return a;
        }
        r.u32(); // damageType
        r.u32(); // weaponTime
        let skill = r.u32(); // weaponSkill
        let damage = r.u32(); // damage
        let variance = r.f64(); // damageVariance
        r.f64(); // damageMod
        r.f64(); // weaponLength
        r.f64(); // maxVelocity
        r.f64(); // weaponOffense
        r.u32(); // maxVelocityEstimated
        a.weapon = Some(AppraiseWeapon {
            damage_min: damage as i32,
            damage_var: variance as i32,
            skill,
        });
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
    if flags & 0x4000 != 0 {
        if r.remaining() < 36 {
            return a;
        }
        let mut levels = [0i32; 9];
        for v in levels.iter_mut() {
            *v = r.u32() as i32;
        }
        a.armor_levels = Some(levels);
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
/// weenieFlags, String16L name, <description tail>`. Decodes the guid and,
/// best-effort, the name, position, item fields (from the description tail),
/// physics state, and the movement section's `forward` command (door state).
/// Returns whatever was parsed even if the tail is truncated.
fn parse_object_create(data: &[u8]) -> ObjectCreate {
    let mut oc = ObjectCreate {
        guid: 0,
        name: None,
        position: None,
        item_type: 0,
        value: 0,
        stack_size: 0,
        items_capacity: 0,
        container_id: 0,
        wielder_id: 0,
        valid_locs: 0,
        cur_loc: 0,
        mtable_id: None,
        setup_id: None,
        model_parts: None,
        is_vendor: false,
        motion_cmd: 0,
        physics_state: 0,
        use_radius: None,
    };
    if data.len() < 4 {
        return oc;
    }
    let mut r = Reader::new(data);
    oc.guid = r.u32();

    // --- ModelData ---
    if r.remaining() < 4 {
        return oc;
    }
    let marker = r.u8();
    if marker == 0x11 && r.remaining() >= 3 {
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
        let mut model_parts = Vec::with_capacity(nanim);
        for _ in 0..nanim {
            let part = r.u8() as usize;
            let model = read_packed_dword(&mut r, 0x1000000);
            model_parts.push((part, model));
        }
        oc.model_parts = Some(model_parts);
        r.align4();
        // --- PhysicsData ---
        if r.remaining() < 8 {
            return oc;
        }
        let phys = r.u32();
        oc.physics_state = r.u32();
        if phys & 0x010000 != 0 {
            let len = r.u32() as usize;
            if len > 0 {
                oc.motion_cmd = parse_movement_forward(&mut r, len);
                r.u32(); // isAutonomous
            }
        }
        if phys & 0x020000 != 0 {
            r.u32(); // placement
        }
        if phys & 0x008000 != 0 {
            oc.position = Some(Position::unpack(&mut r));
        }
        if phys & 0x000002 != 0 {
            oc.mtable_id = Some(r.u32());
        }
        if phys & 0x000800 != 0 {
            r.u32(); // STable
        }
        if phys & 0x001000 != 0 {
            r.u32(); // PeTable
        }
        if phys & 0x000001 != 0 {
            oc.setup_id = Some(r.u32());
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
        // Velocity (0x4), Acceleration (0x8), Omega (0x10) — each a single
        // Vec3, written once by the server.
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
            r.u16(); // sequence timestamps
        }
        r.align4();
    }

    // --- object description tail (best effort; shared with ApproachVendor) ---
    r.align4();
    let tail = parse_object_desc_tail(&mut r);
    oc.name = tail.name;
    oc.item_type = tail.item_type;
    oc.is_vendor = tail.is_vendor;
    oc.value = tail.value;
    oc.stack_size = tail.stack_size;
    oc.items_capacity = tail.items_capacity;
    oc.container_id = tail.container_id;
    oc.wielder_id = tail.wielder_id;
    oc.valid_locs = tail.valid_locs;
    oc.cur_loc = tail.cur_loc;
    oc.use_radius = tail.use_radius;
    oc
}

/// Parse a single enchantment (60 fixed bytes + optional 4-byte spellSetId
/// when `hasSpellSetId != 0`).
fn parse_enchantment(r: &mut Reader) -> Enchantment {
    let mut e = Enchantment::default();
    if r.remaining() < 60 {
        return e;
    }
    e.spell_id = r.u16();
    e.layer = r.u16();
    e.spell_category = r.u16();
    e.has_spell_set_id = r.u16();
    e.power_level = r.u32();
    e.start_time = r.f64();
    e.duration = r.f64();
    e.caster_guid = r.u32();
    e.degrade_modifier = r.f32();
    e.degrade_limit = r.f32();
    e.last_time_degraded = r.f64();
    e.stat_mod_type = r.u32();
    e.stat_mod_key = r.u32();
    e.stat_mod_value = r.f32();
    if e.has_spell_set_id != 0 {
        if r.remaining() >= 4 {
            e.spell_set_id = r.u32();
        }
    }
    e
}

/// Return whether a complete enchantment (60 bytes, plus 4 when
/// `hasSpellSetId` is set) is present at the reader's current position.
/// Peeks only; `parse_enchantment` does the consuming read.
fn enchantment_complete(r: &Reader) -> bool {
    if r.remaining() < 60 {
        return false;
    }
    let has_spell_set = u16::from_le_bytes([r.buf[r.pos + 6], r.buf[r.pos + 7]]) != 0;
    !has_spell_set || r.remaining() >= 64
}

/// `FriendsListUpdate` (0x0021): `u32 count`, per friend
/// `u32 id, u32 online, u32 appearOffline, String16L name, u32, u32`, then
/// `u32 updateType`.
fn parse_friends_list(r: &mut Reader) -> (Vec<Friend>, u32) {
    let mut friends = Vec::new();
    let mut update_type = 0;
    if r.remaining() < 4 {
        return (friends, update_type);
    }
    let count = r.u32() as usize;
    for _ in 0..count {
        if r.remaining() < 12 {
            break;
        }
        let id = r.u32();
        let online = r.u32() != 0;
        r.u32(); // appearOffline (always 0)
        let name = match try_string16l(r) {
            Some(n) => n,
            None => break,
        };
        if r.remaining() < 8 {
            break;
        }
        r.u32(); // count of this friend's friends (always 0)
        r.u32(); // inverse count (always 0)
        friends.push(Friend { id, online, name });
    }
    if r.remaining() >= 4 {
        update_type = r.u32();
    }
    (friends, update_type)
}

/// `ApproachVendor` (0x0062): the vendor shop panel.
fn parse_approach_vendor(r: &mut Reader) -> ApproachVendorInfo {
    let mut v = ApproachVendorInfo::default();
    if r.remaining() < 12 {
        return v;
    }
    v.vendor_guid = r.u32();
    v.merchandise_item_types = r.u32();
    v.min_value = r.u32();
    if r.remaining() < 12 {
        return v;
    }
    v.max_value = r.u32();
    v.deal_magical_items = r.u32() != 0;
    v.buy_price = r.f32();
    if r.remaining() < 12 {
        return v;
    }
    v.sell_price = r.f32();
    v.alt_currency_wcid = r.u32();
    if v.alt_currency_wcid != 0 {
        if r.remaining() < 4 {
            return v;
        }
        v.alt_currency_count = r.u32();
        v.alt_currency_name = try_string16l(r).unwrap_or_default();
    } else {
        if r.remaining() < 4 {
            return v;
        }
        r.u32(); // 0
        try_string16l(r); // ""
    }
    if r.remaining() < 4 {
        return v;
    }
    let num_items = r.u32() as usize;
    for _ in 0..num_items {
        if r.remaining() < 8 {
            break;
        }
        let prefix = r.u32(); // (stackSize & 0xFFFFFF) | (0xFF << 24)
        let item_guid = r.u32();
        let tail = parse_object_desc_tail(r);
        let stack_raw = prefix & 0x00FF_FFFF;
        let stack_size = if stack_raw == 0x00FF_FFFF { 1 } else { stack_raw };
        let value = tail.value;
        let mut vi = VendorItem {
            guid: item_guid,
            name: tail.name.unwrap_or_default(),
            item_type: tail.item_type,
            value,
            sell_price: 0.0,
            buy_price: 0.0,
            stack_size,
            icon_id: tail.icon_id,
        };
        // The panel displays vendor rate x base value.
        vi.sell_price = (v.sell_price * value as f32).round();
        vi.buy_price = (v.buy_price * value as f32).round();
        v.items.push(vi);
    }
    v
}

/// One `SquelchInfo`: `u32 nFilters, nFilters x u32, String16L name, u32
/// account`.
fn read_squelch_info(r: &mut Reader) -> SquelchInfo {
    let mut info = SquelchInfo::default();
    if r.remaining() < 4 {
        return info;
    }
    let n_filters = r.u32() as usize;
    for _ in 0..n_filters {
        if r.remaining() < 4 {
            return info;
        }
        info.filters.push(r.u32());
    }
    match try_string16l(r) {
        Some(n) => info.name = n,
        None => return info,
    }
    if r.remaining() < 4 {
        return info;
    }
    info.account = r.u32() != 0;
    info
}

/// `SetSquelchDB` (0x01F4): `u16 accountCount(0), u16 accountBuckets(0),
/// u16 count, u16 buckets(32), count x (u32 id + SquelchInfo), SquelchInfo
/// (global)`.
fn parse_squelch_db(r: &mut Reader) -> (Vec<SquelchEntry>, SquelchInfo) {
    let mut entries = Vec::new();
    if r.remaining() < 8 {
        return (entries, SquelchInfo::default());
    }
    r.u16(); // account count (always 0)
    r.u16(); // account buckets (always 0)
    let count = r.u16() as usize;
    r.u16(); // buckets (32)
    for _ in 0..count {
        if r.remaining() < 4 {
            break;
        }
        let id = r.u32();
        let info = read_squelch_info(r);
        entries.push(SquelchEntry { id, info });
    }
    let global = read_squelch_info(r);
    (entries, global)
}

/// One contract tracker: `u32 version, u32 contractId, u32 stage, f64
/// timeWhenDone, f64 timeWhenRepeats`.
fn parse_contract(r: &mut Reader) -> Contract {
    let mut c = Contract::default();
    if r.remaining() < 24 {
        return c;
    }
    c.version = r.u32();
    c.id = r.u32();
    c.stage = r.u32();
    c.time_when_done = r.f64();
    c.time_when_repeats = r.f64();
    c
}

/// `SendClientContractTrackerTable` (0x0314): `u16 count, u16 buckets(32),
/// count x (u32 id + contract tracker)`.
fn parse_contract_table(r: &mut Reader) -> Vec<Contract> {
    let mut contracts = Vec::new();
    if r.remaining() < 4 {
        return contracts;
    }
    let count = r.u16() as usize;
    r.u16(); // buckets (32)
    for _ in 0..count {
        if r.remaining() < 4 {
            break;
        }
        let key = r.u32(); // contract id
        let tracker = parse_contract(r);
        contracts.push(Contract { id: key, ..tracker });
    }
    contracts
}

/// One `AllegianceData` record: guid, cpCached, cpTithed, bitfield, gender,
/// heritage, rank, [level if bitfield & 0x8], loyalty, leadership, 8 bytes
/// (timeOnline + age or deprecated u64), name.
fn read_allegiance_data(r: &mut Reader) -> Option<AllegianceMember> {
    if r.remaining() < 16 {
        return None;
    }
    let guid = r.u32();
    r.u32(); // cpCached
    r.u32(); // cpTithed
    let bitfield = r.u32();
    if r.remaining() < 4 {
        return None;
    }
    r.u8(); // gender
    r.u8(); // heritage
    let rank = r.u16();
    let level = if bitfield & 0x8 != 0 {
        if r.remaining() < 4 {
            return None;
        }
        r.u32()
    } else {
        0
    };
    if r.remaining() < 4 {
        return None;
    }
    let loyalty = r.u16();
    let leadership = r.u16();
    if r.remaining() < 8 {
        return None;
    }
    r.raw(8);
    let name = try_string16l(r)?;
    Some(AllegianceMember {
        guid,
        level,
        rank,
        loyalty,
        leadership,
        online: bitfield & 0x1 != 0,
        name,
    })
}

/// `AllegianceUpdate` (0x0020): the full allegiance hierarchy.
fn parse_allegiance_update(r: &mut Reader) -> AllegianceUpdateInfo {
    let mut a = AllegianceUpdateInfo::default();
    if r.remaining() < 12 {
        return a;
    }
    a.rank = r.u32();
    a.total_members = r.u32();
    a.total_vassals = r.u32();
    if r.remaining() < 8 {
        return a;
    }
    let record_count = r.u16() as usize;
    r.u16(); // oldVersion (0xB)
    r.u16(); // officers count (always 0)
    r.u16(); // officers buckets (256)
    if r.remaining() < 4 {
        return a;
    }
    let titles_count = r.u32() as usize;
    for _ in 0..titles_count {
        if try_string16l(r).is_none() {
            return a;
        }
    }
    if r.remaining() < 16 {
        return a;
    }
    r.u32(); // monarchBroadcastTime
    r.u32(); // monarchBroadcastsToday
    r.u32(); // spokesBroadcastTime
    r.u32(); // spokesBroadcastsToday
    if try_string16l(r).is_none() {
        return a; // motd
    }
    if try_string16l(r).is_none() {
        return a; // motdSetBy
    }
    if r.remaining() < 4 {
        return a;
    }
    a.chat_room = r.u32();
    if r.remaining() < 4 {
        return a;
    }
    r.u32(); // bindpoint cell
    if r.remaining() < 28 {
        return a;
    }
    r.raw(28); // Pos (3 f32) + Rotation (4 f32)
    match try_string16l(r) {
        Some(n) => a.name = n,
        None => return a,
    }
    if r.remaining() < 8 {
        return a;
    }
    r.u32(); // nameLastSetTime
    a.locked = r.u32() != 0;
    if r.remaining() < 4 {
        return a;
    }
    r.raw(4); // i32 approvedVassal
    if record_count > 0 {
        if let Some(monarch) = read_allegiance_data(r) {
            a.members.push(monarch);
        }
        for _ in 1..record_count {
            if r.remaining() < 4 {
                break;
            }
            r.u32(); // parent GUID
            match read_allegiance_data(r) {
                Some(m) => a.members.push(m),
                None => break,
            }
        }
    }
    a
}

/// `FellowshipFullUpdate` (0x02BE): the full fellowship roster.
fn parse_fellowship_full_update(r: &mut Reader) -> FellowshipFullUpdateInfo {
    let mut f = FellowshipFullUpdateInfo::default();
    if r.remaining() < 4 {
        return f;
    }
    let count = r.u16() as usize;
    r.u16(); // buckets (16)
    for _ in 0..count {
        if r.remaining() < 44 {
            break;
        }
        let guid = r.u32();
        r.u32(); // cpCached
        r.u32(); // lumCached
        let level = r.u32();
        let h_max = r.u32();
        let s_max = r.u32();
        let m_max = r.u32();
        let h_cur = r.u32();
        let s_cur = r.u32();
        let m_cur = r.u32();
        r.u32(); // shareLoot
        let name = match try_string16l(r) {
            Some(n) => n,
            None => break,
        };
        f.fellows.push(Fellow {
            guid,
            level,
            health: (h_max, h_cur),
            stamina: (s_max, s_cur),
            mana: (m_max, m_cur),
            name,
        });
    }
    match try_string16l(r) {
        Some(n) => f.name = n,
        None => return f,
    }
    if r.remaining() < 20 {
        return f;
    }
    f.leader_guid = r.u32();
    f.share_xp = r.u32() != 0;
    f.even_share = r.u32() != 0;
    f.open = r.u32() != 0;
    f.locked = r.u32() != 0;
    // DepartedMembers: u16 count u16 buckets, count x (u32 guid + i32 time).
    if r.remaining() >= 4 {
        let dep_count = r.u16() as usize;
        r.u16();
        if r.remaining() < dep_count.saturating_mul(8) {
            return f;
        }
        r.raw(dep_count.saturating_mul(8));
    }
    // FellowshipLocks: u16 count u16 buckets, count x (String16L key + 5 u32).
    if r.remaining() >= 4 {
        let lock_count = r.u16() as usize;
        r.u16();
        for _ in 0..lock_count {
            if try_string16l(r).is_none() {
                return f;
            }
            if r.remaining() < 20 {
                return f;
            }
            r.raw(20);
        }
    }
    f
}

/// A house payment list: `u32 count`, each `i32 num, i32 paid, u32
/// weenieId, String16L name, String16L pluralName`.
fn read_house_payments(r: &mut Reader) -> Vec<HousePayment> {
    let mut out = Vec::new();
    if r.remaining() < 4 {
        return out;
    }
    let count = r.u32() as usize;
    for _ in 0..count {
        if r.remaining() < 12 {
            break;
        }
        let num = r.i32();
        let paid = r.i32();
        let weenie_id = r.u32();
        let name = match try_string16l(r) {
            Some(n) => n,
            None => break,
        };
        let plural_name = try_string16l(r).unwrap_or_default();
        out.push(HousePayment { num, paid, weenie_id, name, plural_name });
    }
    out
}

/// `HouseData` (0x0225): house panel info (retail layout).
fn parse_house_data(r: &mut Reader) -> HouseDataInfo {
    let mut h = HouseDataInfo::default();
    if r.remaining() < 36 {
        return h;
    }
    h.dwelling_id = r.u32();
    h.owner_id = r.u32();
    r.u32(); // bitmask
    r.i32(); // minLevel
    r.i32(); // maxLevel
    r.i32(); // minAllegRank
    r.i32(); // maxAllegRank
    h.maintenance_free = r.u32() != 0;
    h.house_type = r.u32();
    match try_string16l(r) {
        Some(n) => h.owner_name = n,
        None => return h,
    }
    h.buy = read_house_payments(r);
    h.rent = read_house_payments(r);
    h
}

/// `ViewContents` (0x0196): `u32 containerGuid, u32 count, count x (u32
/// itemGuid, u32 containerType)`.
fn parse_view_contents(r: &mut Reader) -> (u32, Vec<(u32, u32)>) {
    let mut items = Vec::new();
    let mut container_guid = 0;
    if r.remaining() < 8 {
        return (container_guid, items);
    }
    container_guid = r.u32();
    let count = r.u32() as usize;
    for _ in 0..count {
        if r.remaining() < 8 {
            break;
        }
        let item = r.u32();
        let ct = r.u32();
        items.push((item, ct));
    }
    (container_guid, items)
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

/// Fields captured from the "object description" tail (the part of an
/// ObjectCreate / per-vendor-item body after the guid, starting at
/// `u32 weenieFlags`). Populated best-effort: `name` is `None` and the rest
/// stay at their defaults if the tail is truncated.
#[derive(Clone, Debug, Default)]
struct ObjectDescTail {
    name: Option<String>,
    item_type: u32,
    is_vendor: bool,
    value: u32,
    stack_size: u32,
    items_capacity: u32,
    container_id: u32,
    wielder_id: u32,
    valid_locs: i32,
    cur_loc: i32,
    icon_id: u32,
    use_radius: Option<f32>,
}

/// `ReadString16L` without panicking on a truncated buffer.
fn try_string16l(r: &mut Reader) -> Option<String> {
    if r.remaining() < 2 {
        return None;
    }
    match crate::strings::read_string16l(r.buf, r.pos) {
        Some((s, pos)) => {
            r.pos = pos;
            Some(s.to_string())
        }
        None => None,
    }
}

/// Map a `u16` motion command to the full 32-bit `MotionCommand` id (the
/// client stores the low 16 bits in the movement section). Unmapped,
/// non-zero low 16 bits are treated as a full 32-bit passthrough.
pub(crate) fn cmd_from_u16(v: u32) -> u32 {
    match v {
        0x0003 => 0x4100_0003, // Ready
        0x0005 => 0x4500_0005, // WalkForward
        0x0006 => 0x4500_0006, // WalkBackwards
        0x0007 => 0x4400_0007, // RunForward
        0x000d => 0x6500_000d, // TurnRight
        0x000e => 0x6500_000e, // TurnLeft
        0x000b => 0x4000_000b, // On (door open)
        0x000c => 0x4000_000c, // Off (door closed)
        0x000f => 0x6500_000f, // SideStepRight
        0x0010 => 0x6500_0010, // SideStepLeft
        0 => 0,
        // Unmapped non-zero codes (combat Thrust/Slash/Attack/Aim/spell-cast,
        // etc.) are passed through as their low-16 value; the motion table
        // keys on motion_cmd & 0xfffff, so the low 16 bits are what resolve.
        _ => v
    }
}

/// The `forward` MotionCommand of a `len`-byte movement section (PhysicsData
/// `Movement` flag). `r` is advanced by `len` unconditionally; the command is
/// only meaningful for movement types 0/2 (raw/interpreted command).
fn parse_movement_forward(r: &mut Reader, len: usize) -> u32 {
    if r.remaining() < len {
        r.raw(r.remaining());
        return 0;
    }
    if len < 4 {
        r.raw(len);
        return 0;
    }
    let slice = r.raw(len);
    let mut s = Reader::new(slice);
    let mt = s.u8();
    let mflags = s.u8();
    if s.remaining() < 2 {
        return 0;
    }
    s.u16(); // motionStyle low
    if mt == 0 || mt == 2 {
        if s.remaining() < 4 {
            return 0;
        }
        let f = s.u32();
        let n_cmds = (f >> 7) as usize;
        if f & 0x1 != 0 && s.remaining() >= 2 {
            s.u16();
        }
        let mut forward = 0u32;
        if f & 0x2 != 0 {
            if s.remaining() < 2 {
                return 0;
            }
            forward = cmd_from_u16(s.u16() as u32);
        }
        if f & 0x4 != 0 && s.remaining() >= 2 {
            s.u16();
        }
        if f & 0x8 != 0 && s.remaining() >= 2 {
            s.u16();
        }
        if f & 0x10 != 0 && s.remaining() >= 4 {
            s.f32();
        }
        if f & 0x20 != 0 && s.remaining() >= 4 {
            s.f32();
        }
        if f & 0x40 != 0 && s.remaining() >= 4 {
            s.f32();
        }
        for _ in 0..n_cmds {
            if s.remaining() < 8 {
                return forward;
            }
            s.u16();
            s.u16();
            s.f32();
        }
        s.align4();
        if mflags & 0x1 != 0 {
            s.u32();
        }
        forward
    } else if mt == 6 || mt == 7 {
        if mt == 6 && s.remaining() >= 4 {
            s.u32();
        }
        0
    } else {
        0
    }
}

/// Skip a `RestrictionDB` (HouseRestrictions flag): `u32 version, u32
/// openStatus, u32 monarchID, u16 count, u16 numBuckets, count x (u32 guid +
/// u32 perm)`.
fn skip_restriction_db(r: &mut Reader) {
    if r.remaining() < 16 {
        return;
    }
    r.raw(12);
    let count = r.u16() as usize;
    r.u16();
    r.raw(count.saturating_mul(8));
}

/// Parse the object description tail. `r` must be positioned at the
/// `u32 weenieFlags` word. Consumes the ENTIRE tail (every WeenieHeaderFlag
/// field in the server's fixed write order, the weenieFlags2 fields, and the
/// final alignment) so the caller can advance to the next back-to-back item.
fn parse_object_desc_tail(r: &mut Reader) -> ObjectDescTail {
    let mut t = ObjectDescTail::default();
    if r.remaining() < 4 {
        return t;
    }
    let weenie_flags = r.u32();
    match try_string16l(r) {
        Some(name) => t.name = Some(name),
        None => return t,
    }
    if r.remaining() < 2 {
        return t;
    }
    read_packed_dword(r, 0); // WeenieClassId
    if r.remaining() < 2 {
        return t;
    }
    t.icon_id = read_packed_dword(r, 0); // IconId
    if r.remaining() < 8 {
        return t;
    }
    t.item_type = r.u32();
    let odf = r.u32();
    t.is_vendor = odf & 0x0000_0200 != 0;
    r.align4();
    let weenie_flags2 = if odf & 0x0400_0000 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.u32()
    } else {
        0
    };
    if weenie_flags & 0x0000_0001 != 0 && try_string16l(r).is_none() {
        return t; // PluralName
    }
    if weenie_flags & 0x0000_0002 != 0 {
        if r.remaining() < 1 {
            return t;
        }
        t.items_capacity = r.u8() as u32;
    }
    if weenie_flags & 0x0000_0004 != 0 {
        if r.remaining() < 1 {
            return t;
        }
        r.raw(1); // ContainersCapacity
    }
    if weenie_flags & 0x0000_0100 != 0 {
        if r.remaining() < 2 {
            return t;
        }
        r.raw(2); // AmmoType
    }
    if weenie_flags & 0x0000_0008 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        t.value = r.u32();
    }
    if weenie_flags & 0x0000_0010 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.raw(4); // Usable
    }
    if weenie_flags & 0x0000_0020 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        // PropertyFloat UseRadius: the distance at which the object can be
        // used (approach stop distance / vendor auto-close radius).
        t.use_radius = Some(r.f32());
    }
    if weenie_flags & 0x0008_0000 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.raw(4); // TargetType
    }
    if weenie_flags & 0x0000_0080 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.raw(4); // UiEffects
    }
    if weenie_flags & 0x0000_0200 != 0 {
        if r.remaining() < 1 {
            return t;
        }
        r.raw(1); // CombatUse
    }
    if weenie_flags & 0x0000_0400 != 0 {
        if r.remaining() < 2 {
            return t;
        }
        r.raw(2); // Structure
    }
    if weenie_flags & 0x0000_0800 != 0 {
        if r.remaining() < 2 {
            return t;
        }
        r.raw(2); // MaxStructure
    }
    if weenie_flags & 0x0000_1000 != 0 {
        if r.remaining() < 2 {
            return t;
        }
        t.stack_size = r.u16() as u32;
    }
    if weenie_flags & 0x0000_2000 != 0 {
        if r.remaining() < 2 {
            return t;
        }
        r.raw(2); // MaxStackSize
    }
    if weenie_flags & 0x0000_4000 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        t.container_id = r.u32();
    }
    if weenie_flags & 0x0000_8000 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        t.wielder_id = r.u32();
    }
    if weenie_flags & 0x0001_0000 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        t.valid_locs = r.i32();
    }
    if weenie_flags & 0x0002_0000 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        t.cur_loc = r.i32();
    }
    if weenie_flags & 0x0004_0000 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.raw(4); // Priority
    }
    if weenie_flags & 0x0010_0000 != 0 {
        if r.remaining() < 1 {
            return t;
        }
        r.raw(1); // RadarBlipColor
    }
    if weenie_flags & 0x0080_0000 != 0 {
        if r.remaining() < 1 {
            return t;
        }
        r.raw(1); // RadarBehavior
    }
    if weenie_flags & 0x0800_0000 != 0 {
        if r.remaining() < 2 {
            return t;
        }
        r.raw(2); // PScript
    }
    if weenie_flags & 0x0100_0000 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.raw(4); // Workmanship
    }
    if weenie_flags & 0x0020_0000 != 0 {
        if r.remaining() < 2 {
            return t;
        }
        r.raw(2); // Burden
    }
    if weenie_flags & 0x0040_0000 != 0 {
        if r.remaining() < 2 {
            return t;
        }
        r.raw(2); // Spell
    }
    if weenie_flags & 0x0200_0000 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.raw(4); // HouseOwner
    }
    if weenie_flags & 0x0400_0000 != 0 {
        skip_restriction_db(r); // HouseRestrictions
    }
    if weenie_flags & 0x2000_0000 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.raw(4); // HookItemTypes
    }
    if weenie_flags & 0x0000_0040 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.raw(4); // Monarch
    }
    if weenie_flags & 0x1000_0000 != 0 {
        if r.remaining() < 2 {
            return t;
        }
        r.raw(2); // HookType
    }
    if weenie_flags & 0x4000_0000 != 0 {
        if r.remaining() < 2 {
            return t;
        }
        read_packed_dword(r, 0); // IconOverlay
    }
    if weenie_flags2 & 0x01 != 0 {
        if r.remaining() < 2 {
            return t;
        }
        read_packed_dword(r, 0); // IconUnderlay
    }
    if weenie_flags & 0x8000_0000 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.raw(4); // MaterialType
    }
    if weenie_flags2 & 0x02 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.raw(4); // Cooldown
    }
    if weenie_flags2 & 0x04 != 0 {
        if r.remaining() < 8 {
            return t;
        }
        r.raw(8); // CooldownDuration
    }
    if weenie_flags2 & 0x08 != 0 {
        if r.remaining() < 4 {
            return t;
        }
        r.raw(4); // PetOwner
    }
    r.align4();
    t
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
        let b = body_targeted_melee_attack(0x0011_0042, 3, 0.5);
        assert_eq!(b.len(), 12);
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), 0x0011_0042);
        assert_eq!(r.u32(), 3); // height
        assert!((r.f32() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn missile_attack_body() {
        let b = body_targeted_missile_attack(0x0011_0043, 2, 1.0);
        assert_eq!(b.len(), 12);
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), 0x0011_0043);
        assert_eq!(r.u32(), 2);
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

    #[test]
    fn player_killed_layout() {
        let mut w = Writer::new();
        w.u32(opcode::PLAYER_KILLED);
        w.string16l("+TuiHero was slain by Black Bunny");
        w.u32(0x5000_01D0); // victim
        w.u32(0x7860_0001); // killer
        match parse_incoming(&w.out).expect("parse") {
            Incoming::PlayerKilled { message, victim, killer } => {
                assert_eq!(message, "+TuiHero was slain by Black Bunny");
                assert_eq!(victim, 0x5000_01D0);
                assert_eq!(killer, 0x7860_0001);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn vital_update_layout() {
        let mut w = Writer::new();
        w.u32(opcode::PRIVATE_UPDATE_ATTRIBUTE_2ND_LEVEL);
        w.u8(7); // per-vital sequence
        w.u32(vital::HEALTH);
        w.u32(4); // 75% of a max of 5, rounded
        match parse_incoming(&w.out).expect("parse") {
            Incoming::VitalUpdate { vital, value } => {
                assert_eq!(vital, vital::HEALTH);
                assert_eq!(value, 4);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn player_killed_truncated_does_not_panic() {
        // The string prefix claims more bytes than are present.
        let mut d = Vec::new();
        d.extend_from_slice(&opcode::PLAYER_KILLED.to_le_bytes());
        d.extend_from_slice(&100u16.to_le_bytes());
        match parse_incoming(&d) {
            None | Some(Incoming::Other(0x019E)) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn vital_update_truncated_does_not_panic() {
        let mut d = Vec::new();
        d.extend_from_slice(&opcode::PRIVATE_UPDATE_ATTRIBUTE_2ND_LEVEL.to_le_bytes());
        d.extend_from_slice(&[7u8, 2, 0, 0]); // seq + partial vital
        match parse_incoming(&d) {
            None | Some(Incoming::Other(0x02E9)) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn player_killed_capture() {
        // Exact 0x019E capture from the live server log: +TuiHero (0x500001D0)
        // killed by a Black Bunny (0x786020B3) in block 0x8602. Layout:
        // string16l message, then victim guid, then killer guid.
        let text = b"+TuiHero's perforated corpse falls before Black Bunny!";
        assert_eq!(text.len(), 54);
        let mut msg = vec![
            0x9E, 0x01, 0x00, 0x00, // opcode PLAYER_KILLED
            0x36, 0x00, // string16l len 54 (2+54 = 56, already 4-aligned)
        ];
        msg.extend_from_slice(text);
        msg.extend_from_slice(&[
            0xD0, 0x01, 0x00, 0x50, // victim 0x500001D0 (+TuiHero)
            0xB3, 0x20, 0x60, 0x78, // killer 0x786020B3 (Black Bunny)
        ]);
        assert_eq!(msg.len(), 4 + 2 + 54 + 4 + 4);
        match parse_incoming(&msg).expect("parse") {
            Incoming::PlayerKilled { message, victim, killer } => {
                assert_eq!(
                    message,
                    "+TuiHero's perforated corpse falls before Black Bunny!"
                );
                assert_eq!(victim, 0x5000_01D0);
                assert_eq!(killer, 0x7860_20B3);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn vital_update_capture() {
        // Exact 0x02E9 captures from the live server log, in arrival order:
        // the death (Health -> 0) and the lifestone revive (Health -> 5)
        // of the same session. Layout: u8 per-vital seq, u32 vital, u32 value.
        let death: &[u8] = &[
            0xE9, 0x02, 0x00, 0x00, // opcode PRIVATE_UPDATE_ATTRIBUTE_2ND_LEVEL
            0x03, // seq (per-vital counter)
            0x02, 0x00, 0x00, 0x00, // vital HEALTH
            0x00, 0x00, 0x00, 0x00, // value 0 (dead)
        ];
        match parse_incoming(death).expect("parse") {
            Incoming::VitalUpdate { vital, value } => {
                assert_eq!(vital, vital::HEALTH);
                assert_eq!(value, 0);
            }
            other => panic!("unexpected: {other:?}"),
        }
        let revive: &[u8] = &[
            0xE9, 0x02, 0x00, 0x00,
            0x07, // seq
            0x02, 0x00, 0x00, 0x00, // vital HEALTH
            0x05, 0x00, 0x00, 0x00, // value 5 (lifestone full revive)
        ];
        match parse_incoming(revive).expect("parse") {
            Incoming::VitalUpdate { vital, value } => {
                assert_eq!(vital, vital::HEALTH);
                assert_eq!(value, 5);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// Build a full `GAME_EVENT` message and return its decoded `Event`.
    fn ge_event(event_type: u32, body: &[u8]) -> Event {
        let mut w = Writer::new();
        w.u32(opcode::GAME_EVENT);
        w.u32(0x5000_01D0); // player guid
        w.u32(1); // event seq
        w.u32(event_type);
        w.out.extend_from_slice(body);
        let mut r = Reader::new(&w.out);
        r.u32(); // opcode
        parse_game_event(&mut r).expect("game event").event
    }

    #[test]
    fn trade_action_body_layouts() {
        // ACCEPT_TRADE is the canonical 28-byte body.
        let b = body_accept_trade(0x0028_0200);
        assert_eq!(b.len(), 28);
        let mut r = Reader::new(&b);
        assert_eq!(r.i32(), 0x0028_0200 as i32);
        assert_eq!(r.f64(), 0.0);
        assert_eq!(r.u32(), 0);
        assert_eq!(r.u32(), 0x0028_0200);
        assert_eq!(r.u32(), 1);
        assert_eq!(r.u32(), 1);
        assert_eq!(r.remaining(), 0);

        assert_eq!(body_open_trade(7), 7u32.to_le_bytes().to_vec());
        assert!(body_close_trade().is_empty());
        assert!(body_decline_trade().is_empty());
        assert!(body_reset_trade().is_empty());

        let b = body_add_to_trade(0x0028_0001, 3);
        assert_eq!(b.len(), 8);
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), 0x0028_0001);
        assert_eq!(r.u32(), 3);

        // BUY: vendor, count, per-item (i32 amount, u32 objectId).
        let b = body_buy(0x0028_0100, &[(2, 0x0028_0001), (1, 0x0028_0002)]);
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), 0x0028_0100);
        assert_eq!(r.u32(), 2);
        assert_eq!(r.i32(), 2);
        assert_eq!(r.u32(), 0x0028_0001);
        assert_eq!(r.i32(), 1);
        assert_eq!(r.u32(), 0x0028_0002);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn private_update_vital_02e7() {
        let mut w = Writer::new();
        w.u32(opcode::PRIVATE_UPDATE_VITAL);
        w.u32(9); // seq
        w.u32(vital::HEALTH); // vital
        w.u32(0); // ranks
        w.u32(100); // starting
        w.u32(0); // xp
        w.u32(87); // current
        match parse_incoming(&w.out).expect("parse") {
            Incoming::VitalUpdate { vital, value } => {
                assert_eq!(vital, vital::HEALTH);
                assert_eq!(value, 87);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn private_update_property_int_02cd() {
        let mut w = Writer::new();
        w.u32(opcode::PRIVATE_UPDATE_PROPERTY_INT);
        w.u32(3); // seq
        w.u32(20); // CoinValue
        w.i32(-150); // value (a sale)
        match parse_incoming(&w.out).expect("parse") {
            Incoming::PrivateUpdatePropertyInt { prop, value } => {
                assert_eq!(prop, 20);
                assert_eq!(value, -150);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn hear_ranged_speech_02bc() {
        let mut w = Writer::new();
        w.u32(opcode::HEAR_RANGED_SPEECH);
        w.string16l("I can't hear you!");
        w.string16l("Rurik");
        w.u32(0x0028_0042);
        w.f32(42.5);
        w.u32(chat_type::SPEECH);
        match parse_incoming(&w.out).expect("parse") {
            Incoming::HearRangedSpeech { text, sender, sender_id, distance, chat_type } => {
                assert_eq!(text, "I can't hear you!");
                assert_eq!(sender, "Rurik");
                assert_eq!(sender_id, 0x0028_0042);
                assert_eq!(distance, 42.5);
                assert_eq!(chat_type, chat_type::SPEECH);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn popup_string_event() {
        let mut b = Writer::new();
        b.string16l("You found 125 pyreals.");
        let Event::PopupString { text } = ge_event(event::POPUP_STRING, &b.out) else {
            panic!("expected PopupString");
        };
        assert_eq!(text, "You found 125 pyreals.");
    }

    #[test]
    fn friends_list_event() {
        let mut b = Writer::new();
        b.u32(1); // count
        b.u32(0x0028_0001); // id
        b.u32(1); // online
        b.u32(0); // appearOffline
        b.string16l("Rurik");
        b.u32(0);
        b.u32(0);
        b.u32(1); // updateType
        let Event::FriendsListUpdate { friends, update_type } = ge_event(event::FRIENDS_LIST_UPDATE, &b.out) else {
            panic!("expected FriendsListUpdate");
        };
        assert_eq!(update_type, 1);
        assert_eq!(friends.len(), 1);
        assert_eq!(friends[0].id, 0x0028_0001);
        assert!(friends[0].online);
        assert_eq!(friends[0].name, "Rurik");
    }

    #[test]
    fn character_title_event() {
        let mut b = Writer::new();
        b.u32(1); // leading 1
        b.u32(0); // current (None)
        b.u32(2); // count
        b.u32(10); // owned
        b.u32(42); // owned
        let Event::CharacterTitle { current, owned } = ge_event(event::CHARACTER_TITLE, &b.out) else {
            panic!("expected CharacterTitle");
        };
        assert_eq!(current, 0);
        assert_eq!(owned, vec![10, 42]);
    }

    #[test]
    fn trade_events() {
        let mut b = Writer::new();
        b.u32(0x0028_0100); // initiator
        b.u32(0x0028_0200); // partner
        b.u64(0); // trailing 0
        let Event::RegisterTrade { initiator, partner } = ge_event(event::REGISTER_TRADE, &b.out) else {
            panic!("expected RegisterTrade");
        };
        assert_eq!(initiator, 0x0028_0100);
        assert_eq!(partner, 0x0028_0200);

        let mut b = Writer::new();
        b.u32(0x0028_0001); // item
        b.u32(trade_side::PARTNER); // side
        b.u32(5); // slot
        let Event::AddToTrade { item, side, slot } = ge_event(event::ADD_TO_TRADE, &b.out) else {
            panic!("expected AddToTrade");
        };
        assert_eq!(item, 0x0028_0001);
        assert_eq!(side, trade_side::PARTNER);
        assert_eq!(slot, 5);

        let mut b = Writer::new();
        b.u32(0x0028_0200); // guid
        let Event::AcceptTrade { guid } = ge_event(event::ACCEPT_TRADE, &b.out) else {
            panic!("expected AcceptTrade");
        };
        assert_eq!(guid, 0x0028_0200);
    }

    #[test]
    fn use_done_event_text() {
        // A known lock-error code maps to the retail string.
        let known = USE_DONE_TEXT.first().expect("non-empty table");
        let (code, text) = known;
        let mut b = Writer::new();
        b.u32(*code);
        let Event::UseDone { code: c, text: t } = ge_event(event::USE_DONE, &b.out) else {
            panic!("expected UseDone");
        };
        assert_eq!(c, *code);
        assert_eq!(t.as_deref(), Some(*text));

        // Code 0 (success) has no text.
        let mut b = Writer::new();
        b.u32(0);
        let Event::UseDone { code: c, text: t } = ge_event(event::USE_DONE, &b.out) else {
            panic!("expected UseDone");
        };
        assert_eq!(c, 0);
        assert!(t.is_none());
    }

    #[test]
    fn enchantment_event() {
        let mut b = Writer::new();
        b.u16(0x0184); // spellId
        b.u16(3); // layer
        b.u16(2); // spellCategory
        b.u16(1); // hasSpellSetId
        b.u32(0); // powerLevel
        b.f64(100.0); // startTime
        b.f64(7200.0); // duration
        b.u32(0x0028_0042); // casterGuid
        b.f32(1.0); // degradeModifier
        b.f32(0.0); // degradeLimit
        b.f64(0.0); // lastTimeDegraded
        b.u32(1); // statModType
        b.u32(2); // statModKey
        b.f32(1.25); // statModValue
        b.u32(0x0001_0000); // spellSetId
        let Event::MagicUpdateEnchantment { enchantment } = ge_event(event::MAGIC_UPDATE_ENCHANTMENT, &b.out) else {
            panic!("expected MagicUpdateEnchantment");
        };
        assert_eq!(enchantment.spell_id, 0x0184);
        assert_eq!(enchantment.layer, 3);
        assert_eq!(enchantment.caster_guid, 0x0028_0042);
        assert_eq!(enchantment.stat_mod_value, 1.25);
        assert_eq!(enchantment.spell_set_id, 0x0001_0000);
    }

    #[test]
    fn enchantment_remove_event() {
        let mut b = Writer::new();
        b.u16(0x0184); // spellId
        b.u16(3); // layer
        let Event::MagicRemoveEnchantment { spell_id, layer } = ge_event(event::MAGIC_REMOVE_ENCHANTMENT, &b.out) else {
            panic!("expected MagicRemoveEnchantment");
        };
        assert_eq!(spell_id, 0x0184);
        assert_eq!(layer, 3);
    }

    #[test]
    fn approach_vendor_event() {
        let mut b = Writer::new();
        b.u32(0x0028_0100); // vendorGuid
        b.u32(0x1000); // merchandiseItemTypes
        b.u32(0); // minValue
        b.u32(10000); // maxValue
        b.u32(1); // dealMagicalItems
        b.f32(2.0); // buyPrice
        b.f32(1.0); // sellPrice
        b.u32(0); // altCurrencyWcid (0 = pyreals)
        b.u32(0); // (wcid==0) count
        b.string16l(""); // (wcid==0) name
        b.u32(1); // numItems
        // item: stackPrefix, itemGuid, object desc tail
        b.u32(0xFF00_0001); // stackPrefix (stackSize 1)
        b.u32(0x0028_0001); // itemGuid
        b.u32(0x8); // weenieFlags (Value)
        b.string16l("Healer Vial");
        b.u16(1); // WeenieClassId (packed, small)
        b.u16(0); // IconId (packed, small)
        b.u32(0x20); // itemType
        b.u32(0); // ODF
        b.u32(24); // value (weenieFlags & 0x8)
        let Event::ApproachVendor(v) = ge_event(event::APPROACH_VENDOR, &b.out) else {
            panic!("expected ApproachVendor");
        };
        assert_eq!(v.vendor_guid, 0x0028_0100);
        assert!(v.deal_magical_items);
        assert_eq!(v.buy_price, 2.0);
        assert_eq!(v.sell_price, 1.0);
        assert_eq!(v.items.len(), 1);
        let it = &v.items[0];
        assert_eq!(it.guid, 0x0028_0001);
        assert_eq!(it.name, "Healer Vial");
        assert_eq!(it.value, 24);
        assert_eq!(it.stack_size, 1);
        assert_eq!(it.buy_price, 48.0); // 2.0 x 24
        assert_eq!(it.sell_price, 24.0); // 1.0 x 24
    }

    #[test]
    fn view_contents_event() {
        let mut b = Writer::new();
        b.u32(0x0028_0300); // containerGuid
        b.u32(2); // count
        b.u32(0x0028_0001);
        b.u32(0); // containerType (item)
        b.u32(0x0028_0002);
        b.u32(1); // containerType (Container)
        let Event::ViewContents { container_guid, items } = ge_event(event::VIEW_CONTENTS, &b.out) else {
            panic!("expected ViewContents");
        };
        assert_eq!(container_guid, 0x0028_0300);
        assert_eq!(items, vec![(0x0028_0001, 0), (0x0028_0002, 1)]);
    }

    #[test]
    fn house_data_event() {
        let mut b = Writer::new();
        b.u32(0x0028_0400); // dwellingId
        b.u32(0x0028_0001); // ownerId
        b.u32(0); // bitmask
        b.i32(0); // minLevel
        b.i32(0); // maxLevel
        b.i32(0); // minAllegRank
        b.i32(0); // maxAllegRank
        b.u32(1); // maintenanceFree
        b.u32(3); // type (mansion)
        b.string16l("Rurik");
        // buy payments
        b.u32(1); // count
        b.i32(100); // num
        b.i32(50); // paid
        b.u32(0x10); // weenieId
        b.string16l("pyreal");
        b.string16l("pyreals");
        // rent payments
        b.u32(0); // count
        let Event::HouseData(h) = ge_event(event::HOUSE_DATA, &b.out) else {
            panic!("expected HouseData");
        };
        assert_eq!(h.dwelling_id, 0x0028_0400);
        assert_eq!(h.owner_id, 0x0028_0001);
        assert_eq!(h.house_type, 3);
        assert!(h.maintenance_free);
        assert_eq!(h.owner_name, "Rurik");
        assert_eq!(h.buy.len(), 1);
        assert_eq!(h.buy[0].num, 100);
        assert_eq!(h.buy[0].paid, 50);
        assert_eq!(h.buy[0].name, "pyreal");
        assert!(h.rent.is_empty());
    }

    #[test]
    fn fellowship_full_update_event() {
        let mut b = Writer::new();
        b.u16(1); // count
        b.u16(16); // buckets
        // one fellow: 44 fixed bytes + name
        b.u32(0x0028_0001); // guid
        b.u32(0); // cpCached
        b.u32(0); // lumCached
        b.u32(10); // level
        b.u32(100); // hMax
        b.u32(50); // sMax
        b.u32(25); // mMax
        b.u32(90); // hCur
        b.u32(45); // sCur
        b.u32(20); // mCur
        b.u32(0); // shareLoot
        b.string16l("Rurik");
        b.string16l("The Fellowship"); // fellowshipName
        b.u32(0x0028_0001); // leaderGuid
        b.u32(1); // shareXP
        b.u32(0); // evenShare
        b.u32(1); // open
        b.u32(0); // locked
        b.u16(0);
        b.u16(32); // departed members (none)
        b.u16(0);
        b.u16(32); // fellowship locks (none)
        let Event::FellowshipFullUpdate(f) = ge_event(event::FELLOWSHIP_FULL_UPDATE, &b.out) else {
            panic!("expected FellowshipFullUpdate");
        };
        assert_eq!(f.name, "The Fellowship");
        assert_eq!(f.leader_guid, 0x0028_0001);
        assert!(f.share_xp);
        assert!(f.open);
        assert!(!f.locked);
        assert_eq!(f.fellows.len(), 1);
        assert_eq!(f.fellows[0].level, 10);
        assert_eq!(f.fellows[0].health, (100, 90));
        assert_eq!(f.fellows[0].stamina, (50, 45));
        assert_eq!(f.fellows[0].mana, (25, 20));
    }

    #[test]
    fn allegiance_update_event() {
        let mut b = Writer::new();
        b.u32(5); // rank
        b.u32(10); // totalMembers
        b.u32(4); // totalVassals
        b.u16(1); // recordCount
        b.u16(0xB); // oldVersion
        b.u16(0); // officers count
        b.u16(256); // officers buckets
        b.u32(0); // titles count
        b.u32(0); // monarchBroadcastTime
        b.u32(0); // monarchBroadcastsToday
        b.u32(0); // spokesBroadcastTime
        b.u32(0); // spokesBroadcastsToday
        b.string16l(""); // motd
        b.string16l(""); // motdSetBy
        b.u32(0x100); // chatRoom
        b.u32(0x0000_1000); // bindpoint cell
        for _ in 0..7 {
            b.f32(0.0); // Pos + Rotation
        }
        b.string16l("House of Tavi"); // allegianceName
        b.u32(0); // nameLastSetTime
        b.u32(0); // locked
        b.i32(0); // approvedVassal
        // monarch record
        b.u32(0x0028_0001); // guid
        b.u32(0); // cpCached
        b.u32(0); // cpTithed
        b.u32(0x1); // bitfield (LOGGEDIN)
        b.u8(1); // gender
        b.u8(0); // heritage
        b.u16(5); // rank
        b.u16(50); // loyalty
        b.u16(60); // leadership
        b.u32(0);
        b.u32(0); // 8 bytes (no age bit)
        b.string16l("Monarch Tavi");
        let Event::AllegianceUpdate(a) = ge_event(event::ALLEGIANCE_UPDATE, &b.out) else {
            panic!("expected AllegianceUpdate");
        };
        assert_eq!(a.rank, 5);
        assert_eq!(a.total_members, 10);
        assert_eq!(a.total_vassals, 4);
        assert_eq!(a.name, "House of Tavi");
        assert_eq!(a.chat_room, 0x100);
        assert!(!a.locked);
        assert_eq!(a.members.len(), 1);
        assert_eq!(a.members[0].name, "Monarch Tavi");
        assert_eq!(a.members[0].rank, 5);
        assert!(a.members[0].online);
    }

    #[test]
    fn object_create_door_motion() {
        let mut w = Writer::new();
        w.u32(opcode::OBJECT_CREATE);
        w.u32(0x0028_0001); // guid
        // ModelData
        w.u8(0x11); // marker
        w.u8(0); // nsub
        w.u8(0); // ntex
        w.u8(1); // nanim
        w.u8(0); // part index
        w.u16(0x1234); // packed model
        w.align4();
        // PhysicsData
        w.u32(0x010000); // flags (Movement)
        w.u32(0); // physicsState
        w.u32(10); // movement section length
        // movement section: mt=0, mflags=0, style, f=0x2, forward=On
        w.u8(0);
        w.u8(0);
        w.u16(0);
        w.u32(0x2);
        w.u16(0x000b);
        w.u32(1); // isAutonomous
        for _ in 0..9 {
            w.u16(0); // sequence timestamps (always written)
        }
        w.align4(); // server aligns before the object desc tail
        w.u32(0); // weenieFlags
        w.string16l("Gate");
        w.u16(0); // WeenieClassId
        w.u16(0); // IconId
        w.u32(0x10); // itemType
        w.u32(0); // ODF
        match parse_incoming(&w.out).expect("parse") {
            Incoming::ObjectCreate(oc) => {
                assert_eq!(oc.guid, 0x0028_0001);
                assert_eq!(oc.name.as_deref(), Some("Gate"));
                assert_eq!(oc.item_type, 0x10);
                assert_eq!(oc.motion_cmd, 0x4000_000B); // DOOR_ON
                assert_eq!(oc.model_parts, Some(vec![(0, 0x0100_1234)]));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn player_description_tail() {
        let mut b = Writer::new();
        b.u32(0x0001); // propertyFlags (Int32)
        b.u32(0x2); // weenieType
        // Int table: encumbrance, coinValue
        b.u16(2);
        b.u16(4);
        b.u32(5);
        b.i32(100);
        b.u32(20);
        b.i32(500);
        // vectorFlags (Attr|Skill|Spell) + hasHealth
        b.u32(0x0103);
        b.u32(1);
        // Attribute block: all 6 attrs + 3 vitals
        b.u32(0x1FF); // attrFlags
        // strength: ranks 5, starting 30, xp 0
        b.u32(5);
        b.u32(30);
        b.u32(0);
        for _ in 0..5 {
            b.u32(0);
            b.u32(10);
            b.u32(0);
        }
        for i in 0..3u32 {
            b.u32(0); // ranks
            b.u32(100 + i); // starting
            b.u32(0); // xp
            b.u32(100 + i); // current
        }
        // Skill table: 2 entries (32 bytes each)
        b.u16(2);
        b.u16(4);
        b.u32(1); // Axe
        b.u16(10);
        b.u16(1);
        b.u32(1);
        b.u32(0);
        b.u32(1);
        b.u32(0);
        b.f64(0.0);
        b.u32(11); // Sword
        b.u16(5);
        b.u16(1);
        b.u32(1);
        b.u32(0);
        b.u32(1);
        b.u32(0);
        b.f64(0.0);
        // Spell table: 1 entry (8 bytes)
        b.u16(1);
        b.u16(2);
        b.u32(10);
        b.f32(2.0);
        // optionFlags (SpellLists8) + characterOptions1
        b.u32(0x0400);
        b.u32(0);
        // SpellLists8: 8 tabs
        b.u32(1);
        b.u32(10);
        for _ in 0..7 {
            b.u32(0);
        }
        b.u32(0x40); // spellFilters
        // inventory: 1 item
        b.u32(1);
        b.u32(0x0028_0001);
        b.u32(0);
        // equipped: 1 item
        b.u32(1);
        b.u32(0x0028_0002);
        b.u32(0x400);
        b.u32(0);
        let Event::PlayerDescription(pd) = ge_event(event::PLAYER_DESCRIPTION, &b.out) else {
            panic!("expected PlayerDescription");
        };
        assert_eq!(pd.coin_value, 500);
        assert_eq!(pd.encumbrance, 100);
        assert_eq!(pd.strength, 30);
        assert_eq!(pd.skills, vec![(1, 10), (11, 5)]);
        assert_eq!(pd.spells, vec![10]);
        assert_eq!(pd.spell_bar.len(), 8);
        assert_eq!(pd.spell_bar[0], vec![10]);
        assert_eq!(pd.spell_filters, vec![0x40]);
        assert_eq!(pd.inventory, vec![(0x0028_0001, 0)]);
        assert_eq!(pd.equipped, vec![(0x0028_0002, 0x400, 0)]);
    }

    #[test]
    fn appraise_profiles() {
        let mut b = Writer::new();
        b.u32(0x0028_0042); // target guid
        b.u32(0x40A9); // Int|String|Weapon|Armor|ArmorLevels
        b.u32(1); // success
        // Int table
        b.u16(2);
        b.u16(4);
        b.u32(25);
        b.i32(5);
        b.u32(19);
        b.i32(100);
        // String table
        b.u16(1);
        b.u16(2);
        b.u32(16);
        b.string16l("A fine sword.");
        // Armor profile (0x80): 8 f32
        for i in 0..8u32 {
            b.f32(i as f32);
        }
        // Weapon profile (0x20)
        b.u32(1); // damageType
        b.u32(10); // weaponTime
        b.u32(11); // weaponSkill
        b.u32(25); // damage
        b.f64(10.0);
        b.f64(1.0);
        b.f64(3.0);
        b.f64(100.0);
        b.f64(1.5);
        b.u32(0);
        // ArmorLevels (0x4000): 9 u32
        for i in 0..9u32 {
            b.u32(i);
        }
        let Event::Appraise(a) = ge_event(event::IDENTIFY_OBJECT_RESPONSE, &b.out) else {
            panic!("expected Appraise");
        };
        assert_eq!(a.guid, 0x0028_0042);
        assert_eq!(a.level, Some(5));
        assert_eq!(a.value, Some(100));
        assert_eq!(a.description, "A fine sword.");
        assert_eq!(a.armor.unwrap().resist, [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        assert_eq!(
            a.weapon.unwrap(),
            AppraiseWeapon { damage_min: 25, damage_var: 10, skill: 11 }
        );
        assert_eq!(a.armor_levels.unwrap(), [0, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn new_action_bodies() {
        // Casting.
        let b = body_cast_untargeted_spell(230);
        assert_eq!(b, vec![230, 0, 0, 0]);
        let b = body_cast_targeted_spell(0x0011_0042, 230);
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), 0x0011_0042);
        assert_eq!(r.u32(), 230);
        // Allegiance.
        let b = body_swear_allegiance(0x0011_0001);
        assert_eq!(b, vec![1, 0, 0x11, 0]);
        let b = body_break_allegiance(0x0011_0001);
        assert_eq!(b, vec![1, 0, 0x11, 0]);
        // Friends.
        let b = body_add_friend("Rurik");
        let mut r = Reader::new(&b);
        assert_eq!(r.string16l(), "Rurik");
        assert_eq!(r.remaining(), 0);
        let b = body_remove_friend(0x0011_0002);
        assert_eq!(b, vec![2, 0, 0x11, 0]);
        // Fellowship.
        let b = body_fellowship_create("The Guild", true);
        let mut r = Reader::new(&b);
        assert_eq!(r.string16l(), "The Guild");
        assert_eq!(r.u32(), 1);
        let b = body_fellowship_quit(true);
        assert_eq!(b, vec![1, 0, 0, 0]);
        let b = body_fellowship_dismiss(0x0011_0003);
        assert_eq!(b, vec![3, 0, 0x11, 0]);
        let b = body_fellowship_recruit(0x0011_0004);
        assert_eq!(b, vec![4, 0, 0x11, 0]);
        let b = body_fellowship_new_leader(0x0011_0005);
        assert_eq!(b, vec![5, 0, 0x11, 0]);
        let b = body_fellowship_openness(true);
        assert_eq!(b, vec![1, 0, 0, 0]);
        // Books.
        let b = body_book_data(0x0028_0001);
        assert_eq!(b, vec![1, 0, 0x28, 0]);
        let b = body_book_modify_page(0x0028_0001, 2, "hello");
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), 0x0028_0001);
        assert_eq!(r.i32(), 2);
        assert_eq!(r.string16l(), "hello");
        let b = body_book_add_page(0x0028_0001);
        assert_eq!(b, vec![1, 0, 0x28, 0]);
        let b = body_book_delete_page(0x0028_0001, 1);
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), 0x0028_0001);
        assert_eq!(r.i32(), 1);
        let b = body_book_page_data(0x0028_0001, 0);
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), 0x0028_0001);
        assert_eq!(r.i32(), 0);
        // Spell favorites.
        let b = body_add_spell_favorite(230, 3, 1);
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), 230);
        assert_eq!(r.u32(), 3);
        assert_eq!(r.u32(), 1);
        let b = body_remove_spell_favorite(230, 1);
        let mut r = Reader::new(&b);
        assert_eq!(r.u32(), 230);
        assert_eq!(r.u32(), 1);
        // Emotes / remove spell / ping.
        let b = body_emote("waves");
        let mut r = Reader::new(&b);
        assert_eq!(r.string16l(), "waves");
        let b = body_soul_emote("stands");
        let mut r = Reader::new(&b);
        assert_eq!(r.string16l(), "stands");
        assert_eq!(body_remove_spell(230), vec![230, 0, 0, 0]);
        assert!(body_ping_request().is_empty());
    }

    #[test]
    fn magic_spell_events() {
        let mut b = Writer::new();
        b.u16(230); // spellId
        b.u16(1); // layer
        let Event::MagicUpdateSpell { spell_id, layer } = ge_event(event::MAGIC_UPDATE_SPELL, &b.out) else {
            panic!("expected MagicUpdateSpell");
        };
        assert_eq!(spell_id, 230);
        assert_eq!(layer, 1);

        let Event::MagicRemoveSpell { spell_id, layer } = ge_event(event::MAGIC_REMOVE_SPELL, &b.out) else {
            panic!("expected MagicRemoveSpell");
        };
        assert_eq!(spell_id, 230);
        assert_eq!(layer, 1);
    }

    #[test]
    fn book_data_response_event() {
        let mut b = Writer::new();
        b.u32(0x0028_0001); // bookID
        b.u32(10); // maxPages
        b.u32(10); // numPages
        b.u32(4000); // maxChars
        b.u32(2); // pageCount
        // page 0
        b.u32(0x0011_0001); // authorId
        b.string16l("Rurik");
        b.string16l("rurik_ac");
        b.u32(0xFFFF_0002);
        b.u32(0); // textIncluded
        // page 1
        b.u32(0x0011_0002);
        b.string16l("Mira");
        b.string16l("mira_ac");
        b.u32(0xFFFF_0002);
        b.u32(0);
        b.string16l("A story book"); // inscription
        b.u32(0x0011_0001); // authorId
        b.string16l("Rurik"); // author
        let Event::BookDataResponse {
            book_guid,
            max_pages,
            max_chars,
            pages,
            inscription,
            author,
        } = ge_event(event::BOOK_DATA_RESPONSE, &b.out)
        else {
            panic!("expected BookDataResponse");
        };
        assert_eq!(book_guid, 0x0028_0001);
        assert_eq!(max_pages, 10);
        assert_eq!(max_chars, 4000);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].author, "Rurik");
        assert_eq!(pages[1].author, "Mira");
        assert_eq!(inscription, "A story book");
        assert_eq!(author, "Rurik");
    }

    #[test]
    fn book_page_data_response_event() {
        let mut b = Writer::new();
        b.u32(0x0028_0001);
        b.u32(3); // pageIndex
        b.u32(0x0011_0001); // authorId
        b.string16l("Rurik");
        b.string16l("rurik_ac");
        b.u32(0xFFFF_0002);
        b.u32(1); // textIncluded
        b.u32(0); // ignoreAuthor
        b.string16l("Once upon a time");
        let Event::BookPageDataResponse { book_guid, page, author, text } =
            ge_event(event::BOOK_PAGE_DATA_RESPONSE, &b.out)
        else {
            panic!("expected BookPageDataResponse");
        };
        assert_eq!(book_guid, 0x0028_0001);
        assert_eq!(page, 3);
        assert_eq!(author, "Rurik");
        assert_eq!(text, "Once upon a time");
    }

    #[test]
    fn book_mutation_events() {
        let mut b = Writer::new();
        b.u32(0x0028_0001);
        b.u32(2);
        b.u32(1); // success
        let Event::BookMutationResponse { book_guid, page, success } =
            ge_event(event::BOOK_MODIFY_PAGE_RESPONSE, &b.out)
        else {
            panic!("expected BookMutationResponse");
        };
        assert_eq!(book_guid, 0x0028_0001);
        assert_eq!(page, 2);
        assert!(success);

        let Event::BookMutationResponse { book_guid, page, success } =
            ge_event(event::BOOK_ADD_PAGE_RESPONSE, &b.out)
        else {
            panic!("expected BookMutationResponse");
        };
        assert_eq!(book_guid, 0x0028_0001);
        assert_eq!(page, 2);
        assert!(success);

        let Event::BookMutationResponse { book_guid, page, success } =
            ge_event(event::BOOK_DELETE_PAGE_RESPONSE, &b.out)
        else {
            panic!("expected BookMutationResponse");
        };
        assert_eq!(book_guid, 0x0028_0001);
        assert_eq!(page, 2);
        assert!(success);
    }

    #[test]
    fn pd_enchantment_registry_vitae() {
        let mut b = Writer::new();
        b.u32(0x0001); // propertyFlags (Int32)
        b.u32(0x2); // weenieType
        // Int table: encumbrance, coinValue
        b.u16(2);
        b.u16(4);
        b.u32(5);
        b.i32(100);
        b.u32(20);
        b.i32(500);
        // vectorFlags (Attr|Skill|Spell|Enchant) + hasHealth
        b.u32(0x0303);
        b.u32(1);
        // Attribute block: all 6 attrs + 3 vitals
        b.u32(0x1FF);
        b.u32(5);
        b.u32(30);
        b.u32(0);
        for _ in 0..5 {
            b.u32(0);
            b.u32(10);
            b.u32(0);
        }
        for i in 0..3u32 {
            b.u32(0);
            b.u32(100 + i);
            b.u32(0);
            b.u32(100 + i);
        }
        // Skill table: 1 entry
        b.u16(1);
        b.u16(4);
        b.u32(1);
        b.u16(10);
        b.u16(1);
        b.u32(1);
        b.u32(0);
        b.u32(1);
        b.u32(0);
        b.f64(0.0);
        // Spell table: 1 entry
        b.u16(1);
        b.u16(2);
        b.u32(10);
        b.f32(2.0);
        // Enchantment registry: mask 0x4 (Vitae only)
        b.u32(0x4);
        // One 60-byte enchantment (Vitae, spell 666)
        b.u16(666); // spellId
        b.u16(0); // layer
        b.u16(0); // spellCategory
        b.u16(0); // hasSpellSetId
        b.u32(0); // powerLevel
        b.f64(0.0); // startTime
        b.f64(0.0); // duration
        b.u32(0); // casterGuid
        b.f32(0.0); // degradeModifier
        b.f32(1.0); // degradeLimit
        b.f64(0.0); // lastTimeDegraded
        b.u32(0); // statModType
        b.u32(36); // statModKey
        b.f32(0.5); // statModValue (death penalty multiplier)
        // optionFlags (SpellLists8) + characterOptions1
        b.u32(0x0400);
        b.u32(0);
        // SpellLists8: 8 tabs (all empty)
        for _ in 0..8 {
            b.u32(0);
        }
        b.u32(0x40); // spellFilters
        // inventory: 0, equipped: 0
        b.u32(0);
        b.u32(0);
        let Event::PlayerDescription(pd) = ge_event(event::PLAYER_DESCRIPTION, &b.out) else {
            panic!("expected PlayerDescription");
        };
        assert_eq!(pd.enchantments.len(), 1);
        assert_eq!(pd.enchantments[0].spell_id, 666);
        assert!((pd.enchantments[0].stat_mod_value - 0.5).abs() < f32::EPSILON);
    }
}
