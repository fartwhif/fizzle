//! `fizzle` — a terminal TUI client for Asheron's Call (ACE server).

mod app;
mod movement;
mod net;
mod plain;
mod profiles;
mod themes;
mod ui;
mod world;

use std::io;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use acgeo::World3D;
use app::App;
use net::DEFAULT_GAME_PORT;
use profiles::Profiles;
use themes::Theme;

#[derive(Parser, Debug)]
#[command(name = "fizzle", version, about = "Terminal TUI client for Asheron's Call (ACE)")]
struct Cli {
    /// Server host
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// UDP game port (the connect port is game port + 1)
    #[arg(long, default_value_t = DEFAULT_GAME_PORT)]
    port: u16,

    /// Name of a known server (host looked up in the profile store)
    #[arg(long)]
    server: Option<String>,

    /// Account name (defaults to the active profile account)
    #[arg(long)]
    account: Option<String>,

    /// Account password (defaults to the saved profile password)
    #[arg(long)]
    password: Option<String>,

    /// Character to auto-enter once the character list arrives
    #[arg(long)]
    character: Option<String>,

    /// UI mode: `tui` (default, ratatui console) or `plain` (stdout, no TUI)
    #[arg(long, default_value = "tui")]
    ui: String,
}

/// Restores the terminal even on a panic mid-session.
struct TermCleanup;

impl Drop for TermCleanup {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            PopKeyboardEnhancementFlags,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut profiles = Profiles::load();

    // Resolve the host: `--server` names a profile entry, else it is a host.
    let (host, server_name) = match &cli.server {
        Some(name) => match profiles.servers.iter().position(|s| &s.name == name) {
            Some(i) => (profiles.servers[i].host.clone(), name.clone()),
            None => (name.clone(), name.clone()),
        },
        None => (cli.host.clone(), cli.host.clone()),
    };

    let account = match cli.account.clone() {
        Some(a) => a,
        None => profiles
            .active_account
            .and_then(|i| profiles.accounts.get(i))
            .map(|a| a.name.clone())
            .unwrap_or_default(),
    };
    let password = match cli.password.clone() {
        Some(p) => p,
        None => profiles
            .accounts
            .iter()
            .find(|a| a.name == account)
            .map(|a| a.password.clone())
            .unwrap_or_default(),
    };
    if account.is_empty() {
        anyhow::bail!("no account: pass --account (and --password)");
    }

    // Persist the profile entry up front.
    let si = profiles.upsert_server(&server_name, &host);
    let ai = profiles.upsert_account(&account, &password);
    profiles.active_server = Some(si);
    profiles.active_account = Some(ai);
    let _ = profiles.save();

    let mut app = App::new(&host, cli.port, &account, &password);
    app.auto_character = cli.character.clone();
    // Persisted UI settings, and the 3D world (None when the retail DATs are
    // unavailable — the client still runs, with the tactical radar only).
    app.theme = Theme::parse(&profiles.theme).unwrap_or(Theme::Custom);
    app.world_view = profiles.world_view == "3d";
    app.glow_actors = profiles.glow_actors;
    app.dat_dir = profiles.dat_dir.clone();
    app.world3d = Some(World3D::new(app.dat_dir.as_deref()));
    app.start();

    match cli.ui.as_str() {
        "plain" => plain::run(&mut app),
        _ => run_event_loop(&mut app),
    }
}

fn run_event_loop(app: &mut App) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        )
    )?;
    let _cleanup = TermCleanup;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result: anyhow::Result<()> = loop {
        terminal.draw(|f| ui::draw(f, app))?;
        if app.quit {
            // Retail exit path: broadcast the Disconnect (0x8000) so the
            // server drops the session now instead of on its 15 s timeout.
            app.teardown();
            break Ok(());
        }
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(k) => {
                    if k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        && k.code == KeyCode::Char('c')
                    {
                        app.quit = true;
                    } else {
                        app.on_key(&k);
                    }
                }
                Event::Mouse(m) => {
                    if let Ok(size) = terminal.size() {
                        let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
                        ui::mouse(app, area, &m);
                    }
                }
                _ => {}
            }
        }
        // Drain network events before mutating app state.
        let events = app.net.as_ref().map(|n| n.poll()).unwrap_or_default();
        for ev in events {
            app.handle_net_event(ev);
        }
        app.step(Instant::now());
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}
