//! The remapping state machine.
//!
//! Feed it one frame of events from the mouse (everything up to, but not
//! including, a SYN_REPORT) and it appends what the virtual device should
//! emit, SYN_REPORTs included. It owns no devices and does no I/O.
//!
//! The one piece of state that matters is `held`: for every button currently
//! down, what its press emitted. A release always undoes exactly that, so a
//! profile switch while a button is held can never leave a key stuck.

use std::collections::HashMap;
use std::sync::Arc;

use crate::event::{
    EV_KEY, EV_MSC, EV_REL, Event, Key, REL_HWHEEL, REL_HWHEEL_HI_RES, REL_WHEEL, REL_WHEEL_HI_RES,
    ScrollAxis, Trigger,
};
use crate::profile::{Action, ResolvedProfile};

/// A single scroll event never taps a key more than this many times, however
/// large its value.
const MAX_TAPS_PER_SCROLL_EVENT: u32 = 8;

#[derive(Debug, Default)]
pub struct Engine {
    profile: Arc<ResolvedProfile>,
    /// Physical button -> the keys its press emitted (empty if disabled).
    held: HashMap<Key, Vec<Key>>,
    /// Output key -> how many held buttons are holding it down. Two buttons
    /// bound to the same key must not release it until both are up.
    down: HashMap<Key, u32>,
    /// Reused buffers, so handling a frame does not allocate.
    frame: Vec<Event>,
    taps: Vec<Event>,
}

impl Engine {
    /// An engine with an empty profile: everything passes through.
    pub fn new() -> Engine {
        Engine::default()
    }

    /// Switches the active profile. Buttons held right now keep what they
    /// emitted and release it normally; only later presses use the new one.
    pub fn set_profile(&mut self, profile: Arc<ResolvedProfile>) {
        self.profile = profile;
    }

    pub fn profile(&self) -> &ResolvedProfile {
        &self.profile
    }

    /// Handles one frame from the mouse, appending the output to `out`.
    pub fn process_frame(&mut self, input: &[Event], out: &mut Vec<Event>) {
        self.frame.clear();
        self.taps.clear();
        // The scan code that accompanies a button press names the original
        // button, so it is dropped from any frame where a button was changed.
        let mut changed_a_key = false;

        for &event in input {
            match event.kind {
                EV_KEY => changed_a_key |= self.handle_key(Key(event.code), event.value),
                EV_REL => self.handle_rel(event),
                _ => self.frame.push(event),
            }
        }

        if changed_a_key {
            self.frame.retain(|event| event.kind != EV_MSC);
        }
        end_frame(&mut self.frame);
        out.append(&mut self.frame);
        out.append(&mut self.taps);
    }

    /// Releases every key the engine is holding down. Used when remapping is
    /// paused and when the mouse disappears (it can no longer send releases).
    pub fn release_all(&mut self, out: &mut Vec<Event>) {
        self.held.clear();
        for (key, _) in self.down.drain() {
            out.push(Event::key(key, 0));
            out.push(Event::SYN);
        }
    }

    /// Returns true if the output differs from the input event.
    fn handle_key(&mut self, button: Key, value: i32) -> bool {
        match value {
            1 => {
                if self.held.contains_key(&button) {
                    // A second press with no release in between; ignore it.
                    return true;
                }
                let emitted = match self.action_for(Trigger::Key(button)) {
                    None => vec![button],
                    Some(Action::Keys(keys)) => keys.clone(),
                    Some(Action::Disabled) => Vec::new(),
                };
                for &key in &emitted {
                    press(&mut self.down, &mut self.frame, key);
                }
                let changed = emitted != [button];
                self.held.insert(button, emitted);
                changed
            }
            0 => match self.held.remove(&button) {
                Some(emitted) => {
                    for &key in emitted.iter().rev() {
                        release(&mut self.down, &mut self.frame, key);
                    }
                    emitted != [button]
                }
                // Not pressed as far as we know (held since before the grab).
                // Forwarding a stray release is harmless.
                None => {
                    self.frame.push(Event::key(button, 0));
                    false
                }
            },
            // Key repeat: only meaningful for a button that passes through.
            _ => {
                let passes_through = self
                    .held
                    .get(&button)
                    .is_some_and(|emitted| emitted[..] == [button]);
                if passes_through {
                    self.frame.push(Event::key(button, value));
                }
                !passes_through
            }
        }
    }

    fn handle_rel(&mut self, event: Event) {
        let (axis, hi_res) = match event.code {
            REL_WHEEL => (ScrollAxis::Vertical, false),
            REL_WHEEL_HI_RES => (ScrollAxis::Vertical, true),
            REL_HWHEEL => (ScrollAxis::Horizontal, false),
            REL_HWHEEL_HI_RES => (ScrollAxis::Horizontal, true),
            // Pointer motion and anything else: untouched.
            _ => return self.frame.push(event),
        };
        let trigger = Trigger::Scroll {
            axis,
            positive: event.value > 0,
        };
        let keys = match self.action_for(trigger) {
            None => return self.frame.push(event),
            Some(Action::Disabled) => return,
            Some(Action::Keys(keys)) => keys.clone(),
        };
        // The kernel reports each scroll twice: whole ticks and a
        // high-resolution stream. Both are swallowed; only whole ticks tap.
        if hi_res {
            return;
        }
        let ticks = event.value.unsigned_abs().min(MAX_TAPS_PER_SCROLL_EVENT);
        for _ in 0..ticks {
            for &key in &keys {
                press(&mut self.down, &mut self.taps, key);
            }
            end_frame(&mut self.taps);
            for &key in keys.iter().rev() {
                release(&mut self.down, &mut self.taps, key);
            }
            end_frame(&mut self.taps);
        }
    }

    /// The active profile's action for a trigger; `None` means pass through.
    /// Left click always passes through, whatever the profile says.
    fn action_for(&self, trigger: Trigger) -> Option<&Action> {
        if trigger == Trigger::Key(Key::BTN_LEFT) {
            return None;
        }
        self.profile.actions.get(&trigger)
    }
}

/// Presses `key` unless something else already holds it down. Keys of a chord
/// go out in separate frames so every app sees the modifier first.
fn press(down: &mut HashMap<Key, u32>, buffer: &mut Vec<Event>, key: Key) {
    let count = down.entry(key).or_insert(0);
    *count += 1;
    if *count == 1 {
        separate_from_previous_key(buffer);
        buffer.push(Event::key(key, 1));
    }
}

/// Releases `key` once the last thing holding it lets go.
fn release(down: &mut HashMap<Key, u32>, buffer: &mut Vec<Event>, key: Key) {
    let Some(count) = down.get_mut(&key) else {
        return;
    };
    *count -= 1;
    if *count == 0 {
        down.remove(&key);
        separate_from_previous_key(buffer);
        buffer.push(Event::key(key, 0));
    }
}

/// If the frame being built already ends in a key event, close it so the next
/// key lands in a frame of its own.
fn separate_from_previous_key(buffer: &mut Vec<Event>) {
    if buffer.last().is_some_and(|event| event.kind == EV_KEY) {
        buffer.push(Event::SYN);
    }
}

/// Ends the frame being built with a SYN_REPORT, unless it is empty.
fn end_frame(buffer: &mut Vec<Event>) {
    if buffer.last().is_some_and(|event| !event.is_syn_report()) {
        buffer.push(Event::SYN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EV_MSC;

    const REL_X: u16 = 0;
    const SYN: Event = Event::SYN;

    fn key(name: &str) -> Key {
        Key::from_name(name).unwrap()
    }

    fn down(name: &str) -> Event {
        Event::key(key(name), 1)
    }

    fn up(name: &str) -> Event {
        Event::key(key(name), 0)
    }

    fn profile(entries: &[(&str, Action)]) -> Arc<ResolvedProfile> {
        let actions = entries
            .iter()
            .map(|(trigger, action)| (trigger.parse().unwrap(), action.clone()))
            .collect();
        Arc::new(ResolvedProfile {
            name: "test".into(),
            actions,
        })
    }

    fn keys(names: &[&str]) -> Action {
        Action::Keys(names.iter().map(|name| key(name)).collect())
    }

    fn run(engine: &mut Engine, input: &[Event]) -> Vec<Event> {
        let mut out = Vec::new();
        engine.process_frame(input, &mut out);
        out
    }

    #[test]
    fn with_no_profile_everything_passes_through() {
        let mut engine = Engine::new();
        let scan = Event {
            kind: EV_MSC,
            code: 4,
            value: 0x90004,
        };
        let frame = [Event::rel(REL_X, 5), scan, down("BTN_SIDE")];
        assert_eq!(
            run(&mut engine, &frame),
            [Event::rel(REL_X, 5), scan, down("BTN_SIDE"), SYN]
        );
        assert_eq!(run(&mut engine, &[up("BTN_SIDE")]), [up("BTN_SIDE"), SYN]);
    }

    #[test]
    fn a_button_becomes_a_key_and_motion_in_the_frame_survives() {
        let mut engine = Engine::new();
        engine.set_profile(profile(&[("BTN_SIDE", keys(&["KEY_F13"]))]));
        let scan = Event {
            kind: EV_MSC,
            code: 4,
            value: 0x90004,
        };
        assert_eq!(
            run(
                &mut engine,
                &[scan, down("BTN_SIDE"), Event::rel(REL_X, -3)]
            ),
            [down("KEY_F13"), Event::rel(REL_X, -3), SYN],
            "the scan code of the original button is dropped"
        );
        assert_eq!(run(&mut engine, &[up("BTN_SIDE")]), [up("KEY_F13"), SYN]);
    }

    #[test]
    fn a_chord_presses_in_order_and_releases_in_reverse() {
        let mut engine = Engine::new();
        engine.set_profile(profile(&[("BTN_SIDE", keys(&["KEY_LEFTALT", "KEY_TAB"]))]));
        assert_eq!(
            run(&mut engine, &[down("BTN_SIDE")]),
            [down("KEY_LEFTALT"), SYN, down("KEY_TAB"), SYN]
        );
        assert_eq!(
            run(&mut engine, &[up("BTN_SIDE")]),
            [up("KEY_TAB"), SYN, up("KEY_LEFTALT"), SYN]
        );
    }

    #[test]
    fn a_disabled_button_emits_nothing() {
        let mut engine = Engine::new();
        engine.set_profile(profile(&[("BTN_SIDE", Action::Disabled)]));
        assert_eq!(run(&mut engine, &[down("BTN_SIDE")]), []);
        assert_eq!(run(&mut engine, &[up("BTN_SIDE")]), []);
    }

    /// The hard rule from CLAUDE.md.
    #[test]
    fn a_held_button_releases_what_its_press_emitted_after_a_profile_switch() {
        let browser = profile(&[("BTN_SIDE", keys(&["KEY_F13"]))]);
        let game = profile(&[("BTN_SIDE", keys(&["KEY_F14"]))]);

        // Remapped press, then switch.
        let mut engine = Engine::new();
        engine.set_profile(browser.clone());
        assert_eq!(
            run(&mut engine, &[down("BTN_SIDE")]),
            [down("KEY_F13"), SYN]
        );
        engine.set_profile(game.clone());
        assert_eq!(run(&mut engine, &[up("BTN_SIDE")]), [up("KEY_F13"), SYN]);
        // The next press uses the new profile.
        assert_eq!(
            run(&mut engine, &[down("BTN_SIDE")]),
            [down("KEY_F14"), SYN]
        );
        assert_eq!(run(&mut engine, &[up("BTN_SIDE")]), [up("KEY_F14"), SYN]);

        // Passthrough press, then switch to a profile that remaps it.
        let mut engine = Engine::new();
        assert_eq!(
            run(&mut engine, &[down("BTN_SIDE")]),
            [down("BTN_SIDE"), SYN]
        );
        engine.set_profile(game);
        assert_eq!(run(&mut engine, &[up("BTN_SIDE")]), [up("BTN_SIDE"), SYN]);

        // Remapped press, then switch to a profile that disables it.
        let mut engine = Engine::new();
        engine.set_profile(browser);
        run(&mut engine, &[down("BTN_SIDE")]);
        engine.set_profile(profile(&[("BTN_SIDE", Action::Disabled)]));
        assert_eq!(run(&mut engine, &[up("BTN_SIDE")]), [up("KEY_F13"), SYN]);
    }

    #[test]
    fn left_click_passes_through_even_if_a_profile_claims_it() {
        let mut engine = Engine::new();
        engine.set_profile(profile(&[("BTN_LEFT", Action::Disabled)]));
        assert_eq!(
            run(&mut engine, &[down("BTN_LEFT")]),
            [down("BTN_LEFT"), SYN]
        );
        assert_eq!(run(&mut engine, &[up("BTN_LEFT")]), [up("BTN_LEFT"), SYN]);
    }

    #[test]
    fn two_buttons_on_one_key_release_it_only_when_both_are_up() {
        let mut engine = Engine::new();
        engine.set_profile(profile(&[
            ("BTN_SIDE", keys(&["KEY_LEFTCTRL"])),
            ("BTN_EXTRA", keys(&["KEY_LEFTCTRL"])),
        ]));
        assert_eq!(
            run(&mut engine, &[down("BTN_SIDE")]),
            [down("KEY_LEFTCTRL"), SYN]
        );
        assert_eq!(run(&mut engine, &[down("BTN_EXTRA")]), []);
        assert_eq!(run(&mut engine, &[up("BTN_SIDE")]), []);
        assert_eq!(
            run(&mut engine, &[up("BTN_EXTRA")]),
            [up("KEY_LEFTCTRL"), SYN]
        );
    }

    #[test]
    fn a_remapped_scroll_direction_taps_and_swallows_both_wheel_events() {
        let mut engine = Engine::new();
        engine.set_profile(profile(&[("REL_WHEEL:+", keys(&["KEY_F15"]))]));

        // One tick up, as the kernel sends it: low-res and hi-res together.
        let tick_up = [Event::rel(REL_WHEEL, 1), Event::rel(REL_WHEEL_HI_RES, 120)];
        assert_eq!(
            run(&mut engine, &tick_up),
            [down("KEY_F15"), SYN, up("KEY_F15"), SYN]
        );
        // A hi-res-only frame between ticks is swallowed entirely.
        assert_eq!(run(&mut engine, &[Event::rel(REL_WHEEL_HI_RES, 15)]), []);

        // The other direction is not bound, so it passes through whole.
        let tick_down = [
            Event::rel(REL_WHEEL, -1),
            Event::rel(REL_WHEEL_HI_RES, -120),
        ];
        assert_eq!(
            run(&mut engine, &tick_down),
            [tick_down[0], tick_down[1], SYN]
        );
    }

    #[test]
    fn scroll_taps_come_after_the_rest_of_the_frame() {
        let mut engine = Engine::new();
        engine.set_profile(profile(&[("REL_WHEEL:-", keys(&["KEY_F16"]))]));
        assert_eq!(
            run(
                &mut engine,
                &[Event::rel(REL_WHEEL, -2), Event::rel(REL_X, 4)]
            ),
            [
                Event::rel(REL_X, 4),
                SYN,
                down("KEY_F16"),
                SYN,
                up("KEY_F16"),
                SYN,
                down("KEY_F16"),
                SYN,
                up("KEY_F16"),
                SYN
            ]
        );
    }

    #[test]
    fn a_disabled_scroll_direction_is_swallowed() {
        let mut engine = Engine::new();
        engine.set_profile(profile(&[("REL_WHEEL:+", Action::Disabled)]));
        let tick = [Event::rel(REL_WHEEL, 1), Event::rel(REL_WHEEL_HI_RES, 120)];
        assert_eq!(run(&mut engine, &tick), []);
    }

    #[test]
    fn release_all_lets_go_of_everything_once() {
        let mut engine = Engine::new();
        engine.set_profile(profile(&[("BTN_SIDE", keys(&["KEY_F13"]))]));
        run(&mut engine, &[down("BTN_SIDE")]);

        let mut out = Vec::new();
        engine.release_all(&mut out);
        assert_eq!(out, [up("KEY_F13"), SYN]);

        // The physical release that arrives later must not release it again.
        assert_eq!(run(&mut engine, &[up("BTN_SIDE")]), [up("BTN_SIDE"), SYN]);
        let mut out = Vec::new();
        engine.release_all(&mut out);
        assert_eq!(out, []);
    }

    #[test]
    fn output_is_appended_not_overwritten() {
        let mut engine = Engine::new();
        let mut out = vec![Event::rel(REL_X, 1), SYN];
        engine.process_frame(&[Event::rel(REL_X, 2)], &mut out);
        assert_eq!(out, [Event::rel(REL_X, 1), SYN, Event::rel(REL_X, 2), SYN]);
    }
}
