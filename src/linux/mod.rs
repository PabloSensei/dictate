//! Linux UI (X11): a StatusNotifierItem tray icon (KDE, GNOME with the AppIndicator
//! extension that Ubuntu ships, XFCE...), the bubble, and a main loop that drains
//! core events. Settings are edited in dictate.ini (tray menu > Settings).

mod overlay;

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip};

use crate::app::{self, UiEvent, shared};
use crate::unix::{self, Cli};
use dictate::i18n::{self, T, t};
use dictate::{autostart, clipboard, config, keys, log};

enum Msg {
    Ui(UiEvent),
    OpenSettings,
    OpenLog,
    Copy(String),
    ToggleAutostart,
    UiLang(&'static str),
    Restart,
    Quit,
}

static TX: OnceLock<Mutex<Sender<Msg>>> = OnceLock::new();

fn send(m: Msg) {
    if let Some(tx) = TX.get() {
        let _ = tx.lock().unwrap().send(m);
    }
}

fn post_event(ev: UiEvent) {
    send(Msg::Ui(ev));
}

fn fatal(msg: &str) {
    log::write(&format!("error: {}", msg.replace('\n', " | ")));
    eprintln!("dictate: {msg}");
    notify(msg);
}

/// A desktop notification through notify-send, if it is installed.
fn notify(msg: &str) {
    let _ = std::process::Command::new("notify-send")
        .args(["--app-name=Dictate", "--icon=audio-input-microphone", "Dictate", msg])
        .spawn();
}

pub fn main() {
    let carried = app::take_handoff();
    i18n::set(i18n::resolve("auto"));
    let lock = match unix::lock_instance() {
        Ok(l) => l,
        Err(e) => return fatal(&e),
    };
    if let Cli::Done = unix::command_line(lock.is_some()) {
        return;
    }
    if std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland")) {
        let w = "This is a Wayland session. dictate supports X11 only for now: on the login screen, \
                 pick \"Ubuntu on Xorg\" (gear icon). Under Wayland the hotkey and typing work only in X11 apps.";
        log::write(w);
        eprintln!("dictate: {w}");
        notify(w);
    }
    if !keys::available() {
        return fatal("no X11 display (DISPLAY is not set or the X server refuses connections)");
    }

    let cfg_path = config::default_path();
    let cfg = match config::load_or_create(&cfg_path) {
        Ok(c) => c,
        Err(e) => return fatal(&format!("{}:\n{e}", t(T::ConfigError))),
    };
    i18n::set(i18n::resolve(&cfg.ui_language));
    let overlay_mode = cfg.overlay;
    app::init(cfg, cfg_path, carried);
    let (tx, rx) = mpsc::channel();
    let _ = TX.set(Mutex::new(tx));
    app::set_poster(post_event);

    let tray = match (DictTray {}).spawn() {
        Ok(h) => Some(h),
        Err(e) => {
            log::write(&format!("no tray icon: {e}"));
            eprintln!(
                "dictate: no tray icon ({e}). On GNOME, enable the AppIndicator extension. \
                 Stop dictate with `dictate --quit`."
            );
            None
        }
    };
    let bubble = match overlay::Bubble::new(overlay_mode) {
        Ok(b) => b,
        Err(e) => {
            log::write(&e);
            None
        }
    };
    app::start_threads();
    eprintln!("dictate: running. Hold {} and speak.", config::key_label(shared().cfg.hotkey));
    run(rx, tray, bubble, lock);
}

fn run(rx: Receiver<Msg>, tray: Option<Handle<DictTray>>, mut bubble: Option<overlay::Bubble>, mut lock: Option<unix::InstanceLock>) {
    let refresh_tray = |tray: &Option<Handle<DictTray>>| {
        if let Some(h) = tray {
            h.update(|_| {});
        }
    };
    loop {
        let animating = bubble.as_ref().is_some_and(|b| b.visible());
        let msg = if animating {
            match rx.recv_timeout(Duration::from_millis(16)) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            }
        };
        match msg {
            None => {}
            Some(Msg::Ui(UiEvent::Refresh)) => {
                refresh_tray(&tray);
                if let Some(b) = bubble.as_mut() {
                    b.set_target(app::bubble_phase());
                }
            }
            Some(Msg::Ui(UiEvent::Preview)) => {
                if let Some(b) = bubble.as_mut() {
                    b.set_caption(&shared().preview.lock().unwrap().clone());
                }
            }
            Some(Msg::Ui(UiEvent::ShowError)) => {
                if let Some(e) = shared().error.lock().unwrap().clone() {
                    eprintln!("dictate: {e}");
                    notify(&e);
                }
            }
            Some(Msg::Ui(UiEvent::History)) => refresh_tray(&tray),
            Some(Msg::Ui(UiEvent::CopyText)) => {
                if let Some(text) = app::take_clipboard_text()
                    && let Err(e) = clipboard::set_text(&text)
                {
                    app::set_error(e);
                }
            }
            Some(Msg::Copy(text)) => {
                if let Err(e) = clipboard::set_text(&text) {
                    app::set_error(e);
                }
            }
            Some(Msg::OpenSettings) => unix::open_path(&shared().cfg_path),
            Some(Msg::OpenLog) => {
                if let Some(p) = log::path() {
                    unix::open_path(&p);
                }
            }
            Some(Msg::ToggleAutostart) => {
                if let Ok(exe) = std::env::current_exe() {
                    let on = !autostart::is_enabled(&exe);
                    match autostart::set(on, &exe) {
                        Ok(()) => log::write(&format!("autostart {}", if on { "on" } else { "off" })),
                        Err(e) => app::set_error(e),
                    }
                }
                refresh_tray(&tray);
            }
            Some(Msg::UiLang(code)) => {
                app::set_ui_language(code);
                refresh_tray(&tray);
            }
            Some(Msg::Restart) => {
                drop(lock.take());
                app::spawn_successor();
                break;
            }
            Some(Msg::Quit) => break,
        }
        if let Some(b) = bubble.as_mut() {
            b.tick();
        }
    }
    log::write("exit");
    // Worker threads may be inside whisper.cpp; exiting the process is the clean way out.
    std::process::exit(0);
}

struct DictTray {}

fn icon(look: app::Look) -> Vec<Icon> {
    [22, 44]
        .into_iter()
        .map(|s| {
            let px = dictate::icon::tray_pixels(s, dictate::icon::LOOK_COLORS[look as usize]);
            Icon { width: s, height: s, data: px.iter().flat_map(|p| p.to_be_bytes()).collect() } // ARGB, big-endian
        })
        .collect()
}

fn info(label: String) -> MenuItem<DictTray> {
    StandardItem { label, enabled: false, ..Default::default() }.into()
}

fn item(label: &str, f: fn() -> Msg) -> MenuItem<DictTray> {
    StandardItem { label: label.replace('&', ""), activate: Box::new(move |_| send(f())), ..Default::default() }.into()
}

impl ksni::Tray for DictTray {
    fn id(&self) -> String {
        "dictate".into()
    }

    fn title(&self) -> String {
        t(T::AppName).into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        icon(app::current_look().0)
    }

    fn tool_tip(&self) -> ToolTip {
        let (_, status) = app::current_look();
        ToolTip { title: t(T::AppName).into(), description: status, ..Default::default() }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let sh = shared();
        let (_, status) = app::current_look();
        let model = sh.cfg.model.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let last = sh.last.lock().unwrap().clone();
        let err = sh.error.lock().unwrap().clone();
        let ui_lang = sh.ui_lang.lock().unwrap().clone();

        let mut m = vec![info(status), info(format!("{}: {model}", t(T::MenuModel)))];
        if !last.is_empty() {
            m.push(info(format!("{}: {last}", t(T::MenuLast))));
        }
        if let Some(e) = err {
            m.push(info(format!("{}: {}", t(T::StError), e.lines().last().unwrap_or(""))));
        }
        m.push(MenuItem::Separator);
        m.push(item(t(T::MenuSettings), || Msg::OpenSettings));

        // History: the latest dictations; a click copies one to the clipboard.
        let recent = unix::menu_entries(15);
        let history: Vec<MenuItem<Self>> = if recent.is_empty() {
            vec![info(t(T::HistoryEmpty).into())]
        } else {
            recent
                .into_iter()
                .map(|(label, text)| {
                    StandardItem { label, activate: Box::new(move |_| send(Msg::Copy(text.clone()))), ..Default::default() }
                        .into()
                })
                .collect()
        };
        m.push(SubMenu { label: t(T::MenuHistory).replace('&', ""), submenu: history, ..Default::default() }.into());

        let mut langs: Vec<MenuItem<Self>> = vec![
            CheckmarkItem {
                label: t(T::MenuSystemLang).into(),
                checked: ui_lang == "auto",
                activate: Box::new(|_| send(Msg::UiLang("auto"))),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
        ];
        for l in i18n::LANGS {
            let code = l.code();
            langs.push(
                CheckmarkItem {
                    label: l.native_name().into(),
                    checked: ui_lang == code,
                    activate: Box::new(move |_| send(Msg::UiLang(code))),
                    ..Default::default()
                }
                .into(),
            );
        }
        m.push(SubMenu { label: t(T::MenuUiLang).into(), submenu: langs, ..Default::default() }.into());
        let auto_on = std::env::current_exe().is_ok_and(|e| autostart::is_enabled(&e));
        m.push(
            CheckmarkItem {
                label: t(T::MenuAutostart).into(),
                checked: auto_on,
                activate: Box::new(|_| send(Msg::ToggleAutostart)),
                ..Default::default()
            }
            .into(),
        );
        m.push(MenuItem::Separator);
        m.push(item(t(T::MenuLog), || Msg::OpenLog));
        m.push(item(t(T::MenuRestart), || Msg::Restart));
        m.push(MenuItem::Separator);
        m.push(item(t(T::MenuQuit), || Msg::Quit));
        m
    }
}
