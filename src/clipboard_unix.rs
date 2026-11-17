//! Puts text on the clipboard (macOS and Linux, through arboard). Never reads it.
//!
//! On X11 the clipboard belongs to whoever owns the selection, so the `Clipboard`
//! must live as long as the app: it is kept per thread (the UI thread) and reused.

use std::cell::RefCell;

thread_local! {
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

pub fn set_text(text: &str) -> Result<(), String> {
    CLIPBOARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = Some(arboard::Clipboard::new().map_err(|e| format!("clipboard: {e}"))?);
        }
        let r = c.as_mut().unwrap().set_text(text).map_err(|e| format!("clipboard: {e}"));
        if r.is_err() {
            *c = None; // start over next time
        }
        r
    })
}
