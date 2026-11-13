//! Hotkey state machine, fed by polling: turns key states into dictation events.
//!
//! Hold:   press starts capturing; the bubble shows after `min_hold`; release finishes.
//!         Shorter presses are dropped.
//! Toggle: a clean tap starts, the next clean tap finishes, Esc cancels.
//! Both:   if any other key is pressed together with the hotkey (RCtrl+C ...), that press
//!         is a shortcut, not dictation: it never starts or finishes anything, and a
//!         hold-mode recording in progress is cancelled.

use std::time::{Duration, Instant};

use crate::config::Mode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Open the microphone.
    Start,
    /// It's a real dictation now: show the bubble.
    Visible,
    /// Stop and transcribe.
    Finish,
    /// Stop and throw the audio away.
    Cancel,
}

/// A tap longer than this is not a tap (toggle mode).
const TAP_MAX: Duration = Duration::from_millis(800);

pub struct Trigger {
    mode: Mode,
    min_hold: Duration,
    max: Duration,
    was_down: bool,
    /// Current press: when it started and whether another key joined it.
    press: Option<(Instant, bool)>,
    /// Recording since, and whether it has been announced as visible.
    rec: Option<(Instant, bool)>,
    /// Ignore the key until it is released (after cancel / max length / failure).
    wait_release: bool,
}

impl Trigger {
    pub fn new(mode: Mode, min_hold: Duration, max: Duration) -> Self {
        Self { mode, min_hold, max, was_down: false, press: None, rec: None, wait_release: false }
    }

    pub fn recording(&self) -> bool {
        self.rec.is_some()
    }

    /// The microphone could not be opened: forget the recording.
    pub fn abort(&mut self) {
        self.rec = None;
        self.wait_release = true;
    }

    /// `down`: hotkey state; `other`: any other key is down; `esc`: Escape is down.
    pub fn tick(&mut self, now: Instant, down: bool, other: bool, esc: bool) -> Vec<Event> {
        let mut ev = Vec::new();
        let pressed = down && !self.was_down;
        let released = !down && self.was_down;
        self.was_down = down;

        if pressed {
            self.press = Some((now, false));
        }
        if down
            && other
            && let Some(p) = self.press.as_mut()
        {
            p.1 = true;
        }
        let combo = self.press.is_some_and(|p| p.1);

        match self.mode {
            Mode::Hold => {
                if pressed && self.rec.is_none() && !self.wait_release {
                    self.rec = Some((now, false));
                    ev.push(Event::Start);
                }
                if let Some((since, visible)) = self.rec {
                    if combo {
                        self.rec = None;
                        self.wait_release = down;
                        ev.push(Event::Cancel);
                    } else if released {
                        self.rec = None;
                        ev.push(if visible || now - since >= self.min_hold { Event::Finish } else { Event::Cancel });
                    } else if now - since >= self.max {
                        self.rec = None;
                        self.wait_release = true;
                        ev.push(Event::Finish);
                    } else if !visible && now - since >= self.min_hold {
                        self.rec = Some((since, true));
                        ev.push(Event::Visible);
                    }
                }
            }
            Mode::Toggle => {
                let tap = released && !combo && self.press.is_some_and(|(t0, _)| now - t0 < TAP_MAX);
                if let Some((since, _)) = self.rec {
                    if esc {
                        self.rec = None;
                        ev.push(Event::Cancel);
                    } else if tap || now - since >= self.max {
                        self.rec = None;
                        ev.push(Event::Finish);
                    }
                } else if tap && !self.wait_release {
                    self.rec = Some((now, true));
                    ev.push(Event::Start);
                    ev.push(Event::Visible);
                }
            }
        }

        if released {
            self.press = None;
            self.wait_release = false;
        }
        ev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sim {
        t: Instant,
        tr: Trigger,
    }

    impl Sim {
        fn new(mode: Mode) -> Self {
            Self { t: Instant::now(), tr: Trigger::new(mode, Duration::from_millis(300), Duration::from_secs(10)) }
        }
        /// Advance `ms` in 15 ms steps with the given inputs; collect events.
        fn run(&mut self, ms: u64, down: bool, other: bool, esc: bool) -> Vec<Event> {
            let mut out = Vec::new();
            let mut left = ms;
            loop {
                let step = left.min(15);
                self.t += Duration::from_millis(step);
                out.extend(self.tr.tick(self.t, down, other, esc));
                if left <= 15 {
                    break;
                }
                left -= step;
            }
            out
        }
    }

    use Event::*;

    #[test]
    fn hold_normal() {
        let mut s = Sim::new(Mode::Hold);
        assert_eq!(s.run(500, true, false, false), vec![Start, Visible]);
        assert_eq!(s.run(15, false, false, false), vec![Finish]);
    }

    #[test]
    fn hold_short_tap_is_dropped() {
        let mut s = Sim::new(Mode::Hold);
        assert_eq!(s.run(100, true, false, false), vec![Start]);
        assert_eq!(s.run(15, false, false, false), vec![Cancel]);
    }

    #[test]
    fn hold_combo_cancels_and_waits_for_release() {
        let mut s = Sim::new(Mode::Hold);
        assert_eq!(s.run(400, true, false, false), vec![Start, Visible]);
        assert_eq!(s.run(50, true, true, false), vec![Cancel]); // RCtrl+C
        assert_eq!(s.run(300, true, false, false), vec![]); // still held: nothing
        assert_eq!(s.run(15, false, false, false), vec![]);
        assert_eq!(s.run(15, true, false, false), vec![Start]); // fresh press works again
    }

    #[test]
    fn hold_max_length_finishes() {
        let mut s = Sim::new(Mode::Hold);
        let ev = s.run(10_100, true, false, false);
        assert_eq!(ev, vec![Start, Visible, Finish]);
        assert_eq!(s.run(15, false, false, false), vec![]);
    }

    #[test]
    fn toggle_tap_tap() {
        let mut s = Sim::new(Mode::Toggle);
        assert_eq!(s.run(120, true, false, false), vec![]);
        assert_eq!(s.run(15, false, false, false), vec![Start, Visible]);
        assert_eq!(s.run(3000, false, false, false), vec![]);
        assert_eq!(s.run(120, true, false, false), vec![]);
        assert_eq!(s.run(15, false, false, false), vec![Finish]);
    }

    #[test]
    fn toggle_ignores_combos_and_long_presses() {
        let mut s = Sim::new(Mode::Toggle);
        s.run(60, true, false, false);
        s.run(60, true, true, false); // RCtrl+C
        assert_eq!(s.run(15, false, false, false), vec![]);
        assert_eq!(s.run(1000, true, false, false), vec![]); // too long for a tap
        assert_eq!(s.run(15, false, false, false), vec![]);
        // while recording, RCtrl+C does not stop it
        s.run(100, true, false, false);
        assert_eq!(s.run(15, false, false, false), vec![Start, Visible]);
        s.run(60, true, true, false);
        assert_eq!(s.run(15, false, false, false), vec![]);
        assert!(s.tr.recording());
    }

    #[test]
    fn toggle_escape_cancels() {
        let mut s = Sim::new(Mode::Toggle);
        s.run(100, true, false, false);
        assert_eq!(s.run(15, false, false, false), vec![Start, Visible]);
        assert_eq!(s.run(30, false, false, true), vec![Cancel]);
        assert!(!s.tr.recording());
    }
}
