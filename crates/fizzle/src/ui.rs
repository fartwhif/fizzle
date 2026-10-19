//! TUI rendering (ratatui) + the in-world mouse model.
//!
//! The in-world UI is a port of the acclient-ts reference: a 14-widget
//! layout (vitals / world pane / chat on the left; radar, multi-widget box,
//! glyph selector and spell/hotkey bars on the right), an Esc menu with
//! themes / widgets / world-view / actor-glow / help subviews, a 3D world
//! renderer (acgeo) with click-to-target picking and a top-down tactical
//! map, the vendor / trade / loot widget boxes, the inventory paper-doll,
//! the Dereth continent map, and the full mouse model (left click,
//! double-click, right-click, scroll wheel, drag-and-drop).
//!
//! All screen text goes through [`put`], which clips to the frame; every
//! style is remapped through the active theme. No network I/O happens here:
//! mouse handlers only mutate `App` state and call its public action
//! methods, which own the protocol traffic.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::app::{App, MenuView, Phase};
use crate::themes::Theme;
use acproto::messages::combat_mode;
use crate::world::{
    ChatStyle, Entity, Inventory, ItemState, Target, VendorItem, VendorState,
    MAIN_PACK_CAPACITY, ITEM_TYPE_CONTAINER, ITEM_TYPE_CREATURE, ITEM_TYPE_DOOR,
    TARGET_ITEM_CONTAINER, TARGET_ITEM_PACK, TARGET_ITEM_SHOP, TARGET_ITEM_TRADE,
};
use acgeo::{Actor, Camera, ReticleTarget, get_dereth_land, wrap_block_coord, ACTOR_SCALE};

/// True if the single unified target is the item `guid` (in any pack / shop /
/// trade / opened container).
fn item_is_targeted(app: &App, guid: u32) -> bool {
    match app.target() {
        Some(Target::Item(g, _)) => g == guid,
        _ => false,
    }
}

/// The targeted item's guid, if the current target is an item.
fn targeted_item(app: &App) -> Option<u32> {
    match app.target() {
        Some(Target::Item(g, _)) => Some(g),
        _ => None,
    }
}

// ================================================================== constants

/// The ten spell/hotkey slot glyphs (1-9 then 0).
const SLOTS: [char; 10] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];
/// The inventory multi-widget index (paper doll + pack grid).
const INV_WIDGET: usize = 11;
/// The examine multi-widget index (right-click switches to it).
const EXAMINE_WIDGET: usize = 13;
/// Paper-doll rows (0..4) at the top of the inventory widget.
const DOLL_ROWS: u16 = 5;
/// First pack-grid row inside the inventory widget (header on row 5).
const INV_GRID_R0: u16 = 6;
/// The vendor list row's page size.
const VENDOR_LIST_PAGE: usize = 8;
/// A double-click: a second click on the same cell within this window.
const DOUBLE_CLICK: Duration = Duration::from_millis(450);
/// How far (screen cells) a 3D click may sit from the edge of a projected
/// body box and still count as a hit (a near body is easy to click; a thin
/// far one only on its own dot).
const PICK_PAD: f32 = 1.0;
/// The merchant "key" item-type flag (a dropped key is used, not stored).
const ITEM_TYPE_KEY: u32 = 0x4000;
/// Ground item (corpse / dropped container) item type.
const ITEM_TYPE_GROUND: u32 = 32;
/// POI markers are not plotted on the radar.
const ITEM_TYPE_POI: u32 = 8192;
/// Retail motion command "Ready" (the idle stance).
const MCMD_READY: u32 = 0x41000003;
/// Motion-table style for non-player bodies (creature style 0x3d).
const STYLE_CREATURE: u32 = 0x3d;

/// The 3D camera framing constants (acgeo `render` uses the same values).
const CAM_BACK: f32 = 13.0;
const CAM_UP: f32 = 6.5;
const CAM_LOOK_UP: f32 = 1.5;
const CAM_FOCAL_K: f32 = 1.0;
const CAM_NEAR: f32 = 0.5;

/// The 3D actor glow-halo colors (self = gold, players = cyan, creatures = orange).
const GLOW_SELF: u32 = 0xFFD24A;
const GLOW_PLAYER: u32 = 0x8CD0FF;
const GLOW_CREATURE: u32 = 0xFF965A;

/// The 14 multi widgets (right column), in selector order.
const WIDGET_LABELS: [&str; 14] = [
    "Allegiance", "Fellowship", "Friends", "Squelch", "Spellbook", "Attributes", "Skills",
    "Titles", "Contracts", "Map", "House", "Inventory", "You", "Examine",
];
/// One-row widget selector: a Nerd Font `nf-md-*` (Material Design Icon)
/// glyph per widget, in `WIDGET_LABELS` order. Code points are the
/// `@mdi/font` values (Nerd Fonts v3 places MDI at 0xF0001-0xF1AF0); each
/// measures as 1 cell so the row stays aligned.
const WIDGET_GLYPHS: [char; 14] = [
    '\u{F023B}', // 0  Allegiance   flag
    '\u{F000E}', // 1  Fellowship   account-multiple
    '\u{F0899}', // 2  Friends      account-heart
    '\u{F036D}', // 3  Squelch      microphone-off
    '\u{F14F7}', // 4  Spellbook    book-open-variant
    '\u{F0127}', // 5  Attributes   chart-areaspline
    '\u{F0124}', // 6  Skills       certificate
    '\u{F0987}', // 7  Titles       medal
    '\u{F0219}', // 8  Contracts    file-document
    '\u{F034D}', // 9  Map          map
    '\u{F02DC}', // 10 House        home
    '\u{F0D2E}', // 11 Inventory    sack
    '\u{F0004}', // 12 You          account
    '\u{F0349}', // 13 Examine      magnify
];

/// The paper-doll slots: (id, name, row, col, EquipMask bits). Structural
/// blanks (no entry) stay plain spaces.
const DOLL_SLOTS: [(char, &str, u16, u16, u32); 24] = [
    ('N', "neck", 0, 0, 0x8000),
    ('H', "head", 0, 2, 0x1),
    ('A', "aetheria 1", 0, 3, 0x10000000),
    ('B', "aetheria 2", 0, 4, 0x20000000),
    ('C', "aetheria 3", 0, 5, 0x40000000),
    ('T', "trinket", 1, 0, 0x4000000),
    ('U', "upper arm armor", 1, 1, 0x800),
    ('V', "chest armor", 1, 2, 0x200),
    ('K', "cloak", 1, 5, 0x8000000),
    ('W', "wrist left", 2, 0, 0x10000),
    ('X', "lower arm armor", 2, 1, 0x1000),
    ('Y', "abdomen armor", 2, 2, 0x400),
    ('P', "upper leg armor", 2, 3, 0x2000),
    ('R', "wrist right", 2, 4, 0x20000),
    ('M', "shirt", 2, 5, 0x2 | 0x8 | 0x10),
    ('F', "ring left", 3, 0, 0x40000),
    ('G', "hands", 3, 1, 0x20),
    ('Q', "lower leg armor", 3, 3, 0x4000),
    ('D', "ring right", 3, 4, 0x80000),
    ('L', "pants", 3, 5, 0x4 | 0x40 | 0x80),
    ('S', "shield", 4, 0, 0x200000),
    ('E', "feet", 4, 3, 0x100),
    ('Z', "weapon", 4, 4, 0x100000 | 0x400000 | 0x1000000 | 0x2000000),
    ('O', "ammo", 4, 5, 0x800000),
];

/// EquipMask location bits and their retail names, in retail order.
const LOC_BITS: [u32; 31] = [
    0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x40, 0x80, 0x100, 0x200, 0x400, 0x800, 0x1000, 0x2000, 0x4000,
    0x8000, 0x10000, 0x20000, 0x40000, 0x80000, 0x100000, 0x200000, 0x400000, 0x800000,
    0x1000000, 0x2000000, 0x4000000, 0x8000000, 0x10000000, 0x20000000, 0x40000000,
];
const LOC_NAMES: [&str; 31] = [
    "head", "chest", "abdomen", "uparm", "lowarm", "hand", "upleg", "lowleg", "foot", "chest",
    "abdomen", "uparm", "lowarm", "upleg", "lowleg", "neck", "wristL", "wristR", "ringL",
    "ringR", "weapon", "shield", "missile", "ammo", "held", "2hand", "trinket", "cloak",
    "aetheria1", "aetheria2", "aetheria3",
];

/// Item-type flags → (glyph, name), checked in order (first flag that
/// matches wins). The glyph is a Nerd Font `nf-md-*` (Material Design Icon)
/// code point — the `@mdi/font` value, since Nerd Fonts v3 places MDI at
/// 0xF0001-0xF1AF0. Each measures as 1 cell, so pack/vendor/loot grids and
/// the paper doll stay aligned. (Icons need a Nerd Font terminal; without
/// one they render as empty/tofu boxes.)
const ITEM_TYPES: [(u32, char, &str); 31] = [
    (0x1, '\u{F04E5}', "melee"),          // sword
    (0x100, '\u{F1841}', "missile"),      // bow-arrow
    (0x2, '\u{F0498}', "armor"),          // shield
    (0x4, '\u{F0A7C}', "clothing"),       // tshirt-v
    (0x8, '\u{F0F0B}', "jewelry"),        // necklace
    (0x10, '\u{F03E9}', "creature"),      // paw
    (0x20, '\u{F025B}', "food"),          // food-apple
    (0x40, '\u{F0114}', "money"),         // cash
    (0x80, '\u{F00D4}', "misc"),          // box
    (0x200, '\u{F003C}', "container"),    // archive
    (0x400, '\u{F02D7}', "useless"),      // help-circle
    (0x800, '\u{F01C8}', "gem"),          // diamond-stone
    (0x1000, '\u{F0096}', "spellcomp"),   // flask-outline
    (0x2000, '\u{F00BA}', "writable"),    // book
    (0x4000, '\u{F0306}', "key"),         // key
    (0x8000, '\u{F1844}', "caster"),      // magic-staff
    (0x10000, '\u{F081A}', "portal"),     // door
    (0x20000, '\u{F033E}', "lockable"),   // lock
    (0x40000, '\u{F039E}', "note"),       // note-text
    (0x80000, '\u{F0B2F}', "manastone"),  // crystal-ball
    (0x100000, '\u{F10F1}', "service"),   // hand-heart
    (0x200000, '\u{F04CE}', "magic"),     // star
    (0x400000, '\u{F02E5}', "cook"),      // pot
    (0x800000, '\u{F0093}', "alchemy"),   // flask
    (0x2000000, '\u{F06D3}', "fletch"),   // feather
    (0x4000000, '\u{F1242}', "alchemy+"), // flask-plus
    (0x8000000, '\u{F06D3}', "fletch+"),  // feather (higher tier)
    (0x10000000, '\u{F02D1}', "lifestone"),  // heart
    (0x20000000, '\u{F05B7}', "tool"),         // wrench
    (0x40000000, '\u{F01A6}', "material"),     // cube
    (0x80000000, '\u{F01CE}', "gameboard"),    // dice-5
];

/// The four world themes, in `Theme::ALL` order.
const THEME_LABELS: [&str; 4] = [
    "ascii borders, classic colors",
    "unicode box borders, classic colors",
    "ascii borders, no color (bold/inverse only)",
    "unicode borders, all green",
];

/// The two world views (0 = tactical, 1 = 3d).
const WORLD_VIEWS: [&str; 2] = [
    "tactical: north-up top-down tactical map",
    "3d: 3D world view (realtime, retail dats)",
];

/// The Esc menu's top-level items.
const MENU_ITEMS: [&str; 8] = [
    "Themes (borders + colors)",
    "Widgets (right-column multi box)",
    "World view (world pane render mode)",
    "Actor glow (3d halo around characters)",
    "Target reticle (3d: animate the target box)",
    "Help (key map)",
    "Log out of the world",
    "Quit",
];

/// The help screen's key map (fizzle: retail F-key panel toggles, but no
/// camera numpad, no server/account form, no plugin toggles).
const HELP_LINES: [&str; 81] = [
    "World keys (the definitive AC key map; the Esc menu aside):",
    "Movement (client-side sim; the server corrects if it disagrees):",
    "  W / up arrow          forward           X / down arrow       backward",
    "  A / left arrow        turn left         D / right arrow      turn right",
    "  Z / C                 strafe left / right  S                 stop",
    "  space                 jump",
    "  hold several move keys to combine (W+D arc, W+C diagonal)",
    "  (a tap is a small nudge; a held key moves continuously; S stops all movement)",
    "  Shift (any move key)  walk (else the run lock decides run / walk)",
    "  Q                     run lock on / off (default on)",
    "Targeting and objects (the retail key map):",
    "  + / -                 next / prev compass item (creature)",
    "  '                     nearest creature",
    "  L / ;                 prev / next creature",
    "  [ / ]                 prev / next object (door, ground item)",
    "  \\                     nearest object",
    "  , / .                 prev / next person",
    "  /                     nearest person",
    "  F                     pick up the target (ground container; pack item -> main pack)",
    "  R                     use the target (a ground container is picked up;",
    "                        an NPC opens the talk dialog — its lines go out",
    "                        as TalkDirect to it)",
    "  E / e                 examine the target (works from any widget)",
    "  T                     attack the target (melee; combat stance first)",
    "  `                     toggle combat / peace mode (sent to the server)",
    "  (you're in the compass too — cycle + / - to target yourself)",
    "Spells:",
    "  1-9, 0                spell slots 1-10",
    "  Del / PgDn            the spell to the left / the next spell",
    "  End                   start casting",
    "  Ins / PgUp            previous / next spell tab",
    "UI:",
    "  Enter / Tab           open chat (Enter sends; Tab or Esc closes it)",
    "  F1                    this help",
    "  F3 / F4                 allegiance / fellowship panel",
    "  F5 / F8 / F9 / F12      spellbook / attributes / skills / inventory panel",
    "  Esc                   open the menu",
    "  Shift+Esc             log out of the world",
    "  Esc menu -> Widgets   set the right-column multi widget",
    "  Esc menu -> World view  tactical / 3D render mode of the World pane",
    "  Esc menu -> Actor glow  on/off the halo around characters (3d view)",
    "  World pane legend:  @ you  p player  m creature  d door  c chest  . item",
    "  3D mode: the movement keys drive the view (needs the retail dats)",
    "Mouse (click to act; the marker is the pointer):",
    "  left-click            target the actor / item under the cursor",
    "                        (in combat stance, a click on a creature attacks it)",
    "  left double-click     use the target under the cursor (a ground item is",
    "                        picked up, a chest / corpse opens to loot, a player",
    "                        opens a trade)",
    "  scroll wheel          scroll the active list; over the pack grid, scroll",
    "                        its items; over the vendor box, scroll its list;",
    "                        over the 3D view, zoom the camera in / out",
    "  right-click           target + examine the item / object under the cursor",
    "  glyph row             open that multi widget",
    "  Spells / Keys bar     select the slot",
    "  pack icon column      drag an item on the main pack icon to move it",
    "  vendor / trade box    click a list item / glyph to target it; click a",
    "                        tab to switch (‹/› page the list); the box's",
    "                        button row does the actions (see below)",
    "  Chat box              focus chat",
    "  item drag & drop      onto trade, vendor, paper doll (equip), pack,",
    "                        player, ground",
    "  menu / list row       click moves, double-click activates (Enter)",
    "Widgets take no keys (only the menu and its submenus do):",
    "  Inventory: items are mouse-picked — click targets, right-click",
    "    examines, drag to the paper doll equips, to a pack icon moves;",
    "    drop the item on the ground to drop it",
    "  Vendor: tabs Items / Buy / Sell — click a list item to target it,",
    "    ‹/› page it, the button row does the tab's actions (Items: Buy/",
    "    Add, Buy: Buy/Buy All/Remove/Clear, Sell: Add/Sell/Sell All/",
    "    Remove/Clear); [Close] or Esc leaves (with a pending order, the",
    "    second press)",
    "  Trade: click a glyph to target the item; the box's buttons do the",
    "    actions (Add / Accept / Decline / Reset / X); Esc closes",
    "  Loot: click a glyph to target the item; the box's buttons do the",
    "    actions (Take / Take All / X); Esc closes",
    "Chat commands (type in chat, then Enter):",
    "  @t / @tell <name> <text>   tell a player directly (/t, /tell too)",
    "  all other / and @ commands go to the server as-is",
    "Menu / list keys:  j/k move, Enter activate, Esc back.",
    "Themes (Esc menu): j/k + Enter — borders + colors, session-only.",
];

// ================================================================== entry points

/// Render the whole frame for the current phase. Takes `&mut App` because
/// the 3D world renderer needs a mutable borrow of `world3d`.
pub fn draw(f: &mut Frame, app: &mut App) {
    match app.phase {
        Phase::Idle | Phase::Connecting | Phase::Entering => draw_status(f, app),
        Phase::CharSelect => draw_charselect(f, app),
        Phase::CharCreate => draw_charcreate(f, app),
        Phase::InWorld | Phase::LoggingOut => draw_world(f, app),
        Phase::Disconnected => draw_disconnected(f, app),
    }
    if (app.phase == Phase::InWorld || app.phase == Phase::LoggingOut) && app.world.dead {
        draw_center_box(f, app, "YOU ARE DEAD", &["Press Enter to chat, q to quit."], Color::Red);
    }
    if app.phase == Phase::InWorld {
        draw_mouse_marker(f, app);
    }
}

/// Mouse event entry point: the full in-world hit-testing model (left click,
/// double-click, right-click, scroll wheel, drag-and-drop, and the menu
/// subviews' click activation).
pub fn mouse(app: &mut App, area: Rect, ev: &crossterm::event::MouseEvent) {
    use crossterm::event::{MouseEventKind, MouseButton};
    // Always track the pointer (the marker is drawn every frame).
    app.mouse_xy = Some((ev.row, ev.column));
    if app.phase != Phase::InWorld {
        return;
    }
    if app.world.dead {
        return;
    }
    let (r, c) = (ev.row, ev.column);
    if let Some(view) = app.menu_view {
        // The menu subviews replace the world panes: a single click moves the
        // selection, a double click activates (exactly the Enter branches),
        // and the help view scrolls on the wheel.
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                view_press(app, view, r, c, area.height, area.width);
            }
            MouseEventKind::ScrollUp if view == MenuView::Help => {
                app.help_scroll = (app.help_scroll as isize - 1).max(0) as usize;
            }
            MouseEventKind::ScrollDown if view == MenuView::Help => {
                app.help_scroll = (app.help_scroll as isize + 1).min(HELP_LINES.len() as isize - 1) as usize;
            }
            _ => {}
        }
        return;
    }
    match ev.kind {
        MouseEventKind::Down(btn) => match btn {
            MouseButton::Left => left_down(app, area, r, c),
            MouseButton::Right => right_down(app, area, r, c),
            _ => {}
        },
        MouseEventKind::Up(MouseButton::Left) => {
            left_up(app, area, r, c);
        }
        MouseEventKind::ScrollUp => wheel(app, area, r, c, true),
        MouseEventKind::ScrollDown => wheel(app, area, r, c, false),
        _ => {}
    }
}

// ================================================================== style helpers

/// Remap a style through the active theme.
fn ts(app: &App, s: Style) -> Style {
    app.theme.style(s)
}
fn normal(app: &App) -> Style {
    ts(app, Style::default())
}
fn focus(app: &App) -> Style {
    ts(app, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
}
fn reverse(app: &App) -> Style {
    ts(app, Style::default().add_modifier(Modifier::REVERSED))
}
fn yellow(app: &App) -> Style {
    ts(app, Style::default().fg(Color::Yellow))
}
fn dark(app: &App) -> Style {
    ts(app, Style::default().fg(Color::DarkGray))
}

/// Write `text` at (row, col) with `style`; ratatui clips to the frame.
fn put(f: &mut Frame, row: u16, col: u16, text: &str, style: Style) {
    if !text.is_empty() {
        f.buffer_mut().set_string(col, row, text, style);
    }
}

/// Truncate a string to `n` bytes on a char boundary (all UI text here is
/// ASCII, but item names can carry UTF-8 — never panic on a boundary).
fn trunc(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut i = n;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// Draw a bordered box (border only) from (x0, y0) to (x1, y1) inclusive of
/// both edges; the focused (yellow) border marks an active box.
fn draw_box(f: &mut Frame, app: &App, x0: u16, y0: u16, x1: u16, y1: u16, title: &str, focused: bool) {
    let rect = Rect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0));
    let mut b = Block::default()
        .borders(Borders::ALL)
        .border_set(app.theme.border_set())
        .title(title);
    if focused {
        b = b.style(ts(app, Style::default().fg(Color::Yellow)));
    }
    f.render_widget(Paragraph::new("").block(b), rect);
}

// ================================================================== world layout

/// The world view's pane geometry (shared by the draw pass and the mouse
/// hit-testing). `None` when the terminal is too small for the layout.
#[derive(Clone, Copy)]
struct WLayout {
    body_h: u16,
    left_w: u16,
    right_w: u16,
    right_x: u16,
    radar_h: u16,
    multi_y1: u16,
    world_y0: u16,
    world_y1: u16,
    widget_y0: u16,
    widget_y1: u16,
    chat_y0: u16,
}

fn world_layout(h: u16, wcols: u16, widget_active: bool) -> Option<WLayout> {
    let body_h = h - 1;
    if body_h < 22 || wcols < 50 {
        return None;
    }
    let right_w = 16.max(wcols / 3);
    let left_w = 20.max(wcols - right_w - 1);
    let right_x = left_w + 1;
    let radar_h = 9.max(h / 4);
    let key_y0 = body_h - 2;
    let widget_h = u16::from(widget_active) * 8;
    let world_y0 = 3;
    let world_h = 6.max(40.min(body_h - 3 - widget_h - 5));
    let world_y1 = world_y0 + world_h;
    let widget_y0 = world_y1;
    let widget_y1 = widget_y0 + widget_h;
    Some(WLayout {
        body_h,
        left_w,
        right_w,
        right_x,
        radar_h,
        multi_y1: key_y0 - 1,
        world_y0,
        world_y1,
        widget_y0,
        widget_y1,
        chat_y0: widget_y1,
    })
}

fn widget_active(app: &App) -> bool {
    app.world.vendor.guid != 0 || app.world.trade.partner != 0 || app.world.container.guid != 0
}

// ================================================================== world screen

fn draw_world(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // The Esc menu (and subviews) replace the world panes; the status bar
    // stays underneath.
    if let Some(view) = app.menu_view {
        draw_menu_view(f, app, area, view);
        draw_status_bar(f, app, area.width);
        return;
    }
    let Some(lb) = world_layout(area.height, area.width, widget_active(app)) else {
        put(f, 1, 1, "(terminal too small)", ts(app, Style::default().fg(Color::Red)));
        return;
    };
    let key_y0 = lb.body_h - 2;
    // Left column: Vitals (3 rows), World, the vendor / trade / loot widget
    // (8 rows, only while one is open), and Chat fills the rest.
    draw_box(f, app, 0, 0, lb.left_w, 3, "Vitals", false);
    draw_vitals(f, app, lb);
    draw_box(
        f,
        app,
        0,
        lb.world_y0,
        lb.left_w,
        lb.world_y1,
        if app.world_view { "3d" } else { "tactical" },
        false,
    );
    if app.world_view {
        draw_world3d(f, app, lb);
    } else {
        draw_tactical(f, app, lb);
    }
    // An open book overlays the world pane (modal reading/editing).
    if app.world.book.as_ref().is_some_and(|b| b.open) {
        draw_book(f, app, lb);
    }
    if app.world.vendor.guid != 0 {
        draw_box(
            f,
            app,
            0,
            lb.widget_y0,
            lb.left_w,
            lb.widget_y1,
            &format!("Vendor: {}", vendor_name(app)),
            true,
        );
        draw_vendor(f, app, lb);
    } else if app.world.trade.partner != 0 {
        draw_box(
            f,
            app,
            0,
            lb.widget_y0,
            lb.left_w,
            lb.widget_y1,
            &format!("Trade: {}", app.world.trade.partner_name),
            true,
        );
        draw_trade(f, app, lb);
    } else if app.world.container.guid != 0 {
        draw_box(
            f,
            app,
            0,
            lb.widget_y0,
            lb.left_w,
            lb.widget_y1,
            &format!("Loot: {}", app.world.container.name),
            true,
        );
        draw_loot(f, app, lb);
    }
    draw_box(f, app, 0, lb.chat_y0, lb.left_w, lb.body_h, "Chat", false);
    draw_chat(f, app, lb);
    // Right column: the borderless Radar, the Multi box, the one-row glyph
    // selector (on the multi box's bottom border row), and the slot bars.
    draw_radar(f, app, lb, area.width);
    draw_box(
        f,
        app,
        lb.right_x,
        lb.radar_h,
        area.width,
        lb.multi_y1,
        WIDGET_LABELS[app.multi_sel.min(13)],
        false,
    );
    draw_multi(f, app, lb, area.width);
    draw_selector(f, app, lb, area.width);
    {
        let tab = app.world.skills.tab;
        let spell_vals: Vec<char> = (0..10).map(|i| {
            let has = app
                .world
                .skills
                .spell_bar
                .get(tab)
                .and_then(|t| t.get(i))
                .copied()
                .unwrap_or(0)
                != 0;
            if has { SLOTS[i] } else { '-' }
        }).collect();
        draw_slot_bar(f, app, key_y0, lb, "Spells:", app.world.skills.slot, &spell_vals);
        draw_slot_bar(f, app, key_y0 + 1, lb, "Keys:", app.hotkey_sel, &SLOTS);
    }
    // The solid reverse status bar on the bottom row.
    draw_status_bar(f, app, area.width);
}

/// The open book (overlaying the world pane): the current page text (or the
/// edit buffer), the page metadata, and the key hints.
fn draw_book(f: &mut Frame, app: &mut App, lb: WLayout) {
    let Some(b) = app.world.book.as_ref() else {
        return;
    };
    let title = format!("Book: {} [p{}]", b.name, b.current + 1);
    draw_box(f, app, 0, lb.world_y0, lb.left_w, lb.world_y1, &title, true);
    let inner_w = lb.left_w.saturating_sub(4) as usize;
    let meta_row = lb.world_y0 + 1;
    let hint_row = lb.world_y1 - 1;
    let shown = b.pages.get(b.current).map(|p| p.index).unwrap_or((b.current + 1) as u32);
    put(
        f,
        meta_row,
        2,
        trunc(&format!("by {}  page {} of {}  [{}p max {}c]", b.author, shown, b.pages.len().max(1), b.max_pages, b.max_chars), inner_w),
        dark(app),
    );
    let mut r = lb.world_y0 + 2;
    if !b.inscription.is_empty() {
        put(f, r, 2, trunc(&b.inscription, inner_w), dark(app));
        r += 1;
    }
    let text: &str = if b.editing {
        &b.buffer
    } else {
        b.pages.get(b.current).map(|p| p.text.as_str()).unwrap_or("(blank)")
    };
    let sty = if b.editing { focus(app) } else { normal(app) };
    let text_rows = hint_row.saturating_sub(r) as usize;
    for line in text.split('\n').take(text_rows) {
        put(f, r, 2, trunc(line, inner_w), sty);
        r += 1;
    }
    let hint = if b.editing {
        "enter save | esc done"
    } else {
        "up/dn page | e edit | a add | d del | r fetch | esc close"
    };
    put(f, hint_row, 2, trunc(hint, inner_w), dark(app));
}

// ---------------------------------------------------------------- vitals

/// Three horizontal vitals bars (health=red, stamina=yellow, mana=blue) with
/// current/max overlaid in the center of each.
fn draw_vitals(f: &mut Frame, app: &App, lb: WLayout) {
    let bar_row: u16 = 1;
    let x0: u16 = 0;
    let x1 = lb.left_w;
    let inner_w = x1 - x0 - 2;
    let gap: u16 = 2;
    let bar_w = 4.max((inner_w - 2 * gap) / 3);
    let v = &app.world.vitals;
    let bars: [((u32, u32), Color); 3] = [
        ((v.health.unwrap_or(0), v.health_max.unwrap_or(0)), Color::Red),
        ((v.stamina.unwrap_or(0), v.stamina_max.unwrap_or(0)), Color::Yellow),
        ((v.mana.unwrap_or(0), v.mana_max.unwrap_or(0)), Color::Blue),
    ];
    let mut c = x0 + 1;
    for ((cur, max), color) in bars {
        let filled = if max > 0 {
            ((cur as f32 / max as f32) * bar_w as f32).round() as usize
        } else {
            0
        }
        .min(bar_w as usize);
        let bar: String = (0..bar_w as usize).map(|i| if i < filled { '#' } else { '.' }).collect();
        put(f, bar_row, c, &bar, ts(app, Style::default().fg(color)));
        let txt = format!("{cur}/{max}");
        if txt.len() < bar_w as usize {
            let start = c + ((bar_w as usize - txt.len()) / 2) as u16;
            if start > c {
                put(f, bar_row, start, &txt, normal(app));
            }
        }
        c += bar_w + gap;
    }
}

// ---------------------------------------------------------------- tactical map

/// The tactical map's geometry: the pane center plus a blip for every
/// positioned entity (self excluded). Shared by the draw pass and the pick.
struct Tactical {
    cx: u16,
    cy: u16,
    blips: Vec<(u32, char, u16, u16)>, // (guid, char, col, row)
}

fn tactical_geom(app: &App, lb: WLayout) -> Option<Tactical> {
    let inner_x0: u16 = 1;
    let inner_x1 = lb.left_w - 2;
    let inner_y0 = lb.world_y0 + 1;
    let map_y1 = lb.world_y1 - 1;
    let Some(mv) = app.movement.as_ref() else {
        return None;
    };
    let (sx, sy) = (mv.pos.x, mv.pos.y);
    let map_h = map_y1 - inner_y0;
    let inner_w = inner_x1 - inner_x0 + 1;
    let cx = inner_x0 + inner_w / 2;
    let cy = inner_y0 + map_h / 2;
    // Scale so the furthest nearby thing fits; clamp to a sane range.
    let mut max_d = 20.0f32;
    for e in app.world.entities.values() {
        if Some(e.guid) == app.self_guid {
            continue;
        }
        max_d = max_d.max((e.pos.x - sx).hypot(e.pos.y - sy));
    }
    max_d = (max_d * 1.15).min(300.0);
    let half = (1.max(inner_w.min(map_h) as usize / 2 - 1)) as f32;
    let scale = max_d / half;
    let mut blips: Vec<(u32, char, u16, u16)> = Vec::new();
    for e in app.world.entities.values() {
        if Some(e.guid) == app.self_guid {
            continue;
        }
        let ch = if e.is_player { 'p' } else { entity_char(e.item_type) };
        let col = (cx as i32 + ((e.pos.x - sx) / scale).round() as i32) as u16;
        let row = (cy as i32 - ((e.pos.y - sy) / scale).round() as i32) as u16;
        blips.push((e.guid, ch, col, row));
    }
    Some(Tactical { cx, cy, blips })
}

/// The north-up top-down tactical map: the player at center with their
/// heading tick, entity blips (p player, m creature, d door, c chest, .
/// other), and the current target marked "T".
fn draw_tactical(f: &mut Frame, app: &App, lb: WLayout) {
    let inner_x0: u16 = 1;
    let inner_x1 = lb.left_w - 2;
    let inner_y0 = lb.world_y0 + 1;
    let map_y1 = lb.world_y1 - 1;
    let Some(t) = tactical_geom(app, lb) else {
        put(f, lb.world_y0 + 2, inner_x0 + 1, "(not in world)", normal(app));
        return;
    };
    let inner_w = (inner_x1 - inner_x0 + 1) as usize;
    let map_h = (map_y1 - inner_y0) as usize;
    let mut grid = vec![' '; inner_w * map_h];
    let set = |grid: &mut Vec<char>, c: u16, r: u16, ch: char| {
        if r < inner_y0 || r >= map_y1 || c < inner_x0 || c >= inner_x1 {
            return;
        }
        grid[((r - inner_y0) as usize) * inner_w + (c - inner_x0) as usize] = ch;
    };
    // The heading tick two cells out from center (the player's bearing).
    let hd = app.movement.as_ref().map(|m| m.heading).unwrap_or(0.0);
    set(
        &mut grid,
        (t.cx as i32 + (2.0 * hd.sin()).round() as i32).max(0) as u16,
        (t.cy as i32 - (2.0 * hd.cos()).round() as i32).max(0) as u16,
        '\'',
    );
    for (_g, ch, c, r) in &t.blips {
        set(&mut grid, *c, *r, *ch);
    }
    // The current target gets a "T" marker (over its blip).
    if let Some(guid) = app.current_target() {
        if let Some((_g, _ch, c, r)) = t.blips.iter().find(|(g, _, _, _)| *g == guid) {
            set(&mut grid, *c, *r, 'T');
        }
    }
    set(&mut grid, t.cx, t.cy, '@'); // self drawn last: never covered
    for r in inner_y0..map_y1 {
        let row: String =
            grid[((r - inner_y0) as usize) * inner_w..((r - inner_y0) as usize) * inner_w + inner_w]
                .iter()
                .collect();
        put(f, r, inner_x0, &row, normal(app));
    }
}

/// The tactical/radar blip for an item type.
fn entity_char(t: u32) -> char {
    match t {
        ITEM_TYPE_CREATURE => 'm',
        ITEM_TYPE_DOOR => 'd',
        ITEM_TYPE_CONTAINER => 'c',
        _ => '.',
    }
}

// ---------------------------------------------------------------- 3d world

/// The 3D world view: render the player's landblock and every visible body
/// from the retail dats into the world pane, from a third-person follow
/// camera. Each cell is a full-block glyph colored from the shaded z-buffer.
fn draw_world3d(f: &mut Frame, app: &mut App, lb: WLayout) {
    let x0: u16 = 1;
    let y0 = lb.world_y0 + 1;
    let x1 = lb.left_w - 2;
    let y1 = lb.world_y1 - 2;
    let cols = (x1 - x0 + 1) as usize;
    let rows = (y1 - y0 + 1) as usize;
    let sty = normal(app);
    let mut note = |m: &str| put(f, y0 + 1, x0 + 1, trunc(m, cols.saturating_sub(2)), sty);
    let (cell, sp) = app.self_position();
    let lb_id = (cell >> 16) & 0xFFFF;
    if lb_id == 0 {
        note("(no landblock cell yet)");
        return;
    }
    if app.world3d.as_ref().map(|w| !w.available()).unwrap_or(true) {
        note("(retail dats not found)");
        return;
    }
    let heading = app.movement.as_ref().map(|m| m.heading).unwrap_or(0.0);
    // Build the render inputs before the mutable render borrow.
    let actors = build_actors(app);
    let cam = Camera {
        x: wrap_block_coord(sp.x),
        y: wrap_block_coord(sp.y),
        z: sp.z,
        heading,
    };
    let ret = reticle_target(app);
    let frame = app
        .world3d
        .as_mut()
        .and_then(|w3d| w3d.render(lb_id, &cam, &actors, cols, rows, app.glow_actors, ret.as_ref(), app.reticle_animation));
    let Some(frame) = frame else {
        note("(3d: nothing to render)");
        return;
    };
    // Blit: run-length encode each row's equal-color cells into one span.
    for (ri, row) in frame.cells.chunks_exact(cols).enumerate() {
        let mut c = 0usize;
        while c < cols {
            let col = row[c];
            let mut e = c + 1;
            while e < cols && row[e] == col {
                e += 1;
            }
            let s = Style::default().fg(Color::Rgb(col[0], col[1], col[2]));
            put(f, y0 + ri as u16, x0 + c as u16, &"█".repeat(e - c), ts(app, s));
            c = e;
        }
    }
}

/// The bodies to render: every positioned entity with a body (players
/// always; creatures / ground items / doors / containers by item type). The
/// self actor uses the movement sim's live pos/heading (the entity's own pos
/// lags the last server update) and a gold glow.
fn build_actors(app: &App) -> Vec<Actor> {
    let mut actors: Vec<Actor> = Vec::new();
    let (_self_cell, self_pos) = app.self_position();
    let self_heading = app.movement.as_ref().map(|m| m.heading).unwrap_or(0.0);
    for e in app.world.entities.values() {
        if Some(e.guid) == app.self_guid {
            let half = self_heading / 2.0;
            actors.push(Actor {
                guid: e.guid,
                x: self_pos.x,
                y: self_pos.y,
                z: self_pos.z,
                quat: [half.cos(), 0.0, 0.0, half.sin()],
                mtable_id: e.mtable_id,
                setup_id: e.setup_id,
                model_parts: e.model_parts.clone(),
                motion_cmd: app
                    .movement
                    .as_ref()
                    .map(|m| m.motion_cmd())
                    .unwrap_or(e.motion_cmd),
                style: 0,
                glow: Some(GLOW_SELF),
                scale: ACTOR_SCALE,
            });
            continue;
        }
        if !e.is_player
            && e.item_type != ITEM_TYPE_CREATURE
            && e.item_type != ITEM_TYPE_GROUND
            && e.item_type != ITEM_TYPE_DOOR
            && e.item_type != ITEM_TYPE_CONTAINER
        {
            continue;
        }
        let glow = if e.is_player {
            Some(GLOW_PLAYER)
        } else if e.item_type == ITEM_TYPE_CREATURE {
            Some(GLOW_CREATURE)
        } else {
            None
        };
        actors.push(Actor {
            guid: e.guid,
            x: e.pos.x,
            y: e.pos.y,
            z: e.pos.z,
            quat: e.rot,
            mtable_id: e.mtable_id,
            setup_id: e.setup_id,
            model_parts: e.model_parts.clone(),
            motion_cmd: if e.is_player { e.motion_cmd } else { MCMD_READY },
            style: if e.is_player { 0 } else { STYLE_CREATURE },
            glow,
            // Humanoid bodies (players/creatures) render at the scaled-up
            // height; objects (ground items/doors/containers) stay 1.0.
            scale: if glow.is_some() { ACTOR_SCALE } else { 1.0 },
        });
    }
    actors
}

/// The 3D target reticle: the current target's feet position plus a per-kind
/// half-extent bounding the rendered body. `None` without a world target.
fn reticle_target(app: &App) -> Option<ReticleTarget> {
    let (cell, _sp) = app.self_position();
    if cell == 0 {
        return None;
    }
    let guid = app.current_target()?;
    let e = app.world.entities.get(&guid)?;
    let (r, half_h) = entity_half_extents(e);
    Some(ReticleTarget {
        x: e.pos.x,
        y: e.pos.y,
        z: e.pos.z,
        r,
        half_h,
    })
}

// ---------------------------------------------------------------- radar

/// Borderless radar (the retail compass): a heading-up circle (rotates with
/// the player's bearing) with N/S/E/W on the rim, entities plotted by
/// bearing + distance, and the coordinates on the bottom row.
fn draw_radar(f: &mut Frame, app: &App, lb: WLayout, wcols: u16) {
    let x0 = lb.right_x;
    let x1 = (x0 + lb.right_w).min(wcols);
    let y0: u16 = 0;
    let y1 = lb.radar_h;
    let coord_row = y1 - 1;
    let circle_y1 = y1 - 1; // circle rows are y0 .. circleY1-1
    let Some(mv) = app.movement.as_ref() else {
        put(f, y0 + 1, x0 + 1, "(not in world)", normal(app));
        return;
    };
    let (sx, sy) = (mv.pos.x, mv.pos.y);
    let heading = mv.heading;
    let inner_w = (x1 - x0) as usize;
    let circle_h = (circle_y1 - y0) as usize;
    let cx = x0 + (inner_w / 2) as u16;
    let cy = y0 + (circle_h / 2) as u16;
    let r_max = 2.max(inner_w.saturating_sub(1) / 2).min(circle_h.saturating_sub(1) / 2);
    // The rim.
    for r in y0..circle_y1 {
        for c in x0..x1 {
            let dr = (r as f32) - (cy as f32);
            let dc = (c as f32) - (cx as f32);
            if (dr.hypot(dc) - r_max as f32).abs() < 0.6 {
                put(f, r, c, "o", normal(app));
            }
        }
    }
    // N/S/E/W on the rim, rotated so the player's bearing is up.
    for (deg, ch) in [(0f32, 'N'), (90.0, 'E'), (180.0, 'S'), (270.0, 'W')] {
        let rad = deg.to_radians() - heading;
        let row = (cy as f32 - r_max as f32 * rad.cos()).round() as u16;
        let col = (cx as f32 + r_max as f32 * rad.sin()).round() as u16;
        put(f, row, col, &ch.to_string(), focus(app));
    }
    // Entities: bearing + distance from self (POI markers excluded).
    let mut ents: Vec<(u32, char, f32, f32)> = Vec::new(); // (guid, ch, bearing, dist)
    let mut max_d = 20.0f32;
    for e in app.world.entities.values() {
        if Some(e.guid) == app.self_guid || e.item_type == ITEM_TYPE_POI {
            continue;
        }
        let ch = if e.is_player { 'p' } else { entity_char(e.item_type) };
        let rx = e.pos.x - sx;
        let ry = e.pos.y - sy;
        let dist = rx.hypot(ry);
        if dist < 1e-6 {
            continue;
        }
        let bearing = rx.atan2(ry); // radians clockwise from north
        max_d = max_d.max(dist);
        ents.push((e.guid, ch, bearing, dist));
    }
    max_d = (max_d * 1.15).min(300.0);
    for (guid, ch, bearing, dist) in ents {
        let rad = bearing - heading;
        let rr = (dist / max_d) * (r_max as f32 - 1.0);
        let col = (cx as f32 + rr * rad.sin()).round() as u16;
        let row = (cy as f32 - rr * rad.cos()).round() as u16;
        if row >= y0 && row < circle_y1 && col >= x0 && col < x1 {
            let sel = app.current_target() == Some(guid);
            let glyph = if sel { "^".to_string() } else { ch.to_string() };
            put(f, row, col, &glyph, if sel { reverse(app) } else { normal(app) });
        }
    }
    // Self at center.
    put(f, cy, cx, "@", focus(app));
    // Coordinates.
    let coords = fmt_coord(sx, sy);
    let start = if cx >= (coords.len() / 2) as u16 {
        cx - (coords.len() / 2) as u16
    } else {
        0
    };
    put(f, coord_row, start, &coords, normal(app));
}

/// `12.3N, 45.6E` style coordinates.
fn fmt_coord(x: f32, y: f32) -> String {
    format!(
        "{:.1}{}, {:.1}{}",
        y.abs(),
        if y >= 0.0 { 'N' } else { 'S' },
        x.abs(),
        if x >= 0.0 { 'E' } else { 'W' }
    )
}

// ---------------------------------------------------------------- chat

/// The chat pane: split each line into visual lines on newlines, hard-wrap
/// each to the pane's inner width, showing the most recent that fit. Content
/// stays in columns 2 .. leftW-3.
fn draw_chat(f: &mut Frame, app: &App, lb: WLayout) {
    let y0 = lb.chat_y0;
    let y1 = lb.body_h;
    let inner_rows = (y1 - y0 - 2) as usize;
    if inner_rows < 1 {
        return;
    }
    let inner_w = (lb.left_w - 4) as usize;
    let mut wrapped: Vec<(String, Style)> = Vec::new();
    for line in app.world.chat.iter() {
        let style = style_for_chat(app, line.style);
        let text = line.text.replace("\r\n", "\n").replace('\r', "\n");
        for para in text.split('\n') {
            let chars: Vec<char> = para.chars().collect();
            for chunk in chars.chunks(inner_w.max(1)) {
                let s: String = chunk.iter().collect();
                wrapped.push((s, style));
            }
        }
    }
    let start = wrapped.len().saturating_sub(inner_rows);
    for (i, (text, style)) in wrapped.iter().skip(start).enumerate() {
        put(f, y0 + 1 + i as u16, 2, text, *style);
    }
}

/// The chat line's color (the reference's primary chat color per style).
fn style_for_chat(app: &App, style: ChatStyle) -> Style {
    let c = match style {
        ChatStyle::Speech => Color::White,
        ChatStyle::Own => Color::Cyan,
        ChatStyle::Tell => Color::Yellow,
        ChatStyle::Channel => Color::Blue,
        ChatStyle::System => Color::DarkGray,
        ChatStyle::Combat => Color::Red,
        ChatStyle::Emote => Color::Green,
        ChatStyle::Error => Color::Red,
    };
    ts(app, Style::default().fg(c))
}

// ---------------------------------------------------------------- status bar

/// The solid reverse status bar on the bottom row: connection info, combat
/// mode, the target readout, and a one-shot flash (never truncated).
fn draw_status_bar(f: &mut Frame, app: &App, w: u16) {
    let h = f.area().height;
    let cap = (w - 1) as usize;
    let status;
    if app.chat_focused {
        // Chat mode: the target readout + the input buffer.
        let t = target_readout(app);
        let talk = if app.talk_to.is_some() { "talk" } else { "chat" };
        status = format!(" {t} | {talk}> {}_", app.chat_buf);
    } else {
        let combat = if app.world.combat.mode == combat_mode::NON_COMBAT { "peace" } else { "combat" };
        let t = target_readout(app);
        let mid = format!(" | {combat} | {t}");
        let conn = format!("srv: {} | acc: {}", app.server_name, app.account);
        let tail = match &app.world.flash {
            Some(fl) => format!(" |{fl} "),
            None => " ".to_string(),
        };
        let max_conn = cap.saturating_sub(1).saturating_sub(mid.len()).saturating_sub(tail.len());
        let conn_shown = conn.chars().take(max_conn).collect::<String>().trim_end().to_string();
        status = format!(" {conn_shown}{mid}{tail}");
    }
    let mut line = String::with_capacity(cap);
    line.push(' ');
    for ch in status.chars().take(cap.saturating_sub(1)) {
        line.push(ch);
    }
    while line.len() < cap {
        line.push(' ');
    }
    put(f, h - 1, 0, &line, reverse(app));
}

/// The status-bar target readout: the selected item's name (never a raw
/// guid) or the current world target's name + vital, or `(none)`.
fn target_readout(app: &App) -> String {
    if let Some(guid) = targeted_item(app) {
        return item_name(app, guid);
    }
    match app.current_target() {
        Some(guid) => {
            let mut s = entity_name(app, guid);
            if let Some(e) = app.world.entities.get(&guid) {
                if let (Some(h), Some(hm)) = (e.health, e.health_max) {
                    if hm > 0 {
                        s.push_str(&format!(" {}%H", (h as f32 / hm as f32 * 100.0).round() as u32));
                    }
                } else if let (Some(m), Some(mm)) = (e.mana, e.mana_max) {
                    if mm > 0 {
                        s.push_str(&format!(" {}%M", (m as f32 / mm as f32 * 100.0).round() as u32));
                    }
                }
            }
            s
        }
        None => "(none)".to_string(),
    }
}

/// The name of a world entity (never a raw guid): the record's name, else
/// the kind label from its item type.
fn entity_name(app: &App, guid: u32) -> String {
    match app.world.entities.get(&guid) {
        Some(e) => match &e.name {
            Some(n) if !n.is_empty() => n.clone(),
            _ => {
                let is_creature = e.is_player || e.health.is_some() || e.item_type == 0;
                if is_creature {
                    "creature".to_string()
                } else {
                    item_type_name(e.item_type).to_string()
                }
            }
        },
        None => "?".to_string(),
    }
}

/// The display name of an item guid (never a raw guid): the pack record,
/// else the equipped record, else the vendor stock, else the open container,
/// else the item-type's name.
fn item_name(app: &App, guid: u32) -> String {
    for it in app.world.inventory.pack.iter().chain(app.world.inventory.equipped.iter()) {
        if it.guid == guid {
            return if it.name.is_empty() {
                item_type_name(it.item_type).to_string()
            } else {
                it.name.clone()
            };
        }
    }
    if let Some(vi) = app.world.vendor.items.iter().find(|i| i.guid == guid) {
        return if vi.name.is_empty() {
            item_type_name(vi.item_type).to_string()
        } else {
            vi.name.clone()
        };
    }
    if let Some(it) = app.world.container.items.iter().find(|i| i.guid == guid) {
        return if it.name.is_empty() {
            item_type_name(it.item_type).to_string()
        } else {
            it.name.clone()
        };
    }
    "?".to_string()
}

/// The glyph for an item guid (pack, equipped, or the open container).
fn item_glyph_of_guid(app: &App, guid: u32) -> char {
    for it in app.world.inventory.pack.iter().chain(app.world.inventory.equipped.iter()) {
        if it.guid == guid {
            return item_glyph(it.item_type);
        }
    }
    if let Some(it) = app.world.container.items.iter().find(|i| i.guid == guid) {
        return item_glyph(it.item_type);
    }
    '?'
}

/// The Nerd Font glyph for an item type (first matching flag).
fn item_glyph(t: u32) -> char {
    for (flag, g, _n) in ITEM_TYPES {
        if t & flag != 0 {
            return g;
        }
    }
    '?'
}

/// The short name for an item type (first matching flag).
fn item_type_name(t: u32) -> &'static str {
    for (flag, _g, n) in ITEM_TYPES {
        if t & flag != 0 {
            return n;
        }
    }
    "?"
}
// ================================================================== multi box

/// One content row of the multi box (row `i` inside its inner area),
/// truncated to the inner width.
fn mrow(f: &mut Frame, _app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize, i: usize, text: &str, style: Style) {
    if i < rows {
        put(f, y0 + 1 + i as u16, x0 + 1, trunc(text, inner_w), style);
    }
}

/// The multi box content for the active widget (right column).
fn draw_multi(f: &mut Frame, app: &App, lb: WLayout, wcols: u16) {
    let x0 = lb.right_x;
    let x1 = (x0 + lb.right_w).min(wcols);
    let y0 = lb.radar_h;
    let y1 = lb.multi_y1;
    let inner_w = (x1 - x0 - 2) as usize;
    let rows = (y1 - y0 - 2) as usize;
    match app.multi_sel.min(13) {
        0 => draw_allegiance(f, app, x0, y0, rows, inner_w),
        1 => draw_fellowship(f, app, x0, y0, rows, inner_w),
        2 => draw_friends(f, app, x0, y0, rows, inner_w),
        3 => draw_squelch(f, app, x0, y0, rows, inner_w),
        4 => draw_spellbook(f, app, x0, y0, rows, inner_w),
        5 => draw_attributes(f, app, x0, y0, rows, inner_w),
        6 => draw_skills(f, app, x0, y0, rows, inner_w),
        7 => draw_titles(f, app, x0, y0, rows, inner_w),
        8 => draw_contracts(f, app, x0, y0, rows, inner_w),
        9 => draw_dereth(f, app, x0, y0, rows, inner_w),
        10 => draw_house(f, app, x0, y0, rows, inner_w),
        11 => draw_inventory(f, app, x0, y0, rows, inner_w),
        12 => draw_you(f, app, x0, y0, rows, inner_w),
        _ => draw_examine(f, app, x0, y0, rows, inner_w),
    }
}

/// A parenthesized note row (the widget's empty state).
fn mnote(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize, i: usize, txt: &str) {
    mrow(f, app, x0, y0, rows, inner_w, i, &format!("({txt})"), normal(app));
}

fn draw_allegiance(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    match &app.world.social.allegiance {
        None => mnote(f, app, x0, y0, rows, inner_w, 0, "not in an allegiance"),
        Some(a) => {
            mrow(f, app, x0, y0, rows, inner_w, 0, &format!("leader: {}", a.leader), focus(app));
            mrow(f, app, x0, y0, rows, inner_w, 1, &format!("level: {}", a.level), normal(app));
        }
    }
    mrow(f, app, x0, y0, rows, inner_w, rows.saturating_sub(1), "v swear to target | m break w/ target", dark(app));
}

fn draw_fellowship(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    let Some(fe) = app.world.social.fellowship.as_ref() else {
        mnote(f, app, x0, y0, rows, inner_w, 0, "not in a fellowship");
        mrow(f, app, x0, y0, rows, inner_w, rows.saturating_sub(1), "/fellowship <name> create | /fellowship quit", dark(app));
        return;
    };
    let open = if fe.open { "open" } else { "closed" };
    mrow(f, app, x0, y0, rows, inner_w, 0, &format!("leader: {} ({open})", fe.leader), focus(app));
    for (i, m) in fe.members.iter().enumerate() {
        if i < app.list_scroll {
            continue;
        }
        let row = 1 + (i - app.list_scroll);
        if row >= rows.saturating_sub(2) {
            break;
        }
        mrow(f, app, x0, y0, rows, inner_w, row, m, normal(app));
    }
    let shown = fe.members.len().saturating_sub(app.list_scroll).min(rows.saturating_sub(3).max(0));
    if fe.members.len() > app.list_scroll + shown {
        mrow(f, app, x0, y0, rows, inner_w, rows.saturating_sub(2), &format!("+{} more", fe.members.len() - app.list_scroll - shown), dark(app));
    }
    mrow(f, app, x0, y0, rows, inner_w, rows.saturating_sub(1), "v recruit | m dismiss | n lead | p open", dark(app));
}

fn draw_friends(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    let friends = &app.world.social.friends;
    if friends.is_empty() {
        mnote(f, app, x0, y0, rows, inner_w, 0, "no friends");
        mrow(f, app, x0, y0, rows, inner_w, rows.saturating_sub(1), "/friend <name> to add", dark(app));
        return;
    }
    mrow(f, app, x0, y0, rows, inner_w, 0, &format!("{} friends", friends.len()), normal(app));
    let cap = rows.saturating_sub(3);
    for (i, m) in friends.iter().enumerate() {
        if i < app.list_scroll {
            continue;
        }
        let row = 1 + (i - app.list_scroll);
        if row >= rows.saturating_sub(2) {
            break;
        }
        mrow(f, app, x0, y0, rows, inner_w, row, m, normal(app));
    }
    if friends.len() > app.list_scroll + cap {
        mrow(f, app, x0, y0, rows, inner_w, rows.saturating_sub(2), &format!("+{} more", friends.len() - app.list_scroll - cap), dark(app));
    }
    mrow(f, app, x0, y0, rows, inner_w, rows.saturating_sub(1), "m remove target | /friend <name> add", dark(app));
}

fn draw_squelch(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    let q = &app.world.social.squelch;
    if q.is_empty() {
        mnote(f, app, x0, y0, rows, inner_w, 0, "no squelches");
        return;
    }
    mrow(f, app, x0, y0, rows, inner_w, 0, &format!("{} squelched", q.len()), normal(app));
    let cap = rows.saturating_sub(2);
    for (i, m) in q.iter().enumerate() {
        if i < app.list_scroll {
            continue;
        }
        let row = 1 + (i - app.list_scroll);
        if row >= rows {
            break;
        }
        mrow(f, app, x0, y0, rows, inner_w, row, m, normal(app));
    }
    if q.len() > app.list_scroll + cap {
        mrow(f, app, x0, y0, rows, inner_w, rows.saturating_sub(1), &format!("+{} more", q.len() - app.list_scroll - cap), dark(app));
    }
}

fn draw_spellbook(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    let sk = &app.world.skills;
    let none = sk.spells.is_empty() && sk.spell_bar.iter().all(|t| t.is_empty());
    if none {
        mnote(f, app, x0, y0, rows, inner_w, 0, "no known spells");
    } else {
        let filters = if sk.filters.is_empty() {
            "none".to_string()
        } else {
            sk.filters.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(" ")
        };
        mrow(f, app, x0, y0, rows, inner_w, 0, &format!("known {}  filters {}", sk.spells.len(), filters), normal(app));
        let lens: Vec<String> = (0..8)
            .map(|i| sk.spell_bar.get(i).map(|t| t.len().to_string()).unwrap_or_else(|| "0".into()))
            .collect();
        let bar = format!("bar: {}", lens.join(" "));
        mrow(f, app, x0, y0, rows, inner_w, 1, &bar, normal(app));
        for (k, chunk) in sk.spells.chunks(6).enumerate() {
            if k < app.list_scroll {
                continue;
            }
            let row = 3 + (k - app.list_scroll);
            if row >= rows.saturating_sub(1) {
                break;
            }
            let ids: Vec<String> = chunk.iter().map(|n| format!("{n:>4}")).collect();
            mrow(f, app, x0, y0, rows, inner_w, row, &ids.join(" "), normal(app));
        }
    }
    mrow(
        f,
        app,
        x0,
        y0,
        rows,
        inner_w,
        rows.saturating_sub(1),
        &format!("slot {} tab {}/8 | p add m rm v forget", SLOTS[sk.slot.min(9)], sk.tab + 1),
        dark(app),
    );
}

fn draw_attributes(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    let names = ["Strength", "Endurance", "Coordination", "Quickness", "Focus", "Self"];
    let v = &app.world.vitals;
    for (i, n) in names.iter().enumerate() {
        mrow(f, app, x0, y0, rows, inner_w, i, &format!("{:<13} {}", n, v.attrs[i]), normal(app));
    }
    mrow(
        f,
        app,
        x0,
        y0,
        rows,
        inner_w,
        6,
        &format!("hp {}/{}", v.health.unwrap_or(0), v.health_max.unwrap_or(0)),
        normal(app),
    );
    mrow(
        f,
        app,
        x0,
        y0,
        rows,
        inner_w,
        7,
        &format!(
            "st {}/{}  mn {}/{}",
            v.stamina.unwrap_or(0),
            v.stamina_max.unwrap_or(0),
            v.mana.unwrap_or(0),
            v.mana_max.unwrap_or(0)
        ),
        normal(app),
    );
}

fn draw_skills(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    let sk = &app.world.skills;
    if sk.skills.is_empty() {
        mnote(f, app, x0, y0, rows, inner_w, 0, "no trained skills");
        return;
    }
    // Every skill the character owns: a fresh character's list is all rank 0
    // (the server sends it that way), so showing only rank > 0 would be empty.
    // Trained skills sort first, then by retail ID.
    let mut list: Vec<&(u32, i32)> = sk.skills.iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    mrow(f, app, x0, y0, rows, inner_w, 0, &format!("{} skills", list.len()), normal(app));
    let cap = rows.saturating_sub(1);
    for (i, (id, rank)) in list.iter().enumerate() {
        if i < app.list_scroll {
            continue;
        }
        let row = 1 + (i - app.list_scroll);
        if row >= rows {
            break;
        }
        let name = acproto::messages::SKILL_NAMES.get(*id as usize).copied().unwrap_or("skill");
        mrow(f, app, x0, y0, rows, inner_w, row, &format!("{name}  {rank}"), normal(app));
    }
    if list.len() > app.list_scroll + cap {
        mrow(f, app, x0, y0, rows, inner_w, rows.saturating_sub(1), &format!("+{} more", list.len() - app.list_scroll - cap), dark(app));
    }
}

fn draw_titles(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    let titles = &app.world.social.titles;
    if titles.is_empty() {
        mnote(f, app, x0, y0, rows, inner_w, 0, "no titles");
        return;
    }
    let cur = app.world.social.title;
    mrow(f, app, x0, y0, rows, inner_w, 0, &format!("owned {}", titles.len()), normal(app));
    mrow(f, app, x0, y0, rows, inner_w, 1, &format!("current: title {cur}"), focus(app));
    let cap = rows.saturating_sub(4).max(0);
    for (i, t) in titles.iter().enumerate() {
        if i < app.list_scroll {
            continue;
        }
        let row = 3 + (i - app.list_scroll);
        if row >= rows.saturating_sub(1) || i - app.list_scroll >= cap {
            break;
        }
        let mark = if *t == cur { "* " } else { "  " };
        mrow(f, app, x0, y0, rows, inner_w, row, &format!("{mark}title {t}"), normal(app));
    }
    if titles.len() > app.list_scroll + cap {
        mrow(f, app, x0, y0, rows, inner_w, rows.saturating_sub(1), &format!("+{} more", titles.len() - app.list_scroll - cap), dark(app));
    }
}

fn draw_contracts(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    let cs = &app.world.social.contracts;
    if cs.is_empty() {
        mnote(f, app, x0, y0, rows, inner_w, 0, "no contracts");
        return;
    }
    mrow(f, app, x0, y0, rows, inner_w, 0, &format!("{} contracts", cs.len()), normal(app));
    let cap = rows.saturating_sub(2);
    for (i, c) in cs.iter().enumerate() {
        if i < app.list_scroll {
            continue;
        }
        let row = 1 + (i - app.list_scroll);
        if row >= rows {
            break;
        }
        mrow(f, app, x0, y0, rows, inner_w, row, &format!("#{} {} (stage {})", c.id, c.title, c.stage), normal(app));
    }
    if cs.len() > app.list_scroll + cap {
        mrow(f, app, x0, y0, rows, inner_w, rows.saturating_sub(1), &format!("+{} more", cs.len() - app.list_scroll - cap), dark(app));
    }
}

/// The Map widget: the whole Dereth continent (landmask from the retail cell
/// dat, land = any non-zero height byte) with the player's landblock as one
/// point.
fn draw_dereth(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    let avail_h = rows.saturating_sub(1).max(1);
    let Some(land) = get_dereth_land(app.dat_dir.as_deref()) else {
        mrow(f, app, x0, y0, rows, inner_w, 0, "(retail dats not found — set dat_dir)", normal(app));
        return;
    };
    let w = (land.max_x - land.min_x + 1) as f32;
    let h = (land.max_y - land.min_y + 1) as f32;
    let cell = (inner_w as f32 / w).min(avail_h as f32 / h);
    let mw = 1.max((w * cell).round() as usize).min(inner_w);
    let mh = 1.max((h * cell).round() as usize).min(avail_h);
    let ox = x0 + 1 + ((inner_w - mw) / 2) as u16;
    let oy = y0 + 1 + ((avail_h - mh) / 2) as u16;
    let is_land = |lbx: i64, lby: i64| {
        (0..255).contains(&lbx) && (0..255).contains(&lby) && land.land[(lby * 255 + lbx) as usize] == 1
    };
    for py in 0..mh {
        let ly0 = land.min_y as i64 + (py as f32 / cell) as i64;
        let ly1 = (255i64).min(land.min_y as i64 + ((py + 1) as f32 / cell) as i64);
        let mut line = String::new();
        for px in 0..mw {
            let lx0 = land.min_x as i64 + (px as f32 / cell) as i64;
            let lx1 = (255i64).min(land.min_x as i64 + ((px + 1) as f32 / cell) as i64);
            let mut l = false;
            for lby in ly0..ly1 {
                if l {
                    break;
                }
                for lbx in lx0..lx1 {
                    if is_land(lbx, lby) {
                        l = true;
                        break;
                    }
                }
            }
            line.push(if l { '▓' } else { ' ' });
        }
        put(f, oy + py as u16, ox, &line, normal(app));
    }
    let info;
    if let Some(mv) = app.movement.as_ref() {
        let lb = (mv.cell >> 16) & 0xFFFF;
        let lbx = (lb >> 8) & 0xFF;
        let lby = lb & 0xFF;
        let px = (((lbx as f32 - land.min_x as f32 + 0.5) * cell).round() as i64).clamp(0, mw as i64 - 1) as u16;
        let py = (((lby as f32 - land.min_y as f32 + 0.5) * cell).round() as i64).clamp(0, mh as i64 - 1) as u16;
        put(f, oy + py, ox + px, "@", reverse(app));
        info = format!("Dereth — you {lbx},{lby}");
    } else {
        info = "Dereth — not in world".to_string();
    }
    put(f, y0 + rows as u16 + 1, x0 + 1, trunc(&info, inner_w), normal(app));
}

fn draw_house(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    match &app.world.social.house {
        None => mnote(f, app, x0, y0, rows, inner_w, 0, "no house data"),
        Some(h) => {
            mrow(f, app, x0, y0, rows, inner_w, 0, &h.name, focus(app));
            mrow(f, app, x0, y0, rows, inner_w, 1, &format!("district: {}", h.district), normal(app));
            mrow(f, app, x0, y0, rows, inner_w, 2, &format!("id: {}", h.guid), normal(app));
        }
    }
}
// ---------------------------------------------------------------- inventory

/// The equipped items mapped to their paper-doll cells: each item claims the
/// first DOLL_SLOT (table order) whose EquipMask bits it occupies and that is
/// not already taken (the first-equipped item wins a cell).
fn doll_items(inv: &Inventory) -> Vec<(u32, u16, u16)> {
    let mut used: Vec<(u16, u16)> = Vec::new();
    let mut out: Vec<(u32, u16, u16)> = Vec::new();
    for it in &inv.equipped {
        for (_id, _name, r, c, bits) in DOLL_SLOTS {
            if (it.cur_loc as u32) & bits != 0 && !used.contains(&(r, c)) {
                used.push((r, c));
                out.push((it.guid, r, c));
                break;
            }
        }
    }
    out
}

/// The displayed pack grid's geometry inside the inventory widget (the draw
/// pass and the mouse hit-testing share it).
fn inv_grid_geom(inner_w: usize, rows: usize) -> (usize, usize, usize) {
    let info_rows = if rows >= 10 { 3 } else { 0 };
    let grid_w = 4.max(inner_w - 2);
    let grid_rows = 1.max(((MAIN_PACK_CAPACITY as f32) / grid_w as f32).ceil() as usize).min(rows.saturating_sub(INV_GRID_R0 as usize).saturating_sub(info_rows).max(1));
    (grid_w, grid_rows, inner_w - 1)
}

/// The first item index the pack grid window shows (the clamped scroll).
fn inv_scroll_for(app: &App, cells: usize) -> usize {
    let n = app.world.inventory.pack.len();
    app.list_scroll.min(n.saturating_sub(cells))
}

/// The paper-doll inventory widget: the doll (rows 0..4), the pack grid
/// (header row 5, grid from row 6), and the main pack's icon in the far
/// right column. Items are mouse-picked: click targets, right-click examines,
/// drag equips / moves, drop on the ground drops.
fn draw_inventory(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    if rows < 4 || inner_w < 12 {
        mrow(f, app, x0, y0, rows, inner_w, 0, "(too small)", normal(app));
        return;
    }
    let inv = &app.world.inventory;
    // Paper doll (rows 0..4): a slot shows the item's glyph, empty slots the
    // quiet marker, and the layout's structural blanks stay plain spaces.
    let doll = doll_items(inv);
    for (_id, _name, r, c, _bits) in DOLL_SLOTS {
        let guid = doll
            .iter()
            .find(|(_g, dr, dc)| *dr == r && *dc == c)
            .map(|(g, _, _)| *g);
        let glyph = match guid {
            Some(g) => item_glyph(
                inv.pack
                    .iter()
                    .chain(inv.equipped.iter())
                    .find(|i| i.guid == g)
                    .map(|i| i.item_type)
                    .unwrap_or(0),
            )
            .to_string(),
            None => "·".to_string(),
        };
        let sel = matches!(guid, Some(g) if item_is_targeted(app, g));
        put(f, y0 + 1 + r, x0 + 1 + c, &glyph, if sel { reverse(app) } else { normal(app) });
    }
    // Pack grid: header row 5, grid from row 6, icon column at the far right.
    let (grid_w, grid_rows, icon_x) = inv_grid_geom(inner_w, rows);
    let used = inv.pack.len();
    let cells = grid_rows * grid_w;
    let first = inv_scroll_for(app, cells);
    let hidden = used.saturating_sub(cells);
    let mut header = format!("b backpack {used}/102");
    if hidden > 0 {
        header.push_str(&format!("  [{}-{} of {used}]", first + 1, first + cells.min(used)));
    }
    put(f, y0 + 6, x0 + 1, trunc(&header, icon_x), focus(app));
    for r in 0..grid_rows {
        for c in 0..grid_w {
            let idx = first + r * grid_w + c;
            if idx >= MAIN_PACK_CAPACITY {
                break;
            }
            let glyph = if idx < used {
                item_glyph(inv.pack[idx].item_type).to_string()
            } else {
                "·".to_string()
            };
            let sel = idx < used && item_is_targeted(app, inv.pack[idx].guid);
            put(
                f,
                y0 + 1 + INV_GRID_R0 + r as u16,
                x0 + 1 + c as u16,
                &glyph,
                if sel { reverse(app) } else { normal(app) },
            );
        }
    }
    // Pack icon column: the main pack's icon (reversed = displayed), then
    // each side pack's icon (click one to open it; the open one is focused).
    put(f, y0 + 1 + INV_GRID_R0, x0 + 1 + icon_x as u16, "b", reverse(app));
    for (i, &guid) in inv.packs.iter().enumerate() {
        let row = y0 + 1 + INV_GRID_R0 + 1 + i as u16;
        if (row - y0) as usize >= rows {
            break;
        }
        let glyph = inv
            .pack
            .iter()
            .find(|it| it.guid == guid)
            .map(|it| item_glyph(it.item_type).to_string())
            .unwrap_or_else(|| "?".into());
        let open = app.world.container.guid == guid;
        put(f, row, x0 + 1 + icon_x as u16, &glyph, if open { focus(app) } else { normal(app) });
    }
}

// ---------------------------------------------------------------- you / examine

/// The 16-way compass name for a heading in radians.
fn bearing_name(rad: f32) -> &'static str {
    const NAMES: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW",
        "NNW",
    ];
    let deg = rad.to_degrees();
    let d = (deg % 360.0 + 360.0) % 360.0;
    NAMES[(d / 22.5).round() as usize % 16]
}

/// The You widget: the player's name, coordinates + bearing, burden and the
/// player count.
fn draw_you(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    let Some(mv) = app.movement.as_ref() else {
        mnote(f, app, x0, y0, rows, inner_w, 0, "not in world");
        return;
    };
    mrow(f, app, x0, y0, rows, inner_w, 0, &format!("you: {}", app.self_name), focus(app));
    mrow(
        f,
        app,
        x0,
        y0,
        rows,
        inner_w,
        1,
        &format!("{}  facing {}", fmt_coord(mv.pos.x, mv.pos.y), bearing_name(mv.heading)),
        normal(app),
    );
    let players = app.world.entities.values().filter(|e| e.is_player).count();
    let inv = &app.world.inventory;
    mrow(
        f,
        app,
        x0,
        y0,
        rows,
        inner_w,
        2,
        &format!(
            "burden {}/{}  players {players}",
            inv.encumbrance,
            150 * inv.strength
        ),
        normal(app),
    );
    let v = &app.world.vitals;
    mrow(
        f,
        app,
        x0,
        y0,
        rows,
        inner_w,
        3,
        &format!(
            "hp {}/{}  st {}/{}  mn {}/{}",
            v.health.unwrap_or(0),
            v.health_max.unwrap_or(0),
            v.stamina.unwrap_or(0),
            v.stamina_max.unwrap_or(0),
            v.mana.unwrap_or(0),
            v.mana_max.unwrap_or(0)
        ),
        normal(app),
    );
    let ping = app
        .world
        .ping_rtt_ms
        .map(|ms| format!("{ms:.0} ms"))
        .unwrap_or_else(|| "????".to_string());
    let c = &app.world.combat;
    let height = match c.height {
        1 => "low",
        3 => "high",
        _ => "med",
    };
    mrow(f, app, x0, y0, rows, inner_w, 4, &format!("ping {ping}  {}", crate::app::combat_mode_name(c.mode)), normal(app));
    mrow(f, app, x0, y0, rows, inner_w, 5, &format!("atk h{height} {}%  stance {}", (c.power * 100.0).round() as u32, stance_name(c.stance)), normal(app));
    let vitae = app.world.vitals.vitae.map(|v| format!("{:.0}%", v * 100.0)).unwrap_or_else(|| "-".to_string());
    mrow(f, app, x0, y0, rows, inner_w, 6, &format!("xp {}  vitae {vitae}", app.world.vitals.xp), normal(app));
}

/// The display name of a stance motion command.
fn stance_name(cmd: u32) -> &'static str {
    match cmd {
        acproto::motion::command::READY => "ready",
        acproto::motion::command::SITTING => "sitting",
        acproto::motion::command::CROUCH => "crouching",
        acproto::motion::command::SLEEPING => "sleeping",
        _ => "none",
    }
}

/// The Examine widget: the appraised target's identity, item metadata,
/// creature / weapon / armor facets, and the description (word-wrapped).
fn draw_examine(f: &mut Frame, app: &App, x0: u16, y0: u16, rows: usize, inner_w: usize) {
    let ex = &app.world.examine;
    if !ex.valid {
        mnote(f, app, x0, y0, rows, inner_w, 0, "nothing examined yet");
        mrow(f, app, x0, y0, rows, inner_w, 1, "target something, press E", normal(app));
        return;
    }
    let mut i = 0usize;
    mrow(f, app, x0, y0, rows, inner_w, i, &ex.name, focus(app));
    i += 1;
    let mut meta: Vec<String> = Vec::new();
    if ex.level != 0 {
        meta.push(format!("lvl {}", ex.level));
    }
    if ex.value != 0 {
        meta.push(format!("value {}", ex.value));
    }
    if ex.encumbrance != 0 {
        meta.push(format!("wt {}", ex.encumbrance));
    }
    if ex.bonded {
        meta.push("bonded".to_string());
    }
    if ex.attuned {
        meta.push("attuned".to_string());
    }
    if !meta.is_empty() {
        mrow(f, app, x0, y0, rows, inner_w, i, &meta.join("  "), normal(app));
        i += 1;
    }
    if ex.max_health > 0 {
        mrow(f, app, x0, y0, rows, inner_w, i, &format!("hp {}/{}", ex.health, ex.max_health), normal(app));
        i += 1;
        if ex.attrs.iter().any(|&a| a != 0) {
            mrow(
                f,
                app,
                x0,
                y0,
                rows,
                inner_w,
                i,
                &format!("str {}  end {}  quic {}", ex.attrs[0], ex.attrs[1], ex.attrs[2]),
                normal(app),
            );
            i += 1;
            mrow(
                f,
                app,
                x0,
                y0,
                rows,
                inner_w,
                i,
                &format!("coord {}  focus {}  self {}", ex.attrs[3], ex.attrs[4], ex.attrs[5]),
                normal(app),
            );
            i += 1;
        }
        if ex.vitals[0] != 0 || ex.vitals[1] != 0 {
            mrow(
                f,
                app,
                x0,
                y0,
                rows,
                inner_w,
                i,
                &format!("stam {}  mana {}", ex.vitals[0], ex.vitals[1]),
                normal(app),
            );
            i += 1;
        }
    }
    if ex.damage_min > 0 {
        mrow(
            f,
            app,
            x0,
            y0,
            rows,
            inner_w,
            i,
            &format!("dmg {}  skill {}", ex.damage_min, ex.skill),
            normal(app),
        );
        i += 1;
        if ex.damage_var > 0 {
            mrow(f, app, x0, y0, rows, inner_w, i, &format!("variance {}%", ex.damage_var), normal(app));
            i += 1;
        }
    }
    if ex.resist.iter().any(|&r| r != 0) {
        mrow(
            f,
            app,
            x0,
            y0,
            rows,
            inner_w,
            i,
            &format!("rl s{} p{} b{}", ex.resist[0], ex.resist[1], ex.resist[2]),
            normal(app),
        );
        i += 1;
        mrow(
            f,
            app,
            x0,
            y0,
            rows,
            inner_w,
            i,
            &format!("rl c{} f{} a{}", ex.resist[3], ex.resist[4], ex.resist[5]),
            normal(app),
        );
        i += 1;
        mrow(
            f,
            app,
            x0,
            y0,
            rows,
            inner_w,
            i,
            &format!("rl n{} e{}", ex.resist[6], ex.resist[7]),
            normal(app),
        );
        i += 1;
    }
    if ex.armor_levels.iter().any(|&a| a != 0) {
        mrow(
            f,
            app,
            x0,
            y0,
            rows,
            inner_w,
            i,
            &format!("al {}", ex.armor_levels.iter().map(|a| a.to_string()).collect::<Vec<_>>().join("/")),
            normal(app),
        );
        i += 1;
    }
    for wl in wrap_text(&ex.description, inner_w) {
        mrow(f, app, x0, y0, rows, inner_w, i, &wl, normal(app));
        i += 1;
    }
}

/// Word-wrap text to a width (hard-wrap over-long words; paragraphs on \n).
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            if line.is_empty() {
                line.push_str(word);
            } else if line.len() + 1 + word.len() <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::take(&mut line));
                line.push_str(word);
            }
        }
        out.push(line);
    }
    out
}

/// Comma-group a non-negative integer (1200 -> "1,200").
fn commas(n: u32) -> String {
    let s = n.to_string();
    let bytes: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for (i, ch) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*ch);
    }
    out
}

// ---------------------------------------------------------------- selector / slot bars

/// The first glyph's column in the one-row selector: the glyphs are centered
/// in the right column (left-aligned with one border cell when they don't
/// fit). Shared by the draw pass and the click hit-testing.
fn selector_start(x0: u16, x1: u16, n: usize) -> u16 {
    let wcol = x1.saturating_sub(x0) as usize;
    x0 + if n <= wcol - 2 { ((wcol - n) / 2) as u16 } else { 1 }
}

/// The small selector: one Nerd Font icon per multi widget on the multi
/// box's bottom border row, the current one reversed.
fn draw_selector(f: &mut Frame, app: &App, lb: WLayout, wcols: u16) {
    let start = selector_start(lb.right_x, wcols, 14);
    for i in 0..14 {
        put(
            f,
            lb.multi_y1,
            start + i as u16,
            &WIDGET_GLYPHS[i].to_string(),
            if i == app.multi_sel { reverse(app) } else { normal(app) },
        );
    }
}

/// The glyph column of each slot in a spell/hotkey slot bar (the label at
/// x0, the bar width w): slot i sits two columns after the previous slot and
/// only slots that fit inside the bar are drawn. Shared by the draw pass and
/// the click hit-testing.
fn slot_cols(x0: u16, w: u16, label: &str) -> Vec<u16> {
    let mut out = Vec::new();
    let mut c = (x0 + label.len() as u16) as usize;
    for _i in 0..SLOTS.len() {
        if c + 2 > (x0 + w) as usize {
            break;
        }
        out.push((c + 1) as u16);
        c += 2;
    }
    out
}

/// One spell/hotkey slot bar (right column, bottom of the layout). Each
/// slot shows a filled marker (`values[i]`) or a dash when empty; the
/// selected slot is reversed.
fn draw_slot_bar(f: &mut Frame, app: &mut App, row: u16, lb: WLayout, label: &str, sel: usize, values: &[char]) {
    put(f, row, lb.right_x, label, normal(app));
    for (i, gc) in slot_cols(lb.right_x, lb.right_w, label).iter().enumerate() {
        put(f, row, gc - 1, " ", normal(app));
        let c = values.get(i).copied().unwrap_or('-');
        put(f, row, *gc, &c.to_string(), if i == sel { reverse(app) } else { normal(app) });
    }
}
// ---------------------------------------------------------------- vendor

/// A selectable row in the vendor list (stock on Items, the pending buy
/// order on Buy, the sellable inventory on Sell).
#[derive(Clone)]
struct StripItem {
    guid: u32,
    item_type: u32,
    amount: u32,
    price: u32,
}

/// What the player pays for one unit of the stock item: the server's
/// per-item price when it has one, else the vendor's sell rate on its value.
fn buy_cost(v: &VendorState, it: &VendorItem) -> u32 {
    if it.buy_price > 0.0 {
        it.buy_price.max(1.0) as u32
    } else {
        (v.sell_mult * it.value as f32 - 0.1).ceil().max(1.0) as u32
    }
}

/// What the vendor pays per unit of value when the player sells.
fn sell_payout(v: &VendorState, value: u32) -> u32 {
    (v.buy_mult * value as f32 + 0.1).floor().max(1.0) as u32
}

/// An item's display name (the record's name, else its item type's name).
fn item_display_name(name: &str, item_type: u32) -> String {
    if name.is_empty() {
        item_type_name(item_type).to_string()
    } else {
        name.to_string()
    }
}

/// The vendor's name for the box title (never a raw guid).
fn vendor_name(app: &App) -> String {
    entity_name(app, app.world.vendor.guid)
}

/// The vendor shop's category list as item types: 0 ("All") + one per
/// distinct item type in the stock, ascending (the server sends no category
/// tabs; this is a client-side filter).
fn vendor_categories(v: &VendorState) -> Vec<u32> {
    let mut seen: Vec<u32> = v.items.iter().map(|it| it.item_type).collect();
    seen.sort_unstable();
    seen.dedup();
    let mut cats = vec![0u32];
    cats.extend(seen);
    cats
}

/// The label of a category's item type.
fn vendor_cat_label(cat: u32) -> &'static str {
    if cat == 0 {
        "All"
    } else {
        item_type_name(cat)
    }
}

/// The list row's contents for the active tab: the vendor's stock (Items,
/// category-filtered), the pending buy order (Buy), or the sellable
/// inventory (Sell).
fn vendor_list(app: &App) -> Vec<StripItem> {
    let v = &app.world.vendor;
    if v.guid == 0 {
        return Vec::new();
    }
    match v.tab {
        0 => v
            .items
            .iter()
            .filter(|it| v.category == 0 || it.item_type == v.category)
            .map(|it| StripItem {
                guid: it.guid,
                item_type: it.item_type,
                amount: it.stack_size.max(1),
                price: buy_cost(v, it),
            })
            .collect(),
        1 => v
            .buying
            .iter()
            .filter_map(|(guid, amt)| {
                v.items.iter().find(|it| &it.guid == guid).map(|it| StripItem {
                    guid: it.guid,
                    item_type: it.item_type,
                    amount: *amt,
                    price: buy_cost(v, it),
                })
            })
            .collect(),
        _ => app
            .world
            .inventory
            .pack
            .iter()
            .filter(|it| it.value > 0)
            .map(|it| StripItem {
                guid: it.guid,
                item_type: it.item_type,
                amount: it.stack_size.max(1),
                price: sell_payout(v, it.value),
            })
            .collect(),
    }
}

/// The vendor list row's page: a fixed window of entries (shared by the
/// draw and the press pass, so ‹/› paging and hit-testing agree).
struct VPage {
    items: Vec<StripItem>,
    start: usize,
    max_start: usize,
    has_prev: bool,
    has_next: bool,
}

fn vendor_page(app: &App) -> VPage {
    let list = vendor_list(app);
    let max_start = list.len().saturating_sub(VENDOR_LIST_PAGE);
    let start = app.world.vendor.page.min(max_start);
    let end = (start + VENDOR_LIST_PAGE).min(list.len());
    VPage {
        items: list[start..end].to_vec(),
        start,
        max_start,
        has_prev: start > 0,
        has_next: start + VENDOR_LIST_PAGE < list.len(),
    }
}

/// One styled segment of a put_segs row (the draw pass and the click
/// hit-testing share it).
struct Seg {
    text: String,
    style: SegStyle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SegStyle {
    Normal,
    Focus,
    Reverse,
    Yellow,
}

fn seg(text: &str, style: SegStyle) -> Seg {
    Seg {
        text: text.to_string(),
        style,
    }
}

fn seg_style(app: &App, s: SegStyle) -> Style {
    match s {
        SegStyle::Normal => normal(app),
        SegStyle::Focus => focus(app),
        SegStyle::Reverse => reverse(app),
        SegStyle::Yellow => yellow(app),
    }
}

/// Draw a run of styled segments left-to-right from `base`, truncating at
/// `base + max_w` (never wraps).
fn put_segs(f: &mut Frame, app: &App, row: u16, base: u16, max_w: u16, segs: &[Seg]) {
    let mut c = base;
    for s in segs {
        if (c - base) >= max_w {
            break;
        }
        let take = s.text.len().min(max_w as usize - (c - base) as usize);
        if take == 0 {
            break;
        }
        put(f, row, c, &s.text[..take], seg_style(app, s.style));
        c += take as u16;
    }
}

/// The [start, end) column ranges of each segment (same left-to-right
/// truncation at the box edge as the draw pass) — shared by the click
/// hit-testing.
fn seg_ranges(segs: &[Seg], base: u16, max_w: u16) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    let mut c = base;
    for s in segs {
        if (c - base) >= max_w {
            break;
        }
        let take = s.text.len().min(max_w as usize - (c - base) as usize);
        if take == 0 {
            break;
        }
        out.push((c, c + take as u16));
        c += take as u16;
    }
    out
}

/// The segment index under column `c` on a put_segs-drawn row, or `None`.
fn seg_at(segs: &[Seg], base: u16, max_w: u16, c: u16) -> Option<usize> {
    seg_ranges(segs, base, max_w).iter().position(|&(a, b)| c >= a && c < b)
}

const VENDOR_ITEMS_BUTTONS: [&str; 2] = ["Buy", "Add"];
const VENDOR_BUY_BUTTONS: [&str; 4] = ["Buy", "Buy All", "Remove", "Clear"];
const VENDOR_SELL_BUTTONS: [&str; 5] = ["Add", "Sell", "Sell All", "Remove", "Clear"];
const TRADE_BUTTONS: [&str; 5] = ["Add", "Accept", "Decline", "Reset", "X"];
const LOOT_BUTTONS: [&str; 3] = ["Take", "Take All", "X"];

/// The vendor box's per-tab action buttons (row y0+5), in retail order.
fn vendor_buttons(tab: usize) -> &'static [&'static str] {
    match tab {
        1 => &VENDOR_BUY_BUTTONS,
        2 => &VENDOR_SELL_BUTTONS,
        _ => &VENDOR_ITEMS_BUTTONS,
    }
}

/// A button row's segments: one [label] per button, 1-space separated
/// (shared by the draw pass and the press hit-testing).
fn button_segs(buttons: &[&str]) -> Vec<Seg> {
    buttons
        .iter()
        .enumerate()
        .map(|(i, b)| {
            seg(
                if i == 0 {
                    format!("[{b}]")
                } else {
                    format!(" [{b}]")
                }
                .as_str(),
                SegStyle::Normal,
            )
        })
        .collect()
}

/// The vendor tab row's segments (row y0+1): the three tabs (retail order).
fn vendor_tab_segs(app: &App) -> Vec<Seg> {
    let tabs = ["Items", "Buy", "Sell"];
    tabs.iter()
        .enumerate()
        .map(|(i, t)| {
            seg(
                if i == 0 {
                    format!("[{t}]")
                } else {
                    format!(" [{t}]")
                }
                .as_str(),
                if i == app.world.vendor.tab {
                    SegStyle::Reverse
                } else {
                    SegStyle::Normal
                },
            )
        })
        .collect()
}

/// The vendor list row's segments (row y0+2): the ‹/› page markers (only
/// when the list overflows), one glyph per entry (the targeted one
/// reversed; on the Sell tab the listed ones reversed too). Items carry no
/// text — the unified target's name shows in the status bar.
fn vendor_list_segs(app: &App) -> Vec<Seg> {
    let page = vendor_page(app);
    if page.items.is_empty() {
        return vec![seg("(none)", SegStyle::Normal)];
    }
    let mut segs = Vec::new();
    let mut first = true;
    if page.has_prev {
        segs.push(seg("‹", SegStyle::Focus));
        first = false;
    }
    for it in &page.items {
        let listed =
            app.world.vendor.tab == 2 && app.world.vendor.selling.iter().any(|(g, _)| *g == it.guid);
        let sel = item_is_targeted(app, it.guid) || listed;
        let amt = if it.amount > 1 {
            format!(" x{}", it.amount)
        } else {
            String::new()
        };
        let txt = if first {
            format!("{}{amt}", item_glyph(it.item_type))
        } else {
            format!(" {}{amt}", item_glyph(it.item_type))
        };
        segs.push(seg(&txt, if sel { SegStyle::Reverse } else { SegStyle::Normal }));
        first = false;
    }
    if page.has_next {
        segs.push(seg(" ›", SegStyle::Focus));
    }
    segs
}

/// The summary row (row y0+3): the target's unit cost (Items), the pending
/// buy order's total (Buy), or the marked-sell total (Sell) — retail's
/// "Buying %d items worth %dp" / "Selling %d items worth %dp".
fn vendor_summary_segs(app: &App) -> Vec<Seg> {
    let v = &app.world.vendor;
    if v.tab == 0 {
        let it_price = targeted_item(app)
            .and_then(|g| vendor_list(app).iter().find(|x| x.guid == g).map(|x| x.price));
        return match it_price {
            Some(price) => vec![seg(&format!("cost: {}p each", commas(price)), SegStyle::Focus)],
            None => Vec::new(),
        };
    }
    if v.buying.is_empty() && v.tab == 1 {
        return Vec::new();
    }
    if v.selling.is_empty() && v.tab == 2 {
        return Vec::new();
    }
    let (n, total): (u32, u32) = if v.tab == 1 {
        let mut n = 0u32;
        let mut total = 0u32;
        for (g, a) in &v.buying {
            n += a;
            total += v
                .items
                .iter()
                .find(|i| &i.guid == g)
                .map(|i| buy_cost(v, i) * a)
                .unwrap_or(0);
        }
        (n, total)
    } else {
        let mut n = 0u32;
        let mut total = 0u32;
        for (g, a) in &v.selling {
            n += a;
            total += app
                .world
                .inventory
                .pack
                .iter()
                .find(|i| i.guid == *g)
                .map(|i| sell_payout(v, i.value) * a)
                .unwrap_or(0);
        }
        (n, total)
    };
    let label = if v.tab == 1 { "Buying" } else { "Selling" };
    vec![seg(&format!("{label} {n} items worth {}p", commas(total)), SegStyle::Focus)]
}

/// The purse row (row y0+4): retail's "You have %hsp" (pyreals = the
/// player's coin value).
fn vendor_purse_segs(app: &App) -> Vec<Seg> {
    let amt = app.world.inventory.coin_value.max(0) as u32;
    vec![seg(
        &format!("You have {}p", commas(amt)),
        SegStyle::Yellow,
    )]
}

/// The visible window of the category row (row y0+6): the span of
/// categories from `cat_scroll` that fits the box (after [Close] and the
/// trailing "[active]" readout), with has_prev/has_next when it overflows —
/// shared by the draw and the press pass, so ‹/› clicks, wheel turns and
/// hit-testing agree.
fn vendor_cat_window(app: &App, inner_w: u16) -> (usize, usize, bool, bool) {
    let cats = vendor_categories(&app.world.vendor);
    let active = vendor_cat_label(app.world.vendor.category);
    let mut room = (inner_w as usize)
        .saturating_sub("[Close]".len())
        .saturating_sub(2 + active.len() + 2);
    let start = app.world.vendor.cat_scroll.min(cats.len().saturating_sub(1));
    if start > 0 {
        room = room.saturating_sub(1); // "‹"
    }
    let mut end = start;
    while end < cats.len() {
        let cost = 2 + vendor_cat_label(cats[end]).len();
        let keep = if end == cats.len() - 1 { 0 } else { 2 }; // " ›" if more follow
        if room < cost + keep {
            break;
        }
        room -= cost;
        end += 1;
    }
    (start, end, start > 0, end < cats.len())
}

/// The bottom row's segments (row y0+6): [Close] and, on the Items tab, the
/// category filter — a ‹/›-marked window of the categories that fit the box
/// (a click selects one — the Items list filters by it); the trailing
/// "[active]" readout always shows the selected category, even when it is
/// scrolled out of the window.
fn vendor_bottom_segs(app: &App, inner_w: u16) -> Vec<Seg> {
    let mut segs = vec![seg("[Close]", SegStyle::Normal)];
    if app.world.vendor.tab == 0 {
        let cats = vendor_categories(&app.world.vendor);
        let (start, end, has_prev, has_next) = vendor_cat_window(app, inner_w);
        if has_prev {
            segs.push(seg("‹", SegStyle::Focus));
        }
        for i in start..end {
            let active = if app.world.vendor.category == 0 {
                i == 0
            } else {
                cats[i] == app.world.vendor.category
            };
            segs.push(seg(
                &format!("  {}", vendor_cat_label(cats[i])),
                if active {
                    SegStyle::Reverse
                } else {
                    SegStyle::Normal
                },
            ));
        }
        if has_next {
            segs.push(seg(" ›", SegStyle::Focus));
        }
        segs.push(seg(
            &format!("  [{}]", vendor_cat_label(app.world.vendor.category)),
            SegStyle::Focus,
        ));
    }
    segs
}

/// The vendor shop (8-row box). Row 1 tabs, row 2 the item list (glyphs),
/// row 3 the summary (unit cost / order totals), row 4 the purse, row 5 the
/// per-tab action buttons, row 6 [Close] + the category filter (Items).
/// Items are identified by the unified target — the status bar carries the
/// name; actions are buttons (the shop takes no keys).
fn draw_vendor(f: &mut Frame, app: &mut App, lb: WLayout) {
    let inner_w = lb.left_w - 2;
    let inner_x: u16 = 1;
    // Keep the unified target inside the list row's page (retail ScrollToShow).
    if let Some(guid) = targeted_item(app) {
        let list = vendor_list(app);
        if let Some(idx) = list.iter().position(|x| x.guid == guid) {
            let max_start = list.len().saturating_sub(VENDOR_LIST_PAGE);
            let start = app.world.vendor.page.min(max_start);
            if idx < start || idx >= start + VENDOR_LIST_PAGE {
                app.world.vendor.page = idx.min(max_start);
            }
        }
    }
    put_segs(f, app, lb.widget_y0 + 1, inner_x, inner_w, &vendor_tab_segs(app));
    put_segs(f, app, lb.widget_y0 + 2, inner_x, inner_w, &vendor_list_segs(app));
    put_segs(f, app, lb.widget_y0 + 3, inner_x, inner_w, &vendor_summary_segs(app));
    put_segs(f, app, lb.widget_y0 + 4, inner_x, inner_w, &vendor_purse_segs(app));
    put_segs(
        f,
        app,
        lb.widget_y0 + 5,
        inner_x,
        inner_w,
        &button_segs(vendor_buttons(app.world.vendor.tab)),
    );
    put_segs(
        f,
        app,
        lb.widget_y0 + 6,
        inner_x,
        inner_w,
        &vendor_bottom_segs(app, inner_w),
    );
}

// ---------------------------------------------------------------- vendor press

/// A left-button press inside the vendor widget box: a click on a tab
/// switches it, on the list targets an item (or pages ‹/›), on a button
/// runs the tab's action, on the bottom row closes or picks a category.
/// Any click other than Close disarms the pending-leave confirm.
fn vendor_press(app: &mut App, lb: WLayout, r: u16, c: u16) {
    let y = r - lb.widget_y0;
    let inner_x: u16 = 1;
    let inner_w = lb.left_w - 2;
    match y {
        1 => {
            app.confirm_close_vendor = false;
            if let Some(i) = seg_at(&vendor_tab_segs(app), inner_x, inner_w, c) {
                if i < 3 {
                    app.world.vendor.tab = i;
                    app.world.flash = Some(["Items", "Buy", "Sell"][i].into());
                }
            }
        }
        2 => {
            app.confirm_close_vendor = false;
            let page = vendor_page(app);
            let ranges = seg_ranges(&vendor_list_segs(app), inner_x, inner_w);
            let off = usize::from(page.has_prev);
            for (i, it) in page.items.iter().enumerate() {
                let Some(&(a, b)) = ranges.get(i + off) else {
                    continue;
                };
                if c >= a && c < b {
                    app.set_item_target(it.guid, TARGET_ITEM_SHOP);
                    app.world.vendor.sel = i;
                    return;
                }
            }
            if page.has_prev {
                let (a, b) = ranges[0];
                if c >= a && c < b {
                    app.world.vendor.page = page.start.saturating_sub(VENDOR_LIST_PAGE);
                    return;
                }
            }
            if page.has_next {
                let (a, b) = *ranges.last().unwrap();
                if c >= a && c < b {
                    app.world.vendor.page = (page.start + VENDOR_LIST_PAGE).min(page.max_start);
                }
            }
        }
        3 | 4 => {
            app.confirm_close_vendor = false; // text rows: a click just disarms
        }
        5 => {
            app.confirm_close_vendor = false;
            let i = seg_at(&button_segs(vendor_buttons(app.world.vendor.tab)), inner_x, inner_w, c);
            if let Some(i) = i {
                vendor_button_action(app, i);
            }
        }
        6 => {
            if app.world.vendor.tab == 0 {
                let cats = vendor_categories(&app.world.vendor);
                let (start, end, has_prev, has_next) = vendor_cat_window(app, inner_w);
                let ranges = seg_ranges(&vendor_bottom_segs(app, inner_w), inner_x, inner_w);
                let Some(&(a, b)) = ranges.first() else {
                    return;
                };
                if c >= a && c < b {
                    app.request_close_vendor();
                    return;
                }
                if has_prev {
                    let (a, b) = ranges[1];
                    if c >= a && c < b {
                        let page = (end - start).max(1);
                        app.world.vendor.cat_scroll = start.saturating_sub(page);
                        return;
                    }
                }
                let off = 1 + usize::from(has_prev);
                for k in start..end {
                    let Some(&(a, b)) = ranges.get(off + (k - start)) else {
                        continue;
                    };
                    if c >= a && c < b {
                        app.confirm_close_vendor = false;
                        app.world.vendor.category = cats[k];
                        app.world.flash =
                            Some(format!("category: {}", vendor_cat_label(cats[k])));
                        return;
                    }
                }
                if has_next {
                    let (a, b) = *ranges.last().unwrap();
                    if c >= a && c < b {
                        let page = (end - start).max(1);
                        app.world.vendor.cat_scroll = (start + page).min(cats.len() - 1);
                    }
                }
            } else if let Some(&(a, b)) =
                seg_ranges(&vendor_bottom_segs(app, inner_w), inner_x, inner_w).first()
            {
                if c >= a && c < b {
                    app.request_close_vendor();
                }
            }
        }
        _ => {}
    }
}

/// The vendor box's list item under a cell (row 2), or `None` — the
/// right-click's vendor-item target + examine. Mirrors vendorPress's
/// hit-testing geometry (the list row's page, ‹/› included).
fn vendor_item_at(app: &App, lb: WLayout, r: u16, c: u16) -> Option<u32> {
    if app.world.vendor.guid == 0 || r - lb.widget_y0 != 2 {
        return None;
    }
    let inner_x: u16 = 1;
    let inner_w = lb.left_w - 2;
    let page = vendor_page(app);
    let ranges = seg_ranges(&vendor_list_segs(app), inner_x, inner_w);
    let off = usize::from(page.has_prev);
    for (i, it) in page.items.iter().enumerate() {
        let Some(&(a, b)) = ranges.get(i + off) else {
            continue;
        };
        if c >= a && c < b {
            return Some(it.guid);
        }
    }
    None
}
/// A left-button press on the vendor box's button row (row y0+5): dispatch
/// to the tab's actions (Items: Buy / Add; Buy: Buy / Buy All / Remove /
/// Clear; Sell: Add / Sell / Sell All / Remove / Clear).
fn vendor_button_action(app: &mut App, i: usize) {
    match app.world.vendor.tab {
        0 => match i {
            0 => vendor_buy_one(app),
            _ => vendor_add_to_buy(app),
        },
        1 => match i {
            0 => vendor_buy_entry(app),
            1 => vendor_buy_all(app),
            2 => vendor_remove_buy(app),
            _ => vendor_clear_buy(app),
        },
        _ => match i {
            0 => vendor_add_to_sell(app),
            1 => vendor_sell_entry(app),
            2 => vendor_sell_all(app),
            3 => vendor_remove_sell(app),
            _ => vendor_clear_sell(app),
        },
    }
}

/// Items [Buy]: buy one of the targeted stock item now.
fn vendor_buy_one(app: &mut App) {
    let guid = match targeted_item(app) {
        Some(g) => g,
        None => {
            app.world.flash = Some("nothing to buy".into());
            return;
        }
    };
    let (price, name) = match app.world.vendor.items.iter().find(|i| i.guid == guid) {
        Some(it) => (buy_cost(&app.world.vendor, it), item_display_name(&it.name, it.item_type)),
        None => {
            app.world.flash = Some("target a list item first (click it)".into());
            return;
        }
    };
    if price as i64 > app.world.inventory.coin_value {
        app.world.flash = Some("You don't have enough money".into());
        return;
    }
    // fizzle sends the whole staged list: send just this one, keep the rest.
    let saved = std::mem::take(&mut app.world.vendor.buying);
    app.world.vendor.buying.push((guid, 1));
    app.vendor_buy();
    app.world.vendor.buying = saved;
    app.world.flash = Some(format!("buying {name}"));
}

/// Items [Add]: stage one more of the targeted stock item on the buy order
/// (5000 max per entry).
fn vendor_add_to_buy(app: &mut App) {
    let guid = match targeted_item(app) {
        Some(g) => g,
        None => {
            app.world.flash = Some("nothing to add".into());
            return;
        }
    };
    let name = match app.world.vendor.items.iter().find(|i| i.guid == guid) {
        Some(it) => item_display_name(&it.name, it.item_type),
        None => {
            app.world.flash = Some("target a list item first (click it)".into());
            return;
        }
    };
    let buying = &mut app.world.vendor.buying;
    match buying.iter_mut().find(|(g, _)| *g == guid) {
        Some(e) => {
            if e.1 + 1 > 5000 {
                app.world.flash = Some(
                    "I can't possibly sell you that much! Please be a little more reasonable.".into(),
                );
                return;
            }
            e.1 += 1;
        }
        None => buying.push((guid, 1)),
    }
    app.world.flash = Some(format!("buy: {name}"));
}

/// Buy [Buy]: buy the targeted buy entry now (then remove it).
fn vendor_buy_entry(app: &mut App) {
    if app.world.vendor.buying.is_empty() {
        app.world.flash = Some("buy list empty".into());
        return;
    }
    let guid = match targeted_item(app) {
        Some(g) if app.world.vendor.buying.iter().any(|(x, _)| *x == g) => g,
        _ => {
            app.world.flash = Some("target a buy entry first (click it)".into());
            return;
        }
    };
    let (amt, price, name) = {
        let v = &app.world.vendor;
        let amt = v.buying.iter().find(|(g, _)| *g == guid).map(|(_, a)| *a).unwrap_or(1);
        match v.items.iter().find(|i| i.guid == guid) {
            Some(it) => (amt, buy_cost(v, it), item_display_name(&it.name, it.item_type)),
            None => (amt, 0, "?".to_string()),
        }
    };
    if price as i64 * amt as i64 > app.world.inventory.coin_value {
        app.world.flash = Some("You don't have enough money".into());
        return;
    }
    // fizzle sends the whole staged list: send just this entry, drop it.
    let mut saved = std::mem::take(&mut app.world.vendor.buying);
    saved.retain(|(g, _)| *g != guid);
    app.world.vendor.buying.push((guid, amt));
    app.vendor_buy();
    app.world.vendor.buying = saved;
    app.world.flash = Some(format!("buying {amt} {name}"));
}

/// Buy [Buy All]: buy the whole pending order now (clears it).
fn vendor_buy_all(app: &mut App) {
    if app.world.vendor.buying.is_empty() {
        app.world.flash = Some("buy list empty".into());
        return;
    }
    let (n, total) = {
        let v = &app.world.vendor;
        let mut n = 0u32;
        let mut total = 0u32;
        for (g, a) in &v.buying {
            n += a;
            total += v
                .items
                .iter()
                .find(|i| &i.guid == g)
                .map(|i| buy_cost(v, i) * a)
                .unwrap_or(0);
        }
        (n, total)
    };
    if total as i64 > app.world.inventory.coin_value {
        app.world.flash = Some("You don't have enough money".into());
        return;
    }
    app.vendor_buy(); // sends the whole order and clears it
    app.world.flash = Some(format!("buying {n} item(s)"));
}

/// Buy [Remove]: drop the targeted entry from the pending order.
fn vendor_remove_buy(app: &mut App) {
    if app.world.vendor.buying.is_empty() {
        app.world.flash = Some("buy list empty".into());
        return;
    }
    let guid = match targeted_item(app) {
        Some(g) if app.world.vendor.buying.iter().any(|(x, _)| *x == g) => g,
        _ => {
            app.world.flash = Some("target a buy entry first (click it)".into());
            return;
        }
    };
    let name = app
        .world
        .vendor
        .items
        .iter()
        .find(|i| i.guid == guid)
        .map(|i| item_display_name(&i.name, i.item_type))
        .unwrap_or_else(|| "?".to_string());
    app.world.vendor.buying.retain(|(g, _)| *g != guid);
    app.world.flash = Some(format!("removed {name}"));
}

/// Buy [Clear]: drop the whole pending order.
fn vendor_clear_buy(app: &mut App) {
    if app.world.vendor.buying.is_empty() {
        app.world.flash = Some("buy list empty".into());
        return;
    }
    app.world.vendor.buying.clear();
    app.world.flash = Some("buy list cleared".into());
}

/// Sell [Add]: list one stack of the targeted pack item for sale.
fn vendor_add_to_sell(app: &mut App) {
    let guid = match targeted_item(app) {
        Some(g) => g,
        None => {
            app.world.flash = Some("nothing to sell".into());
            return;
        }
    };
    let (name, stack) = match app.world.inventory.pack.iter().find(|i| i.guid == guid) {
        Some(it) if it.value > 0 => (
            item_display_name(&it.name, it.item_type),
            it.stack_size.max(1),
        ),
        _ => {
            app.world.flash = Some("target a list item first (click it)".into());
            return;
        }
    };
    if app.world.vendor.selling.iter().any(|(g, _)| *g == guid) {
        app.world.flash = Some("already listed".into());
        return;
    }
    app.world.vendor.selling.push((guid, stack));
    app.world.flash = Some(format!("sell: {name}"));
}

/// Sell [Sell]: sell the targeted listed item now (then unmark it).
fn vendor_sell_entry(app: &mut App) {
    if app.world.vendor.selling.is_empty() {
        app.world.flash = Some("sell list empty".into());
        return;
    }
    let guid = match targeted_item(app) {
        Some(g) if app.world.vendor.selling.iter().any(|(x, _)| *x == g) => g,
        _ => {
            app.world.flash = Some("target a listed item first (click it)".into());
            return;
        }
    };
    let (amt, name) = {
        let amt = app
            .world
            .vendor
            .selling
            .iter()
            .find(|(g, _)| *g == guid)
            .map(|(_, a)| *a)
            .unwrap_or(1);
        let name = app
            .world
            .inventory
            .pack
            .iter()
            .find(|i| i.guid == guid)
            .map(|i| item_display_name(&i.name, i.item_type))
            .unwrap_or_else(|| "?".to_string());
        (amt, name)
    };
    // fizzle sends the whole marked list: send just this entry, unmark it.
    let mut saved = std::mem::take(&mut app.world.vendor.selling);
    saved.retain(|(g, _)| *g != guid);
    app.world.vendor.selling.push((guid, amt));
    app.vendor_sell();
    app.world.vendor.selling = saved;
    app.world.flash = Some(format!("selling {amt} {name}"));
}

/// Sell [Sell All]: sell the whole marked list now (clears it).
fn vendor_sell_all(app: &mut App) {
    if app.world.vendor.selling.is_empty() {
        app.world.flash = Some("sell list empty".into());
        return;
    }
    let n = app.world.vendor.selling.len();
    app.vendor_sell(); // sends the whole list and clears it
    app.world.flash = Some(format!("selling {n} item(s)"));
}

/// Sell [Remove]: unmark the targeted item from the sell list.
fn vendor_remove_sell(app: &mut App) {
    if app.world.vendor.selling.is_empty() {
        app.world.flash = Some("sell list empty".into());
        return;
    }
    let guid = match targeted_item(app) {
        Some(g) if app.world.vendor.selling.iter().any(|(x, _)| *x == g) => g,
        _ => {
            app.world.flash = Some("target a listed item first (click it)".into());
            return;
        }
    };
    let name = app
        .world
        .inventory
        .pack
        .iter()
        .find(|i| i.guid == guid)
        .map(|i| item_display_name(&i.name, i.item_type))
        .unwrap_or_else(|| "?".to_string());
    app.world.vendor.selling.retain(|(g, _)| *g != guid);
    app.world.flash = Some(format!("removed {name}"));
}

/// Sell [Clear]: unmark the whole sell list.
fn vendor_clear_sell(app: &mut App) {
    if app.world.vendor.selling.is_empty() {
        app.world.flash = Some("sell list empty".into());
        return;
    }
    app.world.vendor.selling.clear();
    app.world.flash = Some("sell list cleared".into());
}
// ---------------------------------------------------------------- trade

/// The trade box (8 rows): row 1 the player's offered items, row 3 the
/// other player's, row 5 the partner's name, row 6 the buttons (retail
/// order: Add / Accept / Decline / Reset / X). Items are the unified
/// target; the status bar carries the name.
fn draw_trade(f: &mut Frame, app: &App, lb: WLayout) {
    let y = lb.widget_y0;
    put(f, y + 1, 1, "you:", focus(app));
    let mut col: u16 = 6;
    for &guid in &app.world.trade.our {
        if col + 1 > lb.left_w - 1 {
            break;
        }
        put(
            f,
            y + 1,
            col,
            &item_glyph_of_guid(app, guid).to_string(),
            if item_is_targeted(app, guid) {
                reverse(app)
            } else {
                normal(app)
            },
        );
        col += 2;
    }
    put(f, y + 3, 1, "them:", focus(app));
    col = 7;
    for &guid in &app.world.trade.their {
        if col + 1 > lb.left_w - 1 {
            break;
        }
        put(
            f,
            y + 3,
            col,
            &item_glyph_of_guid(app, guid).to_string(),
            if item_is_targeted(app, guid) {
                reverse(app)
            } else {
                normal(app)
            },
        );
        col += 2;
    }
    put(
        f,
        y + 5,
        1,
        trunc(&format!("trading with {}", app.world.trade.partner_name), (lb.left_w - 2) as usize),
        normal(app),
    );
    put_segs(f, app, y + 6, 1, lb.left_w - 2, &button_segs(&TRADE_BUTTONS));
}

/// The trade box's item under a cell (rows 1 / 3: ours / theirs), or
/// `None` — mirrors the draw pass's glyph columns.
fn trade_item_at(app: &App, lb: WLayout, r: u16, c: u16) -> Option<(u32, usize)> {
    if app.world.trade.partner == 0 {
        return None;
    }
    let y = r - lb.widget_y0;
    let (guids, x0): (&[u32], u16) = match y {
        1 => (&app.world.trade.our, 6),
        3 => (&app.world.trade.their, 7),
        _ => return None,
    };
    for (i, &g) in guids.iter().enumerate() {
        let col = x0 + 2 * i as u16;
        if col + 1 > lb.left_w - 2 {
            break;
        }
        if c == col {
            return Some((g, i));
        }
    }
    None
}

/// A left-button press inside the trade widget box: a click on an item
/// targets it (drag-drop adds it), on a button runs the action.
fn trade_press(app: &mut App, lb: WLayout, r: u16, c: u16) {
    let y = r - lb.widget_y0;
    if y == 6 {
        let Some(i) = seg_at(&button_segs(&TRADE_BUTTONS), 1, lb.left_w - 2, c) else {
            return;
        };
        match i {
            0 => {
                let Some(item) = targeted_item(app) else {
                    app.world.flash = Some("target an item first (click it in the pack)".into());
                    return;
                };
                let name = item_name(app, item);
                app.trade_add(item, 0);
                app.world.flash = Some(format!("added {name} to trade"));
            }
            1 => {
                app.trade_accept();
                app.world.flash = Some("accepted".into());
            }
            2 => {
                app.trade_decline();
                app.world.flash = Some("declined".into());
            }
            3 => {
                app.trade_reset();
                app.world.flash = Some("trade reset".into());
            }
            _ => {
                app.trade_close();
                app.world.flash = Some("trade closed".into());
            }
        }
        return;
    }
    if let Some((guid, i)) = trade_item_at(app, lb, r, c) {
        app.set_item_target(guid, TARGET_ITEM_TRADE);
        app.world.trade.sel = i;
    }
}

// ---------------------------------------------------------------- loot

/// The loot box (8 rows): row 1 the container's name, row 2 the item
/// glyphs, row 3 the target's name + value, row 5 the count, row 6 the
/// buttons (Take / Take All / X).
fn draw_loot(f: &mut Frame, app: &App, lb: WLayout) {
    let y = lb.widget_y0;
    let cont = &app.world.container;
    put(
        f,
        y + 1,
        1,
        trunc(&format!("loot: {}", cont.name), (lb.left_w - 2) as usize),
        focus(app),
    );
    let mut col: u16 = 2;
    for it in &cont.items {
        if col + 1 > lb.left_w - 2 {
            break;
        }
        put(
            f,
            y + 2,
            col,
            &item_glyph(it.item_type).to_string(),
            if item_is_targeted(app, it.guid) {
                reverse(app)
            } else {
                normal(app)
            },
        );
        col += 2;
    }
    if let Some(g) = targeted_item(app) {
        if let Some(it) = cont.items.iter().find(|i| i.guid == g) {
            put(
                f,
                y + 3,
                1,
                trunc(
                    &format!("{} ({})", item_display_name(&it.name, it.item_type), it.value),
                    (lb.left_w - 2) as usize,
                ),
                normal(app),
            );
        }
    }
    let n = cont.items.len();
    put(
        f,
        y + 5,
        1,
        &format!("{n} item{}", if n == 1 { "" } else { "s" }),
        normal(app),
    );
    put_segs(f, app, y + 6, 1, lb.left_w - 2, &button_segs(&LOOT_BUTTONS));
}

/// The loot box's item under a cell (row 2), or `None` — mirrors the draw
/// pass's glyph columns.
fn loot_item_at(app: &App, lb: WLayout, r: u16, c: u16) -> Option<(u32, usize)> {
    if app.world.container.guid == 0 || r - lb.widget_y0 != 2 {
        return None;
    }
    let mut col: u16 = 2;
    for (i, it) in app.world.container.items.iter().enumerate() {
        if col + 1 > lb.left_w - 2 {
            break;
        }
        if c == col {
            return Some((it.guid, i));
        }
        col += 2;
    }
    None
}

/// A left-button press inside the loot widget box: a click on an item
/// targets it (drag-drop puts it in the container), on a button runs the
/// action.
fn loot_press(app: &mut App, lb: WLayout, r: u16, c: u16) {
    let y = r - lb.widget_y0;
    if y == 6 {
        let Some(i) = seg_at(&button_segs(&LOOT_BUTTONS), 1, lb.left_w - 2, c) else {
            return;
        };
        match i {
            0 => {
                let Some(item) = targeted_item(app) else {
                    app.world.flash = Some("click an item in the box first".into());
                    return;
                };
                let name = item_name(app, item);
                app.loot_take(item);
                app.world.flash = Some(format!("taking {name}"));
            }
            1 => {
                if app.world.container.items.is_empty() {
                    app.world.flash = Some("nothing to take".into());
                    return;
                }
                let n = app.world.container.items.len() as u32;
                let items: Vec<u32> = app.world.container.items.iter().map(|i| i.guid).collect();
                for g in items {
                    app.loot_take(g);
                }
                app.world.flash = Some(format!("taking {n} item(s)"));
            }
            _ => {
                app.request_close_container();
                app.world.flash = Some("container closed".into());
            }
        }
        return;
    }
    if let Some((guid, i)) = loot_item_at(app, lb, r, c) {
        app.set_item_target(guid, TARGET_ITEM_CONTAINER);
        app.world.container.sel = i;
    }
}
// ================================================================== mouse model

/// A second click on the same cell within the double-click window.
fn is_double_click(app: &App, now: Instant, pos: (u16, u16)) -> bool {
    matches!((app.last_click_at, app.last_click_pos),
        (Some(t), Some(p)) if now.duration_since(t) <= DOUBLE_CLICK && p == pos)
}

/// The item record for a guid (pack, equipped, or the open container).
fn find_item<'a>(app: &'a App, guid: u32) -> Option<&'a ItemState> {
    let inv = &app.world.inventory;
    inv.pack.iter().find(|i| i.guid == guid)
        .or_else(|| inv.equipped.iter().find(|i| i.guid == guid))
        .or_else(|| app.world.container.items.iter().find(|i| i.guid == guid))
}

/// Whether a guid is in the main pack (not equipped / not in a container).
fn in_main_pack(app: &App, guid: u32) -> bool {
    app.world.inventory.pack.iter().any(|i| i.guid == guid)
}

/// The slot kind of an inventory item the drag started on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InvSlot {
    Pack(usize),
    Doll(usize),
}

/// The inventory item under a mouse cell (paper-doll or pack grid), or
/// `None` (shared by the press and release hit-testing).
fn inv_item_at(app: &App, lb: WLayout, r: u16, c: u16, wcols: u16) -> Option<(u32, InvSlot)> {
    if app.multi_sel != INV_WIDGET {
        return None;
    }
    let inner_x0 = lb.right_x + 1;
    let inner_y0 = lb.radar_h + 1;
    if r < inner_y0 || c < inner_x0 {
        return None;
    }
    let rel_r = (r - inner_y0) as usize;
    let rel_c = (c - inner_x0) as usize;
    let inner_w = (wcols - lb.right_x - 2) as usize;
    let rows = (lb.multi_y1 - lb.radar_h - 2) as usize;
    if rel_r >= rows || rel_c >= inner_w {
        return None;
    }
    let inv = &app.world.inventory;
    if rel_r < DOLL_ROWS as usize {
        let slot_idx = DOLL_SLOTS
            .iter()
            .position(|(_id, _name, dr, dc, _bits)| *dr as usize == rel_r && *dc as usize == rel_c)?;
        let doll = doll_items(inv);
        let (guid, _dr, _dc) = doll
            .iter()
            .find(|(_g, dr, dc)| *dr as usize == rel_r && *dc as usize == rel_c)?;
        Some((*guid, InvSlot::Doll(slot_idx)))
    } else {
        let (gw, gr, _ix) = inv_grid_geom(inner_w, rows);
        if rel_r >= INV_GRID_R0 as usize && rel_r < INV_GRID_R0 as usize + gr && rel_c < gw {
            let idx = inv_scroll_for(app, gr * gw) + (rel_r - INV_GRID_R0 as usize) * gw + rel_c;
            if idx < inv.pack.len() {
                Some((inv.pack[idx].guid, InvSlot::Pack(idx)))
            } else {
                None
            }
        } else {
            None
        }
    }
}

/// The single main pack's icon column (the displayed pack), under a cell.
fn inv_pack_icon_at(lb: WLayout, r: u16, c: u16, wcols: u16) -> bool {
    let inner_x0 = lb.right_x + 1;
    let inner_y0 = lb.radar_h + 1;
    if r < inner_y0 || c < inner_x0 {
        return false;
    }
    let rel_r = (r - inner_y0) as usize;
    let rel_c = (c - inner_x0) as usize;
    let inner_w = (wcols - lb.right_x - 2) as usize;
    let rows = (lb.multi_y1 - lb.radar_h - 2) as usize;
    if rel_r >= rows || rel_c >= inner_w {
        return false;
    }
    rel_c == inner_w - 1 && rel_r == INV_GRID_R0 as usize
}

/// The index of a side pack's icon under a cell (the icon column, rows below
/// the main pack icon), or `None`.
fn side_pack_icon_at(app: &App, lb: WLayout, r: u16, c: u16, wcols: u16) -> Option<usize> {
    if app.multi_sel != INV_WIDGET {
        return None;
    }
    let inner_x0 = lb.right_x + 1;
    let inner_y0 = lb.radar_h + 1;
    if r < inner_y0 || c < inner_x0 {
        return None;
    }
    let rel_r = (r - inner_y0) as usize;
    let rel_c = (c - inner_x0) as usize;
    let inner_w = (wcols - lb.right_x - 2) as usize;
    let rows = (lb.multi_y1 - lb.radar_h - 2) as usize;
    if rel_r >= rows || rel_c >= inner_w || rel_c != inner_w - 1 {
        return None;
    }
    let idx = rel_r.checked_sub((INV_GRID_R0 as usize) + 1)?;
    (idx < app.world.inventory.packs.len()).then_some(idx)
}

/// A paper-doll structural slot (drop target for equipping), under a cell.
fn doll_drop_at(lb: WLayout, r: u16, c: u16, wcols: u16) -> bool {
    let inner_x0 = lb.right_x + 1;
    let inner_y0 = lb.radar_h + 1;
    if r < inner_y0 || c < inner_x0 {
        return false;
    }
    let rel_r = (r - inner_y0) as usize;
    let rel_c = (c - inner_x0) as usize;
    let inner_w = (wcols - lb.right_x - 2) as usize;
    let rows = (lb.multi_y1 - lb.radar_h - 2) as usize;
    if rel_r >= rows || rel_c >= inner_w {
        return false;
    }
    rel_r < DOLL_ROWS as usize
        && DOLL_SLOTS
            .iter()
            .any(|(_id, _name, dr, dc, _bits)| *dr as usize == rel_r && *dc as usize == rel_c)
}

/// The free EquipMask location for an item (the first free bit it is valid
/// for, given what is already worn), or `None` (nowhere to wear it).
fn equip_loc_for(app: &App, it: &ItemState) -> Option<u32> {
    let valid = it.valid_locs as u32;
    if valid == 0 {
        return None;
    }
    let mut occupied = 0u32;
    for e in &app.world.inventory.equipped {
        occupied |= e.cur_loc as u32;
    }
    let free = valid & !occupied;
    if free == 0 {
        return None;
    }
    Some(free & free.wrapping_neg())
}

/// The retail names of the EquipMask bits set in `loc`, joined with `/`.
fn loc_name(loc: u32) -> String {
    let names: Vec<&str> = LOC_BITS
        .iter()
        .zip(LOC_NAMES.iter())
        .filter(|(bit, _n)| loc & *bit != 0)
        .map(|(_b, n)| *n)
        .collect();
    if names.is_empty() {
        "?".to_string()
    } else {
        names.join("/")
    }
}

/// The world entity under a mouse cell (3D projection, or the tactical
/// blip). `None` when nothing is pickable there.
fn pick_world_at(app: &App, lb: WLayout, r: u16, c: u16) -> Option<u32> {
    if app.world_view {
        let avail = app
            .world3d
            .as_ref()
            .map(|w| w.available())
            .unwrap_or(false)
            && ((app.self_position().0 >> 16) & 0xFFFF) != 0;
        if avail {
            return pick_world3d(app, lb, r, c);
        }
        return pick_tactical(app, lb, r, c);
    }
    pick_tactical(app, lb, r, c)
}

/// The camera's projection basis (ported from acgeo's private render math;
/// used only for click picking — acgeo is frozen).
struct Basis {
    cam: [f32; 3],
    f: [f32; 3],
    r: [f32; 3],
    u: [f32; 3],
    focal: f32,
    sc: [f32; 2],
}

fn camera_basis(cam: &Camera, cols: usize, rows: usize, back: f32) -> Basis {
    let h = cam.heading;
    let (fx, fy) = (h.sin(), h.cos());
    let cam_x = cam.x - fx * back;
    let cam_y = cam.y - fy * back;
    let cam_z = cam.z + CAM_UP * (back / CAM_BACK);
    let (mut vfx, mut vfy, mut vfz) = (cam.x - cam_x, cam.y - cam_y, cam.z + CAM_LOOK_UP - cam_z);
    let mut fl = (vfx * vfx + vfy * vfy + vfz * vfz).sqrt();
    if fl < f32::EPSILON {
        (vfx, vfy, vfz, fl) = (0.0, 1.0, 0.0, 1.0);
    }
    vfx /= fl;
    vfy /= fl;
    vfz /= fl;
    let (mut rx, mut ry) = (vfy, -vfx);
    let mut rl = (rx * rx + ry * ry).sqrt();
    if rl < f32::EPSILON {
        (rx, ry, rl) = (1.0, 0.0, 1.0);
    }
    rx /= rl;
    ry /= rl;
    let (ux, uy, uz) = (ry * vfz, -rx * vfz, rx * vfy - ry * vfx);
    Basis {
        cam: [cam_x, cam_y, cam_z],
        f: [vfx, vfy, vfz],
        r: [rx, ry, 0.0],
        u: [ux, uy, uz],
        focal: rows as f32 * CAM_FOCAL_K,
        sc: [(cols - 1) as f32 / 2.0, (rows - 1) as f32 / 2.0],
    }
}

/// Project a world point into the pane (screen cell), `None` behind the
/// near plane. Returns `(sx, sy, zc)` with `zc` the forward depth (the
/// distance along the view ray the renderer depth-tests with).
fn project(b: &Basis, px: f32, py: f32, pz: f32) -> Option<(f32, f32, f32)> {
    let (vx, vy, vz) = (px - b.cam[0], py - b.cam[1], pz - b.cam[2]);
    let zc = vx * b.f[0] + vy * b.f[1] + vz * b.f[2];
    if zc < CAM_NEAR {
        return None;
    }
    let s = b.focal / zc;
    Some((
        b.sc[0] + (vx * b.r[0] + vy * b.r[1]) * s,
        b.sc[1] - (vx * b.u[0] + vy * b.u[1] + vz * b.u[2]) * s,
        zc,
    ))
}

/// The anchor height (world y offset) of an entity's pick point.
fn anchor_height(e: &Entity) -> f32 {
    if e.item_type == ITEM_TYPE_DOOR {
        2.0
    } else if e.is_player || e.item_type == 0 || e.item_type == ITEM_TYPE_CREATURE {
        1.5
    } else {
        0.5
    }
}

/// The approximate half-extents (radius, half-height) of an entity's body,
/// shared by the reticle and the 3D pick.
fn entity_half_extents(e: &Entity) -> (f32, f32) {
    if e.item_type == ITEM_TYPE_DOOR {
        (1.3, 2.0) // a door slab
    } else if e.item_type == ITEM_TYPE_GROUND || e.item_type == ITEM_TYPE_CONTAINER {
        (0.7, 0.7) // a ground item / container
    } else {
        (0.9, 1.4) // a humanoid
    }
}

/// The pickable 3D entity under the cell: the nearest one whose projected
/// body box covers the click (3D view only). A near body is a large box, easy
/// to hit; a thin far body is a tiny box, hit only when aimed at its dot — so
/// a click near the vanishing point no longer grabs something far off.
fn pick_world3d(app: &App, lb: WLayout, r: u16, c: u16) -> Option<u32> {
    if r < lb.world_y0 + 1 || r > lb.world_y1 - 2 || c < 1 || c > lb.left_w - 2 {
        return None;
    }
    let cols = (lb.left_w - 2) as usize;
    let rows = (lb.world_y1 - lb.world_y0 - 2) as usize;
    let (cell, sp) = app.self_position();
    if (cell >> 16) & 0xFFFF == 0 {
        return None;
    }
    let heading = app.movement.as_ref().map(|m| m.heading).unwrap_or(0.0);
    let cam = Camera {
        x: wrap_block_coord(sp.x),
        y: wrap_block_coord(sp.y),
        z: sp.z,
        heading,
    };
    // Use the renderer's live camera distance (it tracks the wheel zoom), so
    // the pick projection lines up with what's on screen — not the default.
    let back = app
        .world3d
        .as_ref()
        .map(|w| w.camera_back())
        .unwrap_or(CAM_BACK);
    let basis = camera_basis(&cam, cols, rows, back);
    let px = (c - 1) as f32;
    let py = (r - lb.world_y0 - 1) as f32;
    let w3d = app.world3d.as_ref();
    // (guid, depth) of the nearest body box that covers the click.
    let mut best: Option<(u32, f32)> = None;
    for e in app.world.entities.values() {
        if Some(e.guid) == app.self_guid {
            continue;
        }
        let pickable = e.is_player
            || e.item_type == ITEM_TYPE_CREATURE
            || e.item_type == ITEM_TYPE_GROUND
            || e.item_type == ITEM_TYPE_DOOR;
        if !pickable {
            continue;
        }
        let (r_h, half_h) = entity_half_extents(e);
        let ax = wrap_block_coord(e.pos.x);
        let ay = wrap_block_coord(e.pos.y);
        let z0 = e.pos.z;
        let z1 = e.pos.z + 2.0 * half_h;
        let mut minx = f32::INFINITY;
        let mut miny = f32::INFINITY;
        let mut maxx = f32::NEG_INFINITY;
        let mut maxy = f32::NEG_INFINITY;
        // Project the body box's 8 corners into an axis-aligned screen box
        // (the render wraps the actor origin, not its local vertices, so the
        // corners are the wrapped origin plus an unwrapped offset).
        for (cx, cy) in [
            (ax - r_h, ay - r_h),
            (ax + r_h, ay - r_h),
            (ax - r_h, ay + r_h),
            (ax + r_h, ay + r_h),
        ] {
            for cz in [z0, z1] {
                if let Some((sx, sy, _)) = project(&basis, cx, cy, cz) {
                    minx = minx.min(sx);
                    miny = miny.min(sy);
                    maxx = maxx.max(sx);
                    maxy = maxy.max(sy);
                }
            }
        }
        if minx.is_infinite() {
            continue; // entirely behind the near plane
        }
        if px + PICK_PAD < minx
            || px - PICK_PAD > maxx
            || py + PICK_PAD < miny
            || py - PICK_PAD > maxy
        {
            continue; // the click isn't on this body
        }
        let (wx, wy, wz) = (ax, ay, e.pos.z + anchor_height(e));
        let Some((_, _, zc)) = project(&basis, wx, wy, wz) else {
            continue;
        };
        // Line of sight: an entity hidden behind terrain, a wall, or a closed
        // door from the camera can't be clicked. Occluded ones are skipped so
        // a nearer visible one still wins.
        if let Some(w) = w3d {
            if !w.line_of_sight(basis.cam, [wx, wy, wz]) {
                continue;
            }
        }
        match best {
            Some((_, bd)) if bd <= zc => {}
            _ => best = Some((e.guid, zc)),
        }
    }
    best.map(|(g, _d)| g)
}

/// The nearest pickable tactical blip (Chebyshev distance ≤ 1) to the cell.
fn pick_tactical(app: &App, lb: WLayout, r: u16, c: u16) -> Option<u32> {
    let t = tactical_geom(app, lb)?;
    let mut best: Option<(u32, u16)> = None;
    for (guid, _ch, bc, br) in &t.blips {
        let Some(e) = app.world.entities.get(guid) else {
            continue;
        };
        let pickable = e.is_player
            || e.item_type == ITEM_TYPE_CREATURE
            || e.item_type == ITEM_TYPE_GROUND
            || e.item_type == ITEM_TYPE_DOOR;
        if !pickable {
            continue;
        }
        let d = bc.abs_diff(c).max(br.abs_diff(r));
        if d > 1 {
            continue;
        }
        match best {
            Some((_, bd)) if bd <= d => {}
            _ => best = Some((*guid, d)),
        }
    }
    best.map(|(g, _d)| g)
}

/// The kind of a world entity for target-list purposes (2 player, 0
/// creature, 1 object) — matches `targets_of_kind`'s classification.
fn target_kind_for(app: &App, guid: u32) -> u8 {
    let Some(e) = app.world.entities.get(&guid) else {
        return 1;
    };
    if e.is_player {
        2
    } else if e.item_type == 0 || e.item_type == ITEM_TYPE_CREATURE {
        0
    } else {
        1
    }
}

/// Switch the current target to a guid: re-select the target kind, rebuild
/// the target list, and point `target_index` at it.
fn set_target_by_guid(app: &mut App, guid: u32) {
    app.target_kind = target_kind_for(app, guid);
    app.refresh_targets();
    app.set_world_target(guid);
    match app.targets.iter().position(|&g| g == guid) {
        Some(i) => app.world.target_index = i,
        None => app.world.flash = Some("out of range".into()),
    }
}

/// A left-button press: start a drag on an inventory item, act on a widget
/// box, pick/target a world entity (attack in combat mode; double-click
/// uses the target), select a widget or slot, or focus chat.
fn left_down(app: &mut App, area: Rect, r: u16, c: u16) {
    let Some(lb) = world_layout(area.height, area.width, widget_active(app)) else {
        return;
    };
    // Inventory item: start a drag + target it.
    if let Some((guid, slot)) = inv_item_at(app, lb, r, c, area.width) {
        app.drag = Some(guid);
        app.set_item_target(guid, TARGET_ITEM_PACK);
        match slot {
            InvSlot::Pack(i) => app.world.inventory.sel = i,
            InvSlot::Doll(i) => app.world.inventory.doll_sel = i,
        }
        return;
    }
    // The single main pack's icon: a click is a no-op.
    if app.multi_sel == INV_WIDGET && inv_pack_icon_at(lb, r, c, area.width) {
        return;
    }
    // A side pack's icon: open it (the loot view shows its contents).
    if let Some(idx) = side_pack_icon_at(app, lb, r, c, area.width) {
        app.open_side_pack(idx);
        return;
    }
    // Vendor / trade / loot widget box.
    if widget_active(app)
        && r >= lb.widget_y0 + 1
        && r <= lb.widget_y1 - 1
        && c >= 1
        && c <= lb.left_w - 2
    {
        if app.world.vendor.guid != 0 {
            vendor_press(app, lb, r, c);
        } else if app.world.trade.partner != 0 {
            trade_press(app, lb, r, c);
        } else {
            loot_press(app, lb, r, c);
        }
        return;
    }
    // World pane: pick (target, attack in combat mode, double-click use).
    if r >= lb.world_y0 + 1 && r <= lb.world_y1 - 2 && c >= 1 && c <= lb.left_w - 2 {
        if let Some(guid) = pick_world_at(app, lb, r, c) {
            set_target_by_guid(app, guid);
            // Commit the target — set_target_by_guid only points
            // the index; appraise_current sends the server query so the status
            // bar and reticle lock onto the clicked entity.
            app.appraise_current();
            if (app.world.combat.mode == combat_mode::MELEE
                || app.world.combat.mode == combat_mode::MISSILE)
                && target_kind_for(app, guid) == 0 {
                app.attack();
            }
            let now = Instant::now();
            let dbl = is_double_click(app, now, (r, c));
            app.last_click_at = Some(now);
            app.last_click_pos = Some((r, c));
            if dbl {
                app.use_target();
            }
        }
        return;
    }
    // Widget selector row (the 14 glyphs).
    if r == lb.multi_y1 && c >= lb.right_x {
        let i = (c as usize).saturating_sub(selector_start(lb.right_x, area.width, 14) as usize);
        if i < 14 && i != app.multi_sel {
            app.open_widget(i);
        }
        return;
    }
    // Spell / hotkey slot bars.
    if c >= lb.right_x && c < lb.right_x + lb.right_w {
        let key_y0 = lb.body_h - 2;
        if r == key_y0 {
            if let Some(i) = slot_cols(lb.right_x, lb.right_w, "Spells:")
                .iter()
                .position(|&gc| gc == c)
            {
                app.world.skills.slot = i;
            }
        } else if r == key_y0 + 1 {
            if let Some(i) = slot_cols(lb.right_x, lb.right_w, "Keys:")
                .iter()
                .position(|&gc| gc == c)
            {
                app.hotkey_sel = i;
            }
        }
        return;
    }
    // Chat: a click focuses it (no buffer clear).
    if r >= lb.chat_y0 && r <= lb.body_h && c < lb.left_w {
        app.chat_focused = true;
    }
}

/// A left-button release: route the in-flight drag to its drop target (the
/// pack icon, a paper-doll slot, a widget box, or the world / ground).
fn left_up(app: &mut App, area: Rect, r: u16, c: u16) {
    let item = match app.drag {
        Some(g) => g,
        None => return,
    };
    let Some(lb) = world_layout(area.height, area.width, widget_active(app)) else {
        return;
    };
    // Extract everything from the item record before any &mut call.
    let name = item_name(app, item);
    let it = find_item(app, item).cloned();
    let in_world_pane = r >= lb.world_y0 + 1
        && r <= lb.world_y1 - 2
        && c >= 1
        && c <= lb.left_w - 2;
    let hit = if in_world_pane { pick_world_at(app, lb, r, c) } else { None };
    app.drag = None;

    // 1. Inventory widget: the pack icon (store) or a paper-doll slot (equip).
    if app.multi_sel == INV_WIDGET {
        if inv_pack_icon_at(lb, r, c, area.width) {
            if in_main_pack(app, item) {
                app.world.flash = Some("already in that pack".into());
            } else if let Some(selfg) = app.self_guid {
                app.put_item_in_container(item, selfg);
                app.world.flash = Some(format!("moving {name} to backpack"));
            }
            return;
        }
        if doll_drop_at(lb, r, c, area.width) {
            let worn = it
                .as_ref()
                .and_then(|i| if i.wielder_id == 0 { None } else { Some(i.wielder_id) })
                .filter(|&w| Some(w) == app.self_guid);
            if worn.is_some() {
                app.world.flash = Some("already worn".into());
            } else {
                match it.as_ref().and_then(|i| equip_loc_for(app, i)) {
                    Some(loc) => {
                        app.equip_item(item, loc as i32);
                        app.world.flash = Some(format!("equipping {name} ({})", loc_name(loc)));
                    }
                    None => app.world.flash = Some("no free wear location".into()),
                }
            }
            return;
        }
    }
    // 2. Widget box: vendor (list for sale), trade (offer), loot (put in).
    if widget_active(app) && r >= lb.widget_y0 && r <= lb.widget_y1 && c < lb.left_w {
        if app.world.vendor.guid != 0 {
            let (sellable, amt) = app
                .world
                .inventory
                .pack
                .iter()
                .find(|i| i.guid == item && i.value > 0)
                .map(|i| (true, i.stack_size.max(1)))
                .unwrap_or((false, 0));
            if !sellable {
                app.world.flash = Some("the vendor won't buy that".into());
            } else if app.world.vendor.selling.iter().any(|(g, _)| *g == item) {
                app.world.flash = Some("already listed".into());
            } else {
                app.world.vendor.selling.push((item, amt));
                app.world.flash = Some(format!("sell: {name}"));
            }
        } else if app.world.trade.partner != 0 {
            app.trade_add(item, 0);
            app.world.flash = Some(format!("added {name} to trade"));
        } else {
            let (cname, cguid) = (app.world.container.name.clone(), app.world.container.guid);
            app.put_item_in_container(item, cguid);
            app.world.flash = Some(format!("put {name} in {cname}"));
        }
        return;
    }
    // 3. World pane: drop on an entity (player trade, key use, container,
    //    or use-on) or the ground (drop).
    if in_world_pane {
        match hit {
            Some(guid) => {
                let kind = target_kind_for(app, guid);
                if kind == 2 {
                    // A player: add to the open trade, or start one.
                    if app.world.trade.partner != 0 {
                        if app.world.trade.partner == guid {
                            app.trade_add(item, 0);
                            app.world.flash = Some(format!("added {name} to trade"));
                        } else {
                            app.world.flash =
                                Some(format!("already trading with {}", app.world.trade.partner_name));
                        }
                    } else {
                        set_target_by_guid(app, guid);
                        app.use_target();
                    }
                } else {
                    let is_key = it.as_ref().map(|i| i.item_type == ITEM_TYPE_KEY).unwrap_or(false);
                    let etype = app.world.entities.get(&guid).map(|e| e.item_type).unwrap_or(0);
                    let tname = entity_name(app, guid);
                    if is_key {
                        app.use_item_on(item, guid);
                        app.world.flash = Some(format!("using {name} on {tname}"));
                    } else if kind == 1 && (etype == ITEM_TYPE_GROUND || etype == ITEM_TYPE_CONTAINER)
                    {
                        app.put_item_in_container(item, guid);
                        app.world.flash = Some(format!("put {name} in {tname}"));
                    } else {
                        app.use_item_on(item, guid);
                        app.world.flash = Some(format!("using {name} on {tname}"));
                    }
                }
            }
            None => {
                // Ground: drop the item at the player's feet.
                app.drop_item(item);
                app.world.flash = Some(format!("dropping {name}"));
            }
        }
        return;
    }
    // 4. Elsewhere: the drag is cancelled (no flash).
}

/// A right-button press: examine an inventory / trade / vendor / loot item
/// (switching to the Examine widget) or a world entity.
fn right_down(app: &mut App, area: Rect, r: u16, c: u16) {
    app.drag = None;
    let Some(lb) = world_layout(area.height, area.width, widget_active(app)) else {
        return;
    };
    if let Some((guid, _slot)) = inv_item_at(app, lb, r, c, area.width) {
        app.set_item_target(guid, TARGET_ITEM_PACK);
        app.identify_item(guid);
        app.multi_sel = EXAMINE_WIDGET;
        app.list_scroll = 0;
        return;
    }
    if let Some((guid, _i)) = trade_item_at(app, lb, r, c) {
        app.set_item_target(guid, TARGET_ITEM_TRADE);
        app.identify_item(guid);
        app.multi_sel = EXAMINE_WIDGET;
        app.list_scroll = 0;
        return;
    }
    if let Some(guid) = vendor_item_at(app, lb, r, c) {
        app.set_item_target(guid, TARGET_ITEM_SHOP);
        app.identify_item(guid);
        app.multi_sel = EXAMINE_WIDGET;
        app.list_scroll = 0;
        return;
    }
    if let Some((guid, _i)) = loot_item_at(app, lb, r, c) {
        app.set_item_target(guid, TARGET_ITEM_CONTAINER);
        app.identify_item(guid);
        app.multi_sel = EXAMINE_WIDGET;
        app.list_scroll = 0;
        return;
    }
    if r >= lb.world_y0 + 1 && r <= lb.world_y1 - 2 && c >= 1 && c <= lb.left_w - 2 {
        if let Some(guid) = pick_world_at(app, lb, r, c) {
            set_target_by_guid(app, guid);
            app.examine();
            app.multi_sel = EXAMINE_WIDGET;
            app.list_scroll = 0;
        }
    }
}

/// A scroll-wheel press: page the vendor list row, or scroll the inventory
/// pack grid (the wheel only ever scrolls those two).
fn wheel(app: &mut App, area: Rect, r: u16, c: u16, up: bool) {
    let d: isize = if up { -1 } else { 1 };
    let Some(lb) = world_layout(area.height, area.width, widget_active(app)) else {
        return;
    };
    // 3D view: scrolling over the world pane zooms the camera in (up) / out
    // (down) rather than scrolling a list.
    if app.world_view && c < lb.left_w && r >= lb.world_y0 && r <= lb.world_y1 {
        if let Some(w) = app.world3d.as_mut() {
            if up {
                w.zoom_in();
            } else {
                w.zoom_out();
            }
        }
        return;
    }
    if app.world.vendor.guid != 0 && c <= lb.left_w - 1 {
        let y = r - lb.widget_y0;
        if y == 2 {
            let page = vendor_page(app);
            app.world.vendor.page =
                ((page.start as isize) + d).clamp(0, page.max_start as isize) as usize;
            return;
        }
        if y == 6 && app.world.vendor.tab == 0 {
            // the category filter row: one category per turn
            let max = vendor_categories(&app.world.vendor).len().saturating_sub(1) as isize;
            app.world.vendor.cat_scroll =
                ((app.world.vendor.cat_scroll as isize) + d).clamp(0, max) as usize;
            return;
        }
    }
    if app.multi_sel != INV_WIDGET {
        return;
    }
    let inner_x0 = lb.right_x + 1;
    let inner_y0 = lb.radar_h + 1;
    if r < inner_y0 || c < inner_x0 {
        return;
    }
    let rel_r = (r - inner_y0) as usize;
    let rel_c = (c - inner_x0) as usize;
    let inner_w = (area.width - lb.right_x - 2) as usize;
    let rows_total = (lb.multi_y1 - lb.radar_h - 2) as usize;
    if rel_r >= rows_total || rel_c >= inner_w {
        return;
    }
    let (gw, gr, _ix) = inv_grid_geom(inner_w, rows_total);
    if rel_r >= INV_GRID_R0 as usize && rel_r < INV_GRID_R0 as usize + gr && rel_c < gw {
        let cells = gr * gw;
        let n = app.world.inventory.pack.len();
        app.list_scroll =
            ((app.list_scroll as isize) + d).max(0).min(n.saturating_sub(cells) as isize) as usize;
    }
}

/// The mouse pointer marker (drawn every frame): a "+" that shows the
/// dragged item's glyph during a drag.
fn draw_mouse_marker(f: &mut Frame, app: &App) {
    let Some((r, c)) = app.mouse_xy else {
        return;
    };
    let area = f.area();
    let r = r.min(area.height - 1);
    let c = c.min(area.width - 1);
    let glyph = app.drag.map(|g| item_glyph_of_guid(app, g)).unwrap_or('+');
    put(f, r, c, &glyph.to_string(), reverse(app));
}

// ================================================================== menu views

/// A left press in a menu subview: a single click moves the selection, a
/// double click activates (exactly the keyboard Enter branches).
fn view_press(app: &mut App, view: MenuView, r: u16, c: u16, h: u16, w: u16) {
    if r < 2 || r > h.saturating_sub(3) || c < 1 || c >= w.saturating_sub(1) {
        app.last_click_at = None;
        return;
    }
    let i = (r - 2) as usize;
    let n = match view {
        MenuView::Menu => 7,
        MenuView::Themes => 4,
        MenuView::Widgets => 14,
        MenuView::WorldView => 2,
        MenuView::Help => 0,
    };
    if i >= n {
        app.last_click_at = None;
        return;
    }
    let now = Instant::now();
    let dbl = is_double_click(app, now, (r, c));
    app.last_click_at = Some(now);
    app.last_click_pos = Some((r, c));
    if !dbl {
        match view {
            MenuView::Menu => app.menu_sel = i.min(6),
            MenuView::Themes => app.theme_sel = i.min(3),
            MenuView::Widgets => app.widget_sel = i.min(13),
            MenuView::WorldView => app.worldview_sel = i.min(1),
            MenuView::Help => {}
        }
        return;
    }
    match view {
        MenuView::Menu => match app.menu_sel {
            0 => {
                if let Some(i) = Theme::ALL.iter().position(|&t| t == app.theme) {
                    app.theme_sel = i;
                }
                app.menu_view = Some(MenuView::Themes);
            }
            1 => {
                app.widget_sel = app.multi_sel.min(13);
                app.menu_view = Some(MenuView::Widgets);
            }
            2 => {
                app.worldview_sel = usize::from(app.world_view);
                app.menu_view = Some(MenuView::WorldView);
            }
            3 => {
                app.glow_actors = !app.glow_actors;
                app.world.flash = Some(format!(
                    "actor glow: {}",
                    if app.glow_actors { "on" } else { "off" }
                ));
            }
            4 => {
                app.reticle_animation = !app.reticle_animation;
                app.world.flash = Some(format!(
                    "reticle animation: {}",
                    if app.reticle_animation { "on" } else { "off" }
                ));
            }
            5 => {
                app.menu_view = Some(MenuView::Help);
                app.help_scroll = 0;
            }
            6 => app.logout(),
            _ => app.quit = true,
        },
        MenuView::Themes => {
            app.theme = Theme::ALL[app.theme_sel.min(3)];
            app.world.flash = Some(format!("theme: {}", app.theme.name()));
        }
        MenuView::Widgets => {
            app.open_widget(app.widget_sel);
        }
        MenuView::WorldView => {
            app.world_view = app.worldview_sel == 1;
            app.world.flash = Some(if app.world_view {
                "world view: 3d"
            } else {
                "world view: tactical"
            }
            .into());
        }
        MenuView::Help => {}
    }
    app.last_click_at = None;
}

/// The Esc menu (and subviews) as a full-screen list: the selected row
/// reversed, a footer hint above the bottom border.
fn draw_menu_view(f: &mut Frame, app: &App, area: Rect, view: MenuView) {
    let (title, footer) = match view {
        MenuView::Menu => ("Menu", "j/k move   Enter open   Esc back"),
        MenuView::Themes => ("Themes", "j/k move   Enter select   Esc back"),
        MenuView::Widgets => ("Widgets (right-column multi box)", "j/k move   Enter select   Esc back"),
        MenuView::WorldView => ("World view (world pane render mode)", "j/k move   Enter select   Esc back"),
        MenuView::Help => ("Help", "j/k scroll   Esc back"),
    };
    draw_box(f, app, 0, 0, area.width, area.height - 1, title, true);
    let mut rows: Vec<(usize, String, bool)> = Vec::new();
    match view {
        MenuView::Menu => {
            for (i, item) in MENU_ITEMS.iter().enumerate() {
                let line = if i == 3 {
                    format!("{}: {}", item, if app.glow_actors { "on" } else { "off" })
                } else if i == 4 {
                    format!("{}: {}", item, if app.reticle_animation { "on" } else { "off" })
                } else {
                    item.to_string()
                };
                rows.push((i, line, i == app.menu_sel));
            }
        }
        MenuView::Themes => {
            for i in 0..Theme::ALL.len() {
                let cur = Theme::ALL[i] == app.theme;
                let line = format!(
                    "{}: {}{}",
                    Theme::ALL[i].name(),
                    THEME_LABELS[i],
                    if cur { "  *current" } else { "" }
                );
                rows.push((i, line, i == app.theme_sel));
            }
        }
        MenuView::Widgets => {
            for i in 0..WIDGET_GLYPHS.len() {
                let line = format!("{}  {}", WIDGET_GLYPHS[i], WIDGET_LABELS[i]);
                rows.push((i, line, i == app.widget_sel));
            }
        }
        MenuView::WorldView => {
            for i in 0..WORLD_VIEWS.len() {
                let key = if i == 1 { "3d" } else { "tactical" };
                let line = format!(
                    "{}: {}{}",
                    key,
                    WORLD_VIEWS[i],
                    if (i == 1) == app.world_view { "  *current" } else { "" }
                );
                rows.push((i, line, i == app.worldview_sel));
            }
        }
        MenuView::Help => {
            for (i, line) in HELP_LINES.iter().enumerate() {
                if i < app.help_scroll {
                    continue;
                }
                rows.push((i, line.to_string(), false));
            }
        }
    }
    let y_max = area.height.saturating_sub(3);
    for (_i, line, sel) in rows {
        let y = 2usize + _i;
        if (y as u16) >= y_max {
            break;
        }
        if sel {
            let padded = format!(
                "{:width$}",
                format!(" {}", line),
                width = (area.width as usize).saturating_sub(2)
            );
            put(f, y as u16, 1, &padded, reverse(app));
        } else {
            put(f, y as u16, 2, &line, normal(app));
        }
    }
    put(f, y_max + 1, 2, footer, dark(app));
}

// ================================================================== non-world screens

/// The status screen's server/account line.
fn status_title(app: &App) -> String {
    format!("{} — {}@{}", app.server_name, app.account, app.host)
}

/// The pre-world status screen (connecting / entering).
fn draw_status(f: &mut Frame, app: &App) {
    let area = f.area();
    let text = vec![
        Line::from(Span::styled(
            "Asheron's Call — TUI client",
            ts(app, Style::default().add_modifier(Modifier::BOLD)),
        )),
        Line::from(""),
        Line::from(Span::raw(&app.status)),
        Line::from(Span::styled(
            status_title(app),
            ts(app, Style::default().fg(Color::DarkGray)),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Esc  Quit",
            ts(app, Style::default().fg(Color::DarkGray)),
        )),
    ];
    let p = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(app.theme.border_set())
                .title("Status"),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

/// The disconnected screen (retail `gmDisconnectedUI`, mode 0x10000002):
/// the reason, and the user-initiated actions.
fn draw_disconnected(f: &mut Frame, app: &App) {
    let area = f.area();
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            &app.status,
            ts(app, Style::default().fg(Color::Yellow)),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Enter  Reconnect",
            ts(app, Style::default().fg(Color::Green)),
        )),
        Line::from(Span::styled(
            "Esc    Quit",
            ts(app, Style::default().fg(Color::DarkGray)),
        )),
    ];
    // Center the box on the screen (5 lines + 2 border rows).
    let width = 40u16.min(area.width);
    let height = (lines.len() + 2) as u16;
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(app.theme.border_set())
                .title(Span::styled(
                    "Disconnected",
                    ts(app, Style::default().fg(Color::Red)),
                ))
                .style(ts(app, Style::default().bg(Color::Black))),
        )
        .wrap(Wrap { trim: false });
    let rect = Rect { x, y, width, height };
    f.render_widget(p, rect);
}

/// The character list (the retail char-select screen, list + hint).
fn draw_charselect(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(3)])
        .split(area);
    let items: Vec<ListItem> = app
        .char_list
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mark = if i == app.char_sel { ">" } else { " " };
            let mut s = format!("{mark} {}", c.name);
            if c.time_since_delete > 0 {
                s.push_str(&format!("  (deleted {}s ago)", c.time_since_delete));
            }
            let style = if i == app.char_sel {
                ts(app, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            } else {
                normal(app)
            };
            ListItem::new(s).style(style)
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_set(app.theme.border_set())
            .title(format!("Characters ({})", app.char_list.len())),
    );
    f.render_widget(list, chunks[0]);
    let hint = Paragraph::new(vec![
        Line::from(Span::raw(&app.status)),
        Line::from(Span::styled(
            "↑/↓ select · Enter enter world · C create · D delete · Esc quit",
            ts(app, Style::default().fg(Color::DarkGray)),
        )),
    ]);
    f.render_widget(hint, chunks[1]);
}

/// The character-create form (heritage / template / gender / start / attrs).
fn draw_charcreate(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(area);
    let form = &app.create_form;
    let h = form.heritage();
    let t = form.template();
    let start = h.start_areas.get(form.start_area_idx).copied().unwrap_or(0);
    let start_name = acproto::chargen::starter_area_name(start);
    let sum = form.attr_sum();
    let over = sum > h.attribute_credits;
    let sum_style = if over { Color::Red } else { Color::Green };
    let attr_names = ["Str", "End", "Coord", "Quick", "Focus", "Self"];
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Name: ", ts(app, Style::default().fg(Color::DarkGray))),
        Span::styled(format!("[{}]", form.name), ts(app, Style::default().fg(Color::Cyan))),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Heritage: ", ts(app, Style::default().fg(Color::DarkGray))),
        Span::raw(h.name.to_string()),
        Span::styled(
            format!("  ({} attr credits)", h.attribute_credits),
            ts(app, Style::default().fg(Color::DarkGray)),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Template: ", ts(app, Style::default().fg(Color::DarkGray))),
        Span::raw(t.name.to_string()),
        Span::styled(
            "  (Tab to change)",
            ts(app, Style::default().fg(Color::DarkGray)),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Gender: ", ts(app, Style::default().fg(Color::DarkGray))),
        Span::raw(if form.gender == 1 { "Male" } else { "Female" }.to_string()),
        Span::styled(
            "  (PgUp to toggle)",
            ts(app, Style::default().fg(Color::DarkGray)),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Start: ", ts(app, Style::default().fg(Color::DarkGray))),
        Span::raw(start_name.unwrap_or("Unknown").to_string()),
        Span::styled(
            "  (←/→ to change)",
            ts(app, Style::default().fg(Color::DarkGray)),
        ),
    ]));
    lines.push(Line::from(""));
    for (i, a) in form.attrs.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:>5} ", attr_names[i]),
                ts(app, Style::default().fg(Color::DarkGray)),
            ),
            Span::styled(
                format!("{a:>3}"),
                ts(app, Style::default().fg(if *a >= 100 { Color::Yellow } else { Color::White })),
            ),
            Span::styled(
                format!("  (F{} +5)", i + 1),
                ts(app, Style::default().fg(Color::DarkGray)),
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Attribute sum: ", ts(app, Style::default().fg(Color::DarkGray))),
        Span::styled(
            format!("{sum} / {}", h.attribute_credits),
            ts(app, Style::default().fg(sum_style).add_modifier(Modifier::BOLD)),
        ),
    ]));
    if let Some(err) = &form.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            ts(app, Style::default().fg(Color::Red)),
        )));
    }
    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(app.theme.border_set())
                .title("Create Character"),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(p, chunks[0]);
    let hint = Paragraph::new(vec![
        Line::from(Span::raw(&app.status)),
        Line::from(Span::styled(
            "type name · ↑/↓ heritage · Tab template · PgUp gender · ←/→ start · F1–F6 +5 attrs · Enter create · Esc back",
            ts(app, Style::default().fg(Color::DarkGray)),
        )),
    ]);
    f.render_widget(hint, chunks[1]);
}

/// A small centered overlay box (the death screen); the text caret is left
/// inside it (the reference places the cursor there too).
fn draw_center_box(f: &mut Frame, app: &App, title: &str, lines: &[&str], title_color: Color) {
    let area = f.area();
    let width = lines
        .iter()
        .map(|l| l.len())
        .max()
        .unwrap_or(title.len())
        .min(40) as u16
        + 4;
    let height = (lines.len() + 1) as u16 + 2;
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x: area.x + x,
        y: area.y + y,
        width,
        height,
    };
    let content: Vec<Line> = lines
        .iter()
        .map(|l| Line::from(Span::raw(l.to_string())))
        .collect();
    let p = Paragraph::new(content).block(
        Block::default()
            .borders(Borders::ALL)
            .border_set(app.theme.border_set())
            .title(Span::styled(
                title.to_string(),
                ts(app, Style::default().fg(title_color)),
            ))
            .style(ts(app, Style::default().bg(Color::Black))),
    );
    f.render_widget(p, rect);
    f.set_cursor_position((rect.x + 1, rect.y + 1));
}

#[cfg(test)]
mod tests {
    use super::{item_glyph, ITEM_TYPES, WIDGET_GLYPHS};
    use ratatui::buffer::Buffer;
    use ratatui::style::Style;

    /// Every icon: the 14 widget-selector glyphs + the 31 item-type glyphs.
    fn all_icons() -> Vec<char> {
        let mut v = WIDGET_GLYPHS.to_vec();
        v.extend(ITEM_TYPES.iter().map(|&(_, g, _)| g));
        v
    }

    /// Every icon must occupy exactly one terminal cell when placed through
    /// ratatui (the real rendering path); otherwise the pack grid, paper doll,
    /// vendor/loot lists and the one-row widget selector would misalign. A
    /// 0- or 2-width glyph shifts the grid.
    #[test]
    fn icons_are_single_cell() {
        let icons = all_icons();
        assert_eq!(icons.len(), WIDGET_GLYPHS.len() + ITEM_TYPES.len());
        for g in icons {
            let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 8, 1));
            let (x, _) = buf.set_stringn(0, 0, &g.to_string(), 8, Style::default());
            assert_eq!(x, 1, "U+{:05X} must be 1 cell wide", g as u32);
        }
    }

    /// Every icon must be a Material Design Icon in the Nerd Fonts v3 range
    /// (0xF0001-0xF1AF0, the Supplementary PUA) — distinct from Powerline and
    /// Font Awesome (BMP PUA), and guaranteed single-cell.
    #[test]
    fn icons_are_in_mdi_range() {
        for g in all_icons() {
            let cp = g as u32;
            assert!(
                (0xF0001..=0xF1AF0).contains(&cp),
                "U+{cp:05X} is outside the Nerd Font MDI range",
            );
        }
    }

    /// Each item-type flag must resolve (first matching flag) to exactly its
    /// own table glyph — catching ordering/masking regressions — and an
    /// unknown type (0) falls back to '?'.
    #[test]
    fn item_flags_resolve_to_their_icons() {
        for (flag, g, _name) in ITEM_TYPES {
            assert_eq!(
                item_glyph(flag),
                g,
                "flag 0x{flag:x} should map to U+{:05X}",
                g as u32,
            );
        }
        assert_eq!(item_glyph(0), '?');
    }
}
