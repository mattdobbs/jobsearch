//! The desktop app: one window showing the board, the menu-bar icon, and the app's
//! menus.
//!
//! The window is a native web view, but it does not load a web page from anywhere:
//! every request it makes is answered inside this process by `server::answer`. No
//! network port is opened.
//!
//! Closing the window does not quit the app. It keeps running from the menu bar so
//! the twice-daily checks continue; the Dock icon or the menu-bar icon brings the
//! window back.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Window, WindowBuilder};
use tray_icon::menu::{AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use wry::{NewWindowResponse, PageLoadEvent, WebViewBuilder};

use crate::state::AppState;
use crate::{log_info, log_warn, login_item, server};

/// The address the window's page lives at. It is a private scheme, not a network one.
const SCHEME: &str = "jobsearch";
const ORIGIN: &str = "jobsearch://localhost";

enum UserEvent {
    Menu(MenuEvent),
    NewCount(i64),
    PageLoaded,
}

/// Hooks for driving the window from an automated test. `JOBSEARCH_UI_TEST` names a
/// script to run in the page once it has loaded; anything the script reports is
/// appended to the file named by `JOBSEARCH_UI_TEST_OUT`.
struct TestHooks {
    script: Option<String>,
    report: Option<PathBuf>,
}

impl TestHooks {
    fn from_env() -> Self {
        let script = std::env::var_os("JOBSEARCH_UI_TEST").and_then(|path| match std::fs::read_to_string(&path) {
            Ok(script) => Some(script),
            Err(e) => {
                log_warn!("could not read the UI test script: {e}");
                None
            }
        });
        TestHooks {
            script,
            report: std::env::var_os("JOBSEARCH_UI_TEST_OUT").map(PathBuf::from),
        }
    }

    pub fn active() -> bool {
        std::env::var_os("JOBSEARCH_UI_TEST").is_some()
    }
}

pub fn test_mode() -> bool {
    TestHooks::active()
}

fn report(path: &Option<PathBuf>, line: &str) {
    let Some(path) = path else { return };
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{line}"));
    if let Err(e) = written {
        log_warn!("could not write the UI test report: {e}");
    }
}

/// Draws a small briefcase for the menu bar. macOS treats it as a template image, so
/// only the alpha channel matters and the system colours it for light and dark bars.
fn menu_bar_icon() -> Icon {
    const SIZE: usize = 36;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    let mut fill = |x0: usize, y0: usize, x1: usize, y1: usize, alpha: u8| {
        for y in y0..y1 {
            for x in x0..x1 {
                rgba[(y * SIZE + x) * 4 + 3] = alpha;
            }
        }
    };
    // Handle.
    fill(12, 5, 24, 8, 255);
    fill(12, 5, 15, 12, 255);
    fill(21, 5, 24, 12, 255);
    // Body, with the corners knocked off.
    fill(3, 11, 33, 31, 255);
    for (x, y) in [(3, 11), (32, 11), (3, 30), (32, 30)] {
        fill(x, y, x + 1, y + 1, 0);
    }
    // The seam and clasp.
    fill(3, 19, 33, 21, 0);
    fill(15, 17, 21, 23, 255);
    Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).expect("icon dimensions are valid")
}

fn count_label(count: i64) -> Option<String> {
    (count > 0).then(|| count.to_string())
}

/// Links to job postings and evidence open in the user's browser, never in the app's
/// own window.
fn open_outside(url: &str, test_mode: bool, test_report: &Option<PathBuf>) {
    let allowed = ["https://", "http://", "mailto:"].iter().any(|prefix| url.starts_with(prefix));
    if !allowed {
        log_warn!("ignored a link the app does not open: {url}");
        return;
    }
    if test_mode {
        report(test_report, &format!("OPENED {url}"));
        return;
    }
    if let Err(e) = open::that(url) {
        log_warn!("could not open {url}: {e}");
    }
}

fn show(window: &Window) {
    window.set_visible(true);
    window.set_minimized(false);
    window.set_focus();
}

/// The standard macOS menus. Without an Edit menu, copy and paste do not work in the
/// window's text fields.
fn app_menu() -> Menu {
    let menu = Menu::new();
    let about = AboutMetadata {
        name: Some(crate::APP_NAME.to_string()),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        comments: Some("Remote jobs at disability- and neurodiversity-inclusive employers.".to_string()),
        ..AboutMetadata::default()
    };
    let built = (|| -> tray_icon::menu::Result<()> {
        menu.append(&Submenu::with_items(
            crate::APP_NAME,
            true,
            &[
                &PredefinedMenuItem::about(None, Some(about)),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::quit(None),
            ],
        )?)?;
        menu.append(&Submenu::with_items(
            "Edit",
            true,
            &[
                &PredefinedMenuItem::undo(None),
                &PredefinedMenuItem::redo(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::cut(None),
                &PredefinedMenuItem::copy(None),
                &PredefinedMenuItem::paste(None),
                &PredefinedMenuItem::select_all(None),
            ],
        )?)?;
        menu.append(&Submenu::with_items(
            "Window",
            true,
            &[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::close_window(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::fullscreen(None),
            ],
        )?)?;
        Ok(())
    })();
    if let Err(e) = built {
        log_warn!("could not build the app menu: {e}");
    }
    menu
}

/// Runs the app's window and menu-bar icon on the main thread. Never returns; Quit
/// exits the process.
pub fn run(state: Arc<AppState>, runtime: tokio::runtime::Handle, show_window: bool) -> ! {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let hooks = TestHooks::from_env();
    let test_mode = state.test_mode;

    let proxy = Mutex::new(event_loop.create_proxy());
    MenuEvent::set_event_handler(Some(move |event| {
        if let Ok(proxy) = proxy.lock() {
            let _ = proxy.send_event(UserEvent::Menu(event));
        }
    }));
    let proxy = Mutex::new(event_loop.create_proxy());
    let _ = state.badge.set(Box::new(move |count| {
        if let Ok(proxy) = proxy.lock() {
            let _ = proxy.send_event(UserEvent::NewCount(count));
        }
    }));

    let menu_bar = app_menu();
    #[cfg(target_os = "macos")]
    menu_bar.init_for_nsapp();

    let window = WindowBuilder::new()
        .with_title(crate::APP_NAME)
        .with_inner_size(LogicalSize::new(1320.0, 880.0))
        .with_min_inner_size(LogicalSize::new(900.0, 560.0))
        .with_visible(show_window)
        .build(&event_loop)
        .expect("the app window could not be created");

    let router = server::router(state.clone());
    let page_proxy = event_loop.create_proxy();
    let (nav_report, window_report, ipc_report) = (hooks.report.clone(), hooks.report.clone(), hooks.report.clone());
    let webview = WebViewBuilder::new()
        .with_asynchronous_custom_protocol(SCHEME.to_string(), move |_id, request, responder| {
            let router = router.clone();
            runtime.spawn(async move {
                responder.respond(server::answer(router, request).await);
            });
        })
        .with_url(format!("{ORIGIN}/"))
        // The window only ever shows the board. Anything else goes to the browser.
        .with_navigation_handler(move |url| {
            if url.starts_with(ORIGIN) || url.starts_with("about:") {
                return true;
            }
            open_outside(&url, test_mode, &nav_report);
            false
        })
        .with_new_window_req_handler(move |url, _features| {
            open_outside(&url, test_mode, &window_report);
            NewWindowResponse::Deny
        })
        .with_ipc_handler(move |message| report(&ipc_report, message.body()))
        .with_on_page_load_handler(move |event, _url| {
            if matches!(event, PageLoadEvent::Finished) {
                let _ = page_proxy.send_event(UserEvent::PageLoaded);
            }
        })
        .build(&window)
        .expect("the app window's content could not be created");

    let open_item = MenuItem::new("Open Job Board", true, None);
    let scan_item = MenuItem::new("Check for Jobs Now", true, None);
    let login = CheckMenuItem::new("Start at Login", true, login_item::is_enabled(), None);
    let quit_item = MenuItem::new("Quit", true, None);
    let tray_menu = Menu::new();
    if let Err(e) = tray_menu.append_items(&[
        &open_item,
        &scan_item,
        &PredefinedMenuItem::separator(),
        &login,
        &PredefinedMenuItem::separator(),
        &quit_item,
    ]) {
        log_warn!("could not build the menu-bar menu: {e}");
    }

    let mut tray: Option<TrayIcon> = None;
    let mut test_script = hooks.script;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        // Owned by the event loop so they live as long as the app does.
        let (_menu_bar, _webview) = (&menu_bar, &webview);
        match event {
            // The menu-bar icon has to be created once the event loop is running.
            Event::NewEvents(StartCause::Init) => {
                let built = TrayIconBuilder::new()
                    .with_menu(Box::new(tray_menu.clone()))
                    .with_tooltip("JobSearch")
                    .with_icon_templated(menu_bar_icon())
                    .build();
                match built {
                    Ok(icon) => {
                        icon.set_title(count_label(state.db.count_new()));
                        tray = Some(icon);
                    }
                    Err(e) => log_warn!("could not create the menu-bar icon: {e}"),
                }
            }
            // Closing the window hides it; the app keeps checking for jobs.
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => window.set_visible(false),
            // The Dock icon was clicked.
            Event::Reopen { .. } => show(&window),
            Event::UserEvent(UserEvent::PageLoaded) => {
                if let Some(script) = test_script.take() {
                    log_info!("running the UI test script");
                    if let Err(e) = webview.evaluate_script(&script) {
                        log_warn!("could not run the UI test script: {e}");
                    }
                }
            }
            Event::UserEvent(UserEvent::NewCount(count)) => {
                if let Some(tray) = &tray {
                    tray.set_title(count_label(count));
                }
            }
            Event::UserEvent(UserEvent::Menu(event)) => {
                if event.id == *open_item.id() {
                    show(&window);
                } else if event.id == *scan_item.id() {
                    state.request_scan();
                } else if event.id == *login.id() {
                    // The checkmark has already toggled; make the system match it.
                    let wanted = login.is_checked();
                    if let Err(e) = login_item::set_enabled(wanted) {
                        log_warn!("could not change Start at Login: {e:#}");
                        login.set_checked(!wanted);
                    }
                } else if event.id == *quit_item.id() {
                    tray.take();
                    *control_flow = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    })
}
