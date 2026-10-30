//! dsh-tray entry point: CLI dispatch and the tao tray event loop.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::run_return::EventLoopExtRunReturn;
use tray_icon::menu::MenuEvent;
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

use dsh_tray::app::{self, Command, ControllerHost, Message, Snapshot};
use dsh_tray::cli::{self, Action};
use dsh_tray::config::{AppPaths, Config};
use dsh_tray::logging;
use dsh_tray::single_instance;
use dsh_tray::tray::{self, TrayUi, UserEvent};
use dsh_tray::{APP_NAME, VERSION};

fn main() {
    let action = match cli::parse(std::env::args().skip(1)) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let result = match action {
        Action::Help => {
            print!("{}", cli::usage());
            Ok(())
        }
        Action::Version => {
            println!("{APP_NAME} {VERSION}");
            Ok(())
        }
        Action::Check { json } => cli::run_check(json),
        Action::Smoke => cli::run_smoke(),
        Action::Tray { hidden } => run_tray(hidden),
    };
    if let Err(error) = result {
        eprintln!("{APP_NAME}: {error:#}");
        std::process::exit(1);
    }
}

struct UiState {
    tray: Option<TrayUi>,
    commands: Option<std::sync::mpsc::Sender<Message>>,
    initial: Snapshot,
    paths: AppPaths,
    config: Config,
    hidden: bool,
    quit_deadline: Option<Instant>,
    shutdown_flag: Arc<AtomicBool>,
    snapshot_proxy: EventLoopProxy<UserEvent>,
    shutdown_proxy: EventLoopProxy<UserEvent>,
}

impl UiState {
    fn init(&mut self) {
        match TrayUi::new(&self.initial, &self.paths) {
            Ok(tray) => {
                logging::info("tray icon created");
                self.tray = Some(tray);
            }
            Err(error) => {
                logging::error(format!("could not create the tray icon: {error:#}"));
                eprintln!("{APP_NAME}: could not create the tray icon: {error:#}");
                std::process::exit(1);
            }
        }

        let snapshot_proxy = self.snapshot_proxy.clone();
        let shutdown_proxy = self.shutdown_proxy.clone();
        let shutdown_flag = self.shutdown_flag.clone();
        let on_snapshot = Box::new(move |snapshot: Snapshot| {
            let _ = snapshot_proxy.send_event(UserEvent::Snapshot(snapshot));
        });
        let on_shutdown = Box::new(move || {
            shutdown_flag.store(true, Ordering::SeqCst);
            let _ = shutdown_proxy.send_event(UserEvent::Shutdown);
        });
        let host = ControllerHost {
            paths: self.paths.clone(),
            config: self.config.clone(),
            hidden: self.hidden,
        };
        match app::start(host, on_snapshot, on_shutdown) {
            Ok(commands) => {
                let quit_sender = commands.clone();
                self.commands = Some(commands);
                if let Err(error) = ctrlc::set_handler(move || {
                    let _ = quit_sender.send(Message::Command(Command::Quit));
                }) {
                    logging::warn(format!("could not install Ctrl-C handler: {error}"));
                }
            }
            Err(error) => {
                logging::error(format!("could not start the controller: {error:#}"));
                eprintln!("{APP_NAME}: could not start the controller: {error:#}");
                std::process::exit(1);
            }
        }
    }

    fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            if commands.send(Message::Command(command)).is_err() {
                logging::warn(format!("controller channel closed; dropped {command:?}"));
            }
        }
    }

    fn on_menu(&mut self, event: MenuEvent) {
        let Some(command) = tray::command_for(&event.id) else {
            return;
        };
        if command == Command::Quit {
            self.quit_deadline = Some(Instant::now() + Duration::from_secs(12));
        }
        self.send(command);
    }

    fn on_tray(&mut self, event: TrayIconEvent) {
        match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => self.send(Command::OpenWeb),
            _ => {}
        }
    }

    fn tick(&mut self, control_flow: &mut ControlFlow) {
        if let Some(deadline) = self.quit_deadline {
            if Instant::now() >= deadline {
                logging::error("graceful shutdown timed out; exiting anyway");
                std::process::exit(1);
            }
            *control_flow = ControlFlow::WaitUntil(deadline);
        }
    }
}

fn run_tray(hidden: bool) -> Result<()> {
    let paths = AppPaths::resolve();
    paths.ensure()?;
    if let Err(error) = logging::init(&paths.log_file()) {
        eprintln!("warning: could not open the log file: {error}");
    }
    logging::info(format!(
        "starting {APP_NAME} {VERSION} (os={}, hidden={hidden})",
        std::env::consts::OS
    ));

    let _instance_guard = match single_instance::acquire(&paths.lock_file())? {
        Some(guard) => guard,
        None => {
            handoff_to_running(&paths);
            return Ok(());
        }
    };
    // No other instance is alive, so any runtime hand-off state is stale.
    single_instance::remove_runtime(&paths.runtime_file());

    let (mut config, warning) = Config::load(&paths.config_file());
    if let Some(warning) = warning {
        logging::warn(format!("config: {warning}"));
    }
    if let Some(warning) = config.normalize() {
        logging::warn(format!("config: {warning}"));
    }
    let initial = Snapshot::initial(&config, &paths);

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }

    let menu_proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = menu_proxy.send_event(UserEvent::Menu(event));
    }));
    let tray_proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let _ = tray_proxy.send_event(UserEvent::Tray(event));
    }));

    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let mut ui = UiState {
        tray: None,
        commands: None,
        initial,
        paths: paths.clone(),
        config,
        hidden,
        quit_deadline: None,
        shutdown_flag: shutdown_flag.clone(),
        snapshot_proxy: event_loop.create_proxy(),
        shutdown_proxy: event_loop.create_proxy(),
    };

    let exit_code = event_loop.run_return(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        ui.tick(control_flow);
        match event {
            Event::NewEvents(StartCause::Init) => ui.init(),
            Event::UserEvent(UserEvent::Snapshot(snapshot)) => {
                if let Some(tray) = ui.tray.as_mut() {
                    tray.update(&snapshot);
                }
            }
            Event::UserEvent(UserEvent::Menu(event)) => ui.on_menu(event),
            Event::UserEvent(UserEvent::Tray(event)) => ui.on_tray(event),
            Event::UserEvent(UserEvent::Shutdown) => {
                ui.shutdown_flag.store(true, Ordering::SeqCst);
                ui.tray.take();
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });

    // The controller stops the child before emitting Shutdown; wait briefly for
    // it so the lock is released only after cleanup (normal path is immediate).
    let deadline = Instant::now() + Duration::from_secs(6);
    while !shutdown_flag.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    logging::info(format!("event loop exited with code {exit_code}"));
    Ok(())
}

fn handoff_to_running(paths: &AppPaths) {
    match single_instance::read_runtime(&paths.runtime_file()) {
        Some(state) => match state.url {
            Some(url) => match open::that(&url) {
                Ok(()) => println!(
                    "{APP_NAME} is already running; opened {}",
                    dsh_tray::process::display_url(&url)
                ),
                Err(error) => {
                    println!("{APP_NAME} is already running (could not open the browser: {error})")
                }
            },
            None => println!("{APP_NAME} is already running (dsh is not serving yet)"),
        },
        None => println!("{APP_NAME} is already running"),
    }
}
