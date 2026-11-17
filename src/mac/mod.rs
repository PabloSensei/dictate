//! macOS UI: a menu bar item (NSStatusItem) with the app menu, the bubble (a
//! borderless non-activating NSPanel whose layer shows the rendered frame), and
//! permission prompts. Core events arrive on the main queue through GCD.
//! Settings are edited in dictate.ini (menu > Settings).

use std::cell::RefCell;
use std::ffi::c_void;

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEvent, NSImage, NSMenu,
    NSMenuDelegate, NSMenuItem, NSPanel, NSScreen, NSStatusBar, NSStatusItem, NSView, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer};
use objc2_quartz_core::{CALayer, CATransaction};

use crate::app::{self, UiEvent, shared};
use crate::unix::{self, Cli, anim::Anim};
use dictate::i18n::{self, T, t};
use dictate::{autostart, clipboard, config, inject, log};

const TAG_SETTINGS: isize = 1;
const TAG_LOG: isize = 2;
const TAG_RESTART: isize = 3;
const TAG_QUIT: isize = 4;
const TAG_AUTOSTART: isize = 5;
const TAG_ACCESSIBILITY: isize = 6;
const TAG_LANG: isize = 100; // 100 = system, 101.. = i18n::LANGS
const TAG_HISTORY: isize = 1000;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const c_void;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryCreate(
        alloc: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        n: isize,
        key_cb: *const c_void,
        value_cb: *const c_void,
    ) -> *const c_void;
    fn CFRelease(p: *const c_void);
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    static kCFBooleanTrue: *const c_void;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DictateController"]
    struct Controller;

    impl Controller {
        #[unsafe(method(menuAction:))]
        fn menu_action(&self, item: &NSMenuItem) {
            on_menu(item.tag());
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            with_ui(|ui| ui.tick_bubble());
        }
    }

    unsafe impl NSObjectProtocol for Controller {}

    unsafe impl NSMenuDelegate for Controller {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            build_menu(menu, self);
        }
    }
);

impl Controller {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

struct Bubble {
    panel: Retained<NSPanel>,
    view: Retained<NSView>,
    /// Hosted by the view (set before wantsLayer), so AppKit never redraws over it.
    layer: Retained<CALayer>,
    anim: Anim,
    timer: Option<Retained<NSTimer>>,
    scale: f64,
}

struct Ui {
    mtm: MainThreadMarker,
    controller: Retained<Controller>,
    item: Retained<NSStatusItem>,
    icons: Vec<Retained<NSImage>>,
    bubble: Option<Bubble>,
    alerted: Vec<String>,
    lock: Option<unix::InstanceLock>,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    /// Full texts behind the History menu items, by tag offset.
    static HISTORY_TEXTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn with_ui(f: impl FnOnce(&mut Ui)) {
    UI.with(|u| {
        if let Ok(mut g) = u.try_borrow_mut()
            && let Some(ui) = g.as_mut()
        {
            f(ui);
        }
    });
}

fn post_event(ev: UiEvent) {
    DispatchQueue::main().exec_async(move || handle(ev));
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// A CGImage from 0xAARRGGBB pixels (premultiplied or straight alpha).
fn cg_image(px: &[u32], w: usize, h: usize, premultiplied: bool) -> Option<CFRetained<CGImage>> {
    let bytes: Vec<u8> = px.iter().flat_map(|p| p.to_le_bytes()).collect(); // B, G, R, A
    let data = unsafe { CFData::new(None, bytes.as_ptr(), bytes.len() as isize) }?;
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    let space = CGColorSpace::new_device_rgb()?;
    let alpha = if premultiplied { CGImageAlphaInfo::PremultipliedFirst } else { CGImageAlphaInfo::First };
    let info = CGBitmapInfo(alpha.0 | CGImageByteOrderInfo::Order32Little.0);
    unsafe {
        CGImage::new(
            w,
            h,
            8,
            32,
            w * 4,
            Some(&space),
            info,
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

fn alert(title: &str, text: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    let a = NSAlert::new(mtm);
    a.setMessageText(&ns(title));
    a.setInformativeText(&ns(text));
    a.runModal();
}

/// Accessibility (typing) and Input Monitoring (reading the hotkey): macOS asks once.
fn request_permissions() {
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let opts = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            (&raw const kCFTypeDictionaryKeyCallBacks).cast(),
            (&raw const kCFTypeDictionaryValueCallBacks).cast(),
        );
        let trusted = AXIsProcessTrustedWithOptions(opts);
        if !opts.is_null() {
            CFRelease(opts);
        }
        let mut listen = CGPreflightListenEventAccess();
        if !listen {
            listen = CGRequestListenEventAccess();
        }
        log::write(&format!("permissions: accessibility={trusted} input_monitoring={listen}"));
    }
}

pub fn main() {
    let carried = app::take_handoff();
    i18n::set(i18n::resolve("auto"));
    let lock = match unix::lock_instance() {
        Ok(l) => l,
        Err(e) => {
            log::write(&format!("error: {e}"));
            eprintln!("dictate: {e}");
            return;
        }
    };
    if let Cli::Done = unix::command_line(lock.is_some()) {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else { return };
    let nsapp = NSApplication::sharedApplication(mtm);
    nsapp.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let cfg_path = config::default_path();
    let cfg = match config::load_or_create(&cfg_path) {
        Ok(c) => c,
        Err(e) => {
            log::write(&format!("error: {e}"));
            alert(t(T::AppName), &format!("{}:\n{e}", t(T::ConfigError)));
            return;
        }
    };
    i18n::set(i18n::resolve(&cfg.ui_language));
    let overlay = cfg.overlay;
    app::init(cfg, cfg_path, carried);
    app::set_poster(post_event);
    request_permissions();

    let controller = Controller::new(mtm);
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(-2.0); // NSSquareStatusItemLength
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    menu.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    item.setMenu(Some(&menu));
    let icons = dictate::icon::LOOK_COLORS
        .iter()
        .filter_map(|&c| {
            let px = dictate::icon::tray_pixels(36, c);
            let img = cg_image(&px, 36, 36, false)?;
            Some(NSImage::initWithCGImage_size(NSImage::alloc(), &img, NSSize::new(18.0, 18.0)))
        })
        .collect();
    let bubble = (overlay != config::Overlay::Off).then(|| make_bubble(mtm, overlay));
    UI.with(|u| *u.borrow_mut() = Some(Ui { mtm, controller, item, icons, bubble, alerted: Vec::new(), lock }));
    with_ui(|ui| ui.refresh_item());

    app::start_threads();
    nsapp.run();
}

fn make_bubble(mtm: MainThreadMarker, mode: config::Overlay) -> Bubble {
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(10.0, 10.0));
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let panel =
        NSPanel::initWithContentRect_styleMask_backing_defer(NSPanel::alloc(mtm), rect, style, NSBackingStoreType::Buffered, false);
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setIgnoresMouseEvents(true);
    panel.setLevel(25); // NSStatusWindowLevel: above normal windows and the Dock
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    let view = NSView::initWithFrame(NSView::alloc(mtm), rect);
    let layer = CALayer::new();
    view.setLayer(Some(&layer));
    view.setWantsLayer(true);
    panel.setContentView(Some(&view));
    Bubble { panel, view, layer, anim: Anim::new(mode), timer: None, scale: 2.0 }
}

impl Ui {
    fn refresh_item(&self) {
        let (look, status) = app::current_look();
        if let Some(button) = self.item.button(self.mtm) {
            if let Some(icon) = self.icons.get(look as usize) {
                button.setImage(Some(icon));
            }
            button.setToolTip(Some(&ns(&format!("{} — {status}", t(T::AppName)))));
        }
    }

    fn set_bubble(&mut self, phase: Option<dictate::bubble::Phase>) {
        let mtm = self.mtm;
        let controller = self.controller.clone();
        let Some(b) = self.bubble.as_mut() else { return };
        if !b.anim.set_target(phase) {
            return;
        }
        // the screen with the mouse; visibleFrame leaves out the menu bar and the Dock
        let mouse = NSEvent::mouseLocation();
        let screens = NSScreen::screens(mtm);
        let screen = screens
            .iter()
            .find(|s| {
                let f = s.frame();
                mouse.x >= f.origin.x
                    && mouse.x < f.origin.x + f.size.width
                    && mouse.y >= f.origin.y
                    && mouse.y < f.origin.y + f.size.height
            })
            .or_else(|| NSScreen::mainScreen(mtm));
        let Some(screen) = screen else { return };
        let vf = screen.visibleFrame();
        let scale = screen.backingScaleFactor().max(1.0);
        b.scale = scale;
        b.anim.set_scale(scale as f32);
        let (px, py) = b.anim.position(0, 0, (vf.size.width * scale) as i32, (vf.size.height * scale) as i32);
        let (w, h) = (b.anim.w as f64 / scale, b.anim.h as f64 / scale);
        let x = vf.origin.x + px as f64 / scale;
        let y = vf.origin.y + vf.size.height - py as f64 / scale - h;
        b.panel.setFrame_display(NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)), false);
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h));
        b.view.setFrame(bounds);
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        b.layer.setFrame(bounds);
        CATransaction::commit();
        b.panel.orderFrontRegardless();
        if b.timer.is_none() {
            let target: &AnyObject = controller.as_ref();
            b.timer = Some(unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    1.0 / 60.0,
                    target,
                    sel!(tick:),
                    None,
                    true,
                )
            });
        }
        self.tick_bubble();
    }

    fn tick_bubble(&mut self) {
        let Some(b) = self.bubble.as_mut() else { return };
        if b.anim.tick() {
            if let Some(img) = cg_image(&b.anim.buf, b.anim.w as usize, b.anim.h as usize, true) {
                let obj: &AnyObject = unsafe { &*((&*img) as *const CGImage as *const AnyObject) };
                // no implicit cross-fade between frames
                CATransaction::begin();
                CATransaction::setDisableActions(true);
                unsafe { b.layer.setContents(Some(obj)) };
                b.layer.setContentsScale(b.scale);
                CATransaction::commit();
            }
        } else {
            b.panel.orderOut(None);
            if let Some(timer) = b.timer.take() {
                timer.invalidate();
            }
        }
    }
}

fn handle(ev: UiEvent) {
    match ev {
        UiEvent::Refresh => with_ui(|ui| {
            ui.refresh_item();
            ui.set_bubble(app::bubble_phase());
        }),
        UiEvent::Preview => with_ui(|ui| {
            let text = shared().preview.lock().unwrap().clone();
            if let Some(b) = ui.bubble.as_mut() {
                b.anim.set_caption(&text);
            }
        }),
        UiEvent::History => {}
        UiEvent::CopyText => {
            if let Some(text) = app::take_clipboard_text()
                && let Err(e) = clipboard::set_text(&text)
            {
                app::set_error(e);
            }
        }
        UiEvent::ShowError => {
            // each distinct error once per run as an alert; it stays in the menu anyway
            let Some(e) = shared().error.lock().unwrap().clone() else { return };
            let mut first = false;
            with_ui(|ui| {
                if !ui.alerted.contains(&e) {
                    ui.alerted.push(e.clone());
                    first = true;
                }
            });
            if first {
                alert(t(T::AppName), &e);
            }
        }
    }
}

fn add_item(menu: &NSMenu, target: &Controller, title: &str, tag: isize, enabled: bool, checked: bool) -> Retained<NSMenuItem> {
    let mtm = MainThreadMarker::new().expect("main thread");
    let it = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(title), Some(sel!(menuAction:)), &ns(""))
    };
    let obj: &AnyObject = target.as_ref();
    unsafe { it.setTarget(Some(obj)) };
    it.setTag(tag);
    it.setEnabled(enabled);
    it.setState(if checked { 1 } else { 0 });
    menu.addItem(&it);
    it
}

fn add_submenu(menu: &NSMenu, title: &str) -> Retained<NSMenu> {
    let mtm = MainThreadMarker::new().expect("main thread");
    let it = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(title), None, &ns("")) };
    let sub = NSMenu::new(mtm);
    sub.setAutoenablesItems(false);
    it.setSubmenu(Some(&sub));
    menu.addItem(&it);
    sub
}

fn separator(menu: &NSMenu) {
    let mtm = MainThreadMarker::new().expect("main thread");
    menu.addItem(&NSMenuItem::separatorItem(mtm));
}

/// Rebuilt every time the menu opens, so it shows the current state.
fn build_menu(menu: &NSMenu, c: &Controller) {
    menu.removeAllItems();
    let sh = shared();
    let (_, status) = app::current_look();
    let model = sh.cfg.model.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let last = sh.last.lock().unwrap().clone();
    let err = sh.error.lock().unwrap().clone();
    let ui_lang = sh.ui_lang.lock().unwrap().clone();
    let plain = |s: &str| s.replace('&', "");

    add_item(menu, c, &status, 0, false, false);
    add_item(menu, c, &format!("{}: {model}", t(T::MenuModel)), 0, false, false);
    if !last.is_empty() {
        add_item(menu, c, &format!("{}: {last}", t(T::MenuLast)), 0, false, false);
    }
    if let Some(e) = err {
        add_item(menu, c, &format!("{}: {}", t(T::StError), e.lines().last().unwrap_or("")), 0, false, false);
    }
    if !inject::trusted() {
        add_item(menu, c, "Allow typing (Accessibility)…", TAG_ACCESSIBILITY, true, false);
    }
    separator(menu);
    add_item(menu, c, &plain(t(T::MenuSettings)), TAG_SETTINGS, true, false);

    let hist = add_submenu(menu, &plain(t(T::MenuHistory)));
    let entries = unix::menu_entries(15);
    if entries.is_empty() {
        add_item(&hist, c, t(T::HistoryEmpty), 0, false, false);
    }
    HISTORY_TEXTS.with(|h| {
        let mut h = h.borrow_mut();
        h.clear();
        for (i, (label, text)) in entries.into_iter().enumerate() {
            add_item(&hist, c, &label, TAG_HISTORY + i as isize, true, false);
            h.push(text);
        }
    });

    let langs = add_submenu(menu, &plain(t(T::MenuUiLang)));
    add_item(&langs, c, t(T::MenuSystemLang), TAG_LANG, true, ui_lang == "auto");
    separator(&langs);
    for (i, l) in i18n::LANGS.iter().enumerate() {
        add_item(&langs, c, l.native_name(), TAG_LANG + 1 + i as isize, true, ui_lang == l.code());
    }
    let auto_on = std::env::current_exe().is_ok_and(|e| autostart::is_enabled(&e));
    add_item(menu, c, &plain(t(T::MenuAutostart)), TAG_AUTOSTART, true, auto_on);
    separator(menu);
    add_item(menu, c, &plain(t(T::MenuLog)), TAG_LOG, true, false);
    add_item(menu, c, &plain(t(T::MenuRestart)), TAG_RESTART, true, false);
    separator(menu);
    add_item(menu, c, &plain(t(T::MenuQuit)), TAG_QUIT, true, false);
}

/// Opens a text file in the default text editor (TextEdit unless changed).
fn open_text(p: &std::path::Path) {
    if let Err(e) = std::process::Command::new("open").arg("-t").arg(p).spawn() {
        log::write(&format!("open {}: {e}", p.display()));
    }
}

fn on_menu(tag: isize) {
    match tag {
        TAG_SETTINGS => open_text(&shared().cfg_path),
        TAG_LOG => {
            if let Some(p) = log::path() {
                open_text(&p);
            }
        }
        TAG_ACCESSIBILITY => {
            let _ = std::process::Command::new("open")
                .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
                .spawn();
        }
        TAG_AUTOSTART => {
            if let Ok(exe) = std::env::current_exe() {
                let on = !autostart::is_enabled(&exe);
                match autostart::set(on, &exe) {
                    Ok(()) => log::write(&format!("autostart {}", if on { "on" } else { "off" })),
                    Err(e) => app::set_error(e),
                }
            }
        }
        TAG_RESTART => {
            with_ui(|ui| drop(ui.lock.take()));
            app::spawn_successor();
            quit();
        }
        TAG_QUIT => quit(),
        t if (TAG_LANG..=TAG_LANG + i18n::LANGS.len() as isize).contains(&t) => {
            let code = if t == TAG_LANG { "auto" } else { i18n::LANGS[(t - TAG_LANG - 1) as usize].code() };
            app::set_ui_language(code);
            with_ui(|ui| ui.refresh_item());
        }
        t if t >= TAG_HISTORY => {
            let text = HISTORY_TEXTS.with(|h| h.borrow().get((t - TAG_HISTORY) as usize).cloned());
            if let Some(text) = text
                && let Err(e) = clipboard::set_text(&text)
            {
                app::set_error(e);
            }
        }
        _ => {}
    }
}

fn quit() {
    log::write("exit");
    // Worker threads may be inside whisper.cpp; exiting the process is the clean way out.
    std::process::exit(0);
}
