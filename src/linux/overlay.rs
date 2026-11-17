//! The bubble on X11: an override-redirect window with a 32-bit ARGB visual (the
//! compositor blends it), an empty input shape (clicks go through), never focused.
//! Placed on the monitor under the mouse, inside the work area.

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    AtomEnum, ClipOrdering, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux,
    EventMask, ImageFormat, PropMode, StackMode, VisualClass, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use dictate::bubble::Phase;
use dictate::config::Overlay;

use crate::unix::anim::Anim;

pub struct Bubble {
    conn: RustConnection,
    root: u32,
    win: u32,
    gc: u32,
    anim: Anim,
    shown: bool,
}

impl Bubble {
    pub fn new(mode: Overlay) -> Result<Option<Self>, String> {
        if mode == Overlay::Off {
            return Ok(None);
        }
        let e = |e: &dyn std::fmt::Display| format!("bubble: {e}");
        let (conn, screen_num) = RustConnection::connect(None).map_err(|x| e(&x))?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let visual = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .map(|v| v.visual_id)
            .ok_or("bubble: the X server has no 32-bit visual, so no transparent windows")?;
        let cmap = conn.generate_id().map_err(|x| e(&x))?;
        conn.create_colormap(ColormapAlloc::NONE, cmap, root, visual).map_err(|x| e(&x))?;
        let win = conn.generate_id().map_err(|x| e(&x))?;
        let aux = CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .override_redirect(1)
            .colormap(cmap)
            .event_mask(EventMask::NO_EVENT);
        conn.create_window(32, win, root, 0, 0, 1, 1, 0, WindowClass::INPUT_OUTPUT, visual, &aux).map_err(|x| e(&x))?;
        // empty input region: the mouse goes to whatever is under the bubble
        conn.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, win, 0, 0, &[]).map_err(|x| e(&x))?;
        let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).ok().and_then(|c| c.reply().ok()).map(|r| r.atom);
        if let (Some(ty), Some(note)) = (atom("_NET_WM_WINDOW_TYPE"), atom("_NET_WM_WINDOW_TYPE_NOTIFICATION")) {
            let _ = conn.change_property32(PropMode::REPLACE, win, ty, AtomEnum::ATOM, &[note]);
        }
        let _ = conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, b"dictate bubble");
        let gc = conn.generate_id().map_err(|x| e(&x))?;
        conn.create_gc(gc, win, &CreateGCAux::new()).map_err(|x| e(&x))?;
        conn.flush().map_err(|x| e(&x))?;
        Ok(Some(Self { conn, root, win, gc, anim: Anim::new(mode), shown: false }))
    }

    pub fn visible(&self) -> bool {
        self.anim.visible()
    }

    pub fn set_target(&mut self, phase: Option<Phase>) {
        if self.anim.set_target(phase) {
            let (scale, work) = self.placement();
            self.anim.set_scale(scale);
            let (x, y) = self.anim.position(work.0, work.1, work.2, work.3);
            let aux = ConfigureWindowAux::new()
                .x(x)
                .y(y)
                .width(self.anim.w as u32)
                .height(self.anim.h as u32)
                .stack_mode(StackMode::ABOVE);
            let _ = self.conn.configure_window(self.win, &aux);
            self.tick();
        }
    }

    pub fn set_caption(&mut self, text: &str) {
        self.anim.set_caption(text);
    }

    pub fn tick(&mut self) {
        if !self.anim.visible() && !self.shown {
            return;
        }
        let still = self.anim.tick();
        if still {
            let bytes: Vec<u8> = self.anim.buf.iter().flat_map(|p| p.to_le_bytes()).collect();
            let (w, h) = (self.anim.w as u16, self.anim.h as u16);
            let _ = self.conn.put_image(ImageFormat::Z_PIXMAP, self.win, self.gc, w, h, 0, 0, 0, 32, &bytes);
            if !self.shown {
                let _ = self.conn.map_window(self.win);
                let _ = self.conn.configure_window(self.win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
                self.shown = true;
            }
        } else if self.shown {
            let _ = self.conn.unmap_window(self.win);
            self.shown = false;
        }
        let _ = self.conn.flush();
    }

    /// Display scale (Xft.dpi / 96, else GDK_SCALE) and the work area of the monitor
    /// under the mouse, in pixels: (left, top, right, bottom).
    fn placement(&self) -> (f32, (i32, i32, i32, i32)) {
        let c = &self.conn;
        let screen = &c.setup().roots[0];
        let mut area = (0, 0, screen.width_in_pixels as i32, screen.height_in_pixels as i32);
        let pointer = c.query_pointer(self.root).ok().and_then(|r| r.reply().ok());
        let (px, py) = pointer.map(|p| (p.root_x as i32, p.root_y as i32)).unwrap_or((0, 0));
        if let Some(mons) = c.randr_get_monitors(self.root, true).ok().and_then(|r| r.reply().ok()) {
            let inside = |m: &&x11rb::protocol::randr::MonitorInfo| {
                (m.x as i32..m.x as i32 + m.width as i32).contains(&px) && (m.y as i32..m.y as i32 + m.height as i32).contains(&py)
            };
            if let Some(m) = mons.monitors.iter().find(inside).or_else(|| mons.monitors.iter().find(|m| m.primary)) {
                area = (m.x as i32, m.y as i32, m.x as i32 + m.width as i32, m.y as i32 + m.height as i32);
            }
        }
        // _NET_WORKAREA leaves out panels and docks (one box for all monitors, so intersect)
        let atom = c.intern_atom(true, b"_NET_WORKAREA").ok().and_then(|r| r.reply().ok()).map(|r| r.atom);
        if let Some(a) = atom.filter(|&a| a != 0)
            && let Some(p) = c.get_property(false, self.root, a, AtomEnum::CARDINAL, 0, 4).ok().and_then(|r| r.reply().ok())
            && let Some(v) = p.value32().map(|v| v.collect::<Vec<u32>>())
            && v.len() == 4
        {
            let (l, t, r, b) = (v[0] as i32, v[1] as i32, (v[0] + v[2]) as i32, (v[1] + v[3]) as i32);
            let cut = (area.0.max(l), area.1.max(t), area.2.min(r), area.3.min(b));
            if cut.0 < cut.2 && cut.1 < cut.3 {
                area = cut;
            }
        }
        (scale(c, self.root), area)
    }
}

fn scale(c: &RustConnection, root: u32) -> f32 {
    let from_xft = c
        .get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 16)
        .ok()
        .and_then(|r| r.reply().ok())
        .and_then(|p| {
            let s = String::from_utf8_lossy(&p.value).into_owned();
            s.lines().find_map(|l| l.strip_prefix("Xft.dpi:").and_then(|v| v.trim().parse::<f32>().ok()))
        })
        .map(|dpi| dpi / 96.0);
    let from_env = std::env::var("GDK_SCALE").ok().and_then(|v| v.parse::<f32>().ok());
    from_xft.or(from_env).unwrap_or(1.0).clamp(1.0, 4.0)
}
