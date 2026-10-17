//! Plain console mode: no TUI. System + world chat lines are printed to
//! stdout, one line per event — machine-greppable and docker-log friendly
//! (parity with acclient-ts plain mode). The process idles until Ctrl+C,
//! then logs the character off cleanly.

use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use crate::app::{App, Phase};
use crate::world::{ChatLine, ChatStyle};

/// Prints new chat/system lines and lifecycle transitions to stdout
/// (one line per event, `[HH:MM:SS]` UTC prefix).
pub struct Plain {
    last_total: u64,
    last_phase: Phase,
}

impl Plain {
    pub fn new() -> Self {
        Self { last_total: 0, last_phase: Phase::Idle }
    }

    /// Clock as `HH:MM:SS` (UTC) — unambiguous and docker-log friendly.
    fn ts() -> String {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() % 86_400)
            .unwrap_or(0);
        format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    }

    fn w(&self, line: &str) {
        let _ = writeln!(std::io::stdout(), "[{}] {}", Self::ts(), line);
        let _ = std::io::stdout().flush();
    }

    /// Format one chat line for plain output.
    fn format(line: &ChatLine) -> String {
        match line.style {
            ChatStyle::System => format!("<system> {}", line.text),
            ChatStyle::Error => format!("<error> {}", line.text),
            ChatStyle::Combat => format!("[combat] {}", line.text),
            ChatStyle::Emote => format!("[{}] {}", line.sender, line.text),
            ChatStyle::Tell => format!("[tell:{}] {}", line.sender, line.text),
            ChatStyle::Channel => format!("[{}] {}", line.sender, line.text),
            ChatStyle::Own => format!("[you] {}", line.text),
            ChatStyle::Speech => {
                if line.sender.is_empty() {
                    line.text.clone()
                } else {
                    format!("{}: {}", line.sender, line.text)
                }
            }
        }
    }

    /// Print new chat lines and any phase transition since the last sync.
    pub fn sync(&mut self, app: &App) {
        let chat = &app.world.chat;
        if chat.total > self.last_total {
            let new = (chat.total - self.last_total) as usize;
            let len = chat.len();
            let start = if new > len { 0 } else { len - new };
            for line in chat.iter().skip(start) {
                self.w(&Self::format(line));
            }
            self.last_total = chat.total;
        }
        if app.phase != self.last_phase {
            match app.phase {
                Phase::CharSelect => {
                    self.w(&format!("connected to {}", app.account));
                    let names: Vec<&str> = app.char_list.iter().map(|c| c.name.as_str()).collect();
                    let joined = if names.is_empty() {
                        "(none)".to_string()
                    } else {
                        names.join(", ")
                    };
                    self.w(&format!("characters: {joined}"));
                }
                Phase::InWorld => self.w(&format!("entered world as {}", app.self_name)),
                Phase::Disconnected => self.w(&format!("disconnected: {}", app.status)),
                _ => {}
            }
            self.last_phase = app.phase;
        }
    }
}

/// Plain mode event loop: no TUI. Drives the session, prints new lines, and
/// idles until Ctrl+C (a clean logoff).
pub fn run(app: &mut App) -> anyhow::Result<()> {
    // Best-effort raw mode so Ctrl+C is readable for a clean logoff (skipped
    // when stdin is not a TTY; the loop keeps running either way).
    let raw_ok = enable_raw_mode().is_ok();
    let mut plain = Plain::new();
    let result: anyhow::Result<()> = loop {
        if event::poll(Duration::from_millis(50)).is_ok_and(|p| p) {
            if let Ok(Event::Key(k)) = event::read() {
                if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
                    app.quit = true;
                } else {
                    app.on_key(&k);
                }
            }
        }
        let events = app.net.as_ref().map(|n| n.poll()).unwrap_or_default();
        for ev in events {
            app.handle_net_event(ev);
        }
        app.step(Instant::now());
        plain.sync(app);
        if app.quit {
            app.teardown();
            break Ok(());
        }
    };
    if raw_ok {
        disable_raw_mode()?;
    }
    result
}
