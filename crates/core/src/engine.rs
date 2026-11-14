//! The remapping state machine.
//!
//! Feed it one frame of events from the mouse (everything up to, but not
//! including, a SYN_REPORT) and it appends what the virtual device should
//! emit, SYN_REPORTs included. It owns no devices and does no I/O.
//!
//! The one piece of state that matters is `held`: for every button currently
//! down, what its press emitted. A release always undoes exactly that, so a
//! profile switch while a button is held can never leave a key stuck.
//!
//! Macros need time, and the engine has no clock of its own: the caller
//! passes the current time in (any steadily increasing `Duration`), and
//! asks [`Engine::next_wake`] when to call [`Engine::advance`] next. That
//! keeps playback exactly testable.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::event::{
    EV_KEY, EV_MSC, EV_REL, Event, Key, REL_HWHEEL, REL_HWHEEL_HI_RES, REL_WHEEL, REL_WHEEL_HI_RES,
    ScrollAxis, Trigger,
};
use crate::macros::Step;
use crate::profile::{Action, MacroMode, ResolvedProfile};

/// A single scroll event never taps a key more than this many times, however
/// large its value.
const MAX_TAPS_PER_SCROLL_EVENT: u32 = 8;

/// A repeating macro never starts a pass sooner than this after the last
/// one started, so a macro with no delays cannot flood the system.
const MIN_REPEAT_PERIOD: Duration = Duration::from_millis(10);

/// How far behind schedule a repeating macro may fall before it gives up
/// catching up and restarts its rhythm from the present.
const LATE_LIMIT: Duration = Duration::from_millis(100);

/// A macro being played.
#[derive(Debug)]
struct Running {
    /// The button or scroll direction that started it.
    trigger: Trigger,
    steps: Arc<[Step]>,
    mode: MacroMode,
    /// Index of the next step to perform.
    position: usize,
    /// When that step is due.
    due: Duration,
    pass_started: Duration,
    /// Keys this macro has pressed and not yet released.
    held: Vec<Key>,
    /// For repeat-while-held: whether the button is still down.
    button_held: bool,
    /// Finish the current pass, then stop.
    stopping: bool,
}

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
    running: Vec<Running>,
}

impl Engine {
    /// An engine with an empty profile: everything passes through.
    pub fn new() -> Engine {
        Engine::default()
    }

    /// Switches the active profile. Buttons held right now keep what they
    /// emitted and release it normally; only later presses use the new one.
    ///
    /// Macros in progress are cut short and their keys released: a macro
    /// meant for one app must not keep typing into the next. Setting the
    /// profile that is already active changes nothing.
    pub fn set_profile(&mut self, profile: Arc<ResolvedProfile>, out: &mut Vec<Event>) {
        if Arc::ptr_eq(&self.profile, &profile) {
            return;
        }
        self.profile = profile;
        for stopped in self.running.drain(..) {
            for &key in stopped.held.iter().rev() {
                release_alone(&mut self.down, out, key);
            }
        }
    }

    pub fn profile(&self) -> &ResolvedProfile {
        &self.profile
    }

    /// Handles one frame from the mouse, appending the output to `out`.
    /// `now` is the current time, used to start and run macros.
    pub fn process_frame(&mut self, input: &[Event], now: Duration, out: &mut Vec<Event>) {
        self.frame.clear();
        self.taps.clear();
        // The scan code that accompanies a button press names the original
        // button, so it is dropped from any frame where a button was changed.
        let mut changed_a_key = false;

        for &event in input {
            match event.kind {
                EV_KEY => changed_a_key |= self.handle_key(Key(event.code), event.value, now),
                EV_REL => self.handle_rel(event, now),
                _ => self.frame.push(event),
            }
        }

        if changed_a_key {
            self.frame.retain(|event| event.kind != EV_MSC);
        }
        end_frame(&mut self.frame);
        out.append(&mut self.frame);
        out.append(&mut self.taps);
        // A macro started by this frame plays its first steps right away.
        self.advance(now, out);
    }

    /// Performs every macro step that is due at `now`.
    pub fn advance(&mut self, now: Duration, out: &mut Vec<Event>) {
        let mut index = 0;
        while index < self.running.len() {
            if run_macro(&mut self.running[index], &mut self.down, now, out) {
                self.running.swap_remove(index);
            } else {
                index += 1;
            }
        }
    }

    /// When [`Engine::advance`] next has something to do, or `None` if no
    /// macro is playing.
    pub fn next_wake(&self) -> Option<Duration> {
        self.running.iter().map(|running| running.due).min()
    }

    /// Starts a macro for `trigger`, or, if one is already playing for it,
    /// applies what a new press means in that macro's mode.
    fn press_macro(
        &mut self,
        trigger: Trigger,
        steps: Arc<[Step]>,
        mode: MacroMode,
        button_held: bool,
        now: Duration,
    ) {
        match self.running.iter_mut().find(|r| r.trigger == trigger) {
            Some(running) => match running.mode {
                // Already playing; let it finish.
                MacroMode::Once => {}
                MacroMode::RepeatWhileHeld => running.button_held = button_held,
                // A press while on turns it off, and the other way round.
                MacroMode::Toggle => running.stopping = !running.stopping,
            },
            None => self.running.push(Running {
                trigger,
                steps,
                mode,
                position: 0,
                due: now,
                pass_started: now,
                held: Vec::new(),
                button_held,
                stopping: false,
            }),
        }
    }

    /// Releases every key the engine is holding down. Used when remapping is
    /// paused and when the mouse disappears (it can no longer send releases).
    pub fn release_all(&mut self, out: &mut Vec<Event>) {
        self.held.clear();
        // Macros stop too; their keys are in `down` and released below.
        self.running.clear();
        for (key, _) in self.down.drain() {
            out.push(Event::key(key, 0));
            out.push(Event::SYN);
        }
    }

    /// Returns true if the output differs from the input event.
    fn handle_key(&mut self, button: Key, value: i32, now: Duration) -> bool {
        match value {
            1 => {
                if self.held.contains_key(&button) {
                    // A second press with no release in between; ignore it.
                    return true;
                }
                let trigger = Trigger::Key(button);
                let emitted = match self.action_for(trigger).cloned() {
                    None => vec![button],
                    Some(Action::Keys(keys)) => keys,
                    Some(Action::Disabled) => Vec::new(),
                    Some(Action::Macro { steps, mode }) => {
                        self.press_macro(trigger, steps, mode, true, now);
                        Vec::new()
                    }
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
                    // A repeat-while-held macro finishes its pass and stops.
                    let trigger = Trigger::Key(button);
                    if let Some(running) = self.running.iter_mut().find(|r| r.trigger == trigger) {
                        running.button_held = false;
                    }
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

    fn handle_rel(&mut self, event: Event, now: Duration) {
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
        let action = match self.action_for(trigger).cloned() {
            None => return self.frame.push(event),
            Some(Action::Disabled) => return,
            Some(action) => action,
        };
        // The kernel reports each scroll twice: whole ticks and a
        // high-resolution stream. Both are swallowed; only whole ticks act.
        if hi_res {
            return;
        }
        let keys = match action {
            Action::Keys(keys) => keys,
            Action::Macro { steps, mode } => {
                // A scroll tick has no "held", so repeat-while-held plays
                // one pass per tick.
                return self.press_macro(trigger, steps, mode, false, now);
            }
            Action::Disabled => return,
        };
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

/// Presses `key` in a frame of its own (if it is not already down).
fn press_alone(down: &mut HashMap<Key, u32>, out: &mut Vec<Event>, key: Key) {
    let before = out.len();
    press(down, out, key);
    if out.len() > before {
        end_frame(out);
    }
}

/// Releases `key` in a frame of its own (if nothing else holds it).
fn release_alone(down: &mut HashMap<Key, u32>, out: &mut Vec<Event>, key: Key) {
    let before = out.len();
    release(down, out, key);
    if out.len() > before {
        end_frame(out);
    }
}

/// Performs the steps of one macro that are due. Returns true when the
/// macro has finished and should be dropped.
fn run_macro(
    running: &mut Running,
    down: &mut HashMap<Key, u32>,
    now: Duration,
    out: &mut Vec<Event>,
) -> bool {
    while running.due <= now {
        let Some(step) = running.steps.get(running.position) else {
            // End of a pass. A macro that forgot to release a key must not
            // leave it stuck.
            for key in std::mem::take(&mut running.held).into_iter().rev() {
                release_alone(down, out, key);
            }
            let again = !running.stopping
                && !running.steps.is_empty()
                && match running.mode {
                    MacroMode::Once => false,
                    MacroMode::RepeatWhileHeld => running.button_held,
                    MacroMode::Toggle => true,
                };
            if !again {
                return true;
            }
            running.position = 0;
            running.due = running.due.max(running.pass_started + MIN_REPEAT_PERIOD);
            // If we are far behind (the process was stalled), start the
            // next pass now rather than replaying the missed passes in a
            // burst. Small lateness is left alone so the rhythm stays exact.
            if now.saturating_sub(running.due) > LATE_LIMIT {
                running.due = now;
            }
            running.pass_started = running.due;
            continue;
        };
        match *step {
            Step::KeyDown { key } => {
                if !running.held.contains(&key) {
                    running.held.push(key);
                    press_alone(down, out, key);
                }
            }
            Step::KeyUp { key } => {
                // Only release what this macro itself pressed.
                if let Some(index) = running.held.iter().position(|held| *held == key) {
                    running.held.remove(index);
                    release_alone(down, out, key);
                }
            }
            Step::Delay { ms } => running.due += Duration::from_millis(u64::from(ms)),
        }
        running.position += 1;
    }
    false
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
        run_at(engine, 0, input)
    }

    /// Feeds a frame in at a given time in milliseconds.
    fn run_at(engine: &mut Engine, ms: u64, input: &[Event]) -> Vec<Event> {
        let mut out = Vec::new();
        engine.process_frame(input, Duration::from_millis(ms), &mut out);
        out
    }

    /// Lets time pass up to `ms` milliseconds and returns what was played.
    fn advance_to(engine: &mut Engine, ms: u64) -> Vec<Event> {
        let mut out = Vec::new();
        engine.advance(Duration::from_millis(ms), &mut out);
        out
    }

    fn switch(engine: &mut Engine, profile: Arc<ResolvedProfile>) {
        let mut out = Vec::new();
        engine.set_profile(profile, &mut out);
        assert_eq!(out, [], "no macro was running, so nothing is released");
    }

    /// A macro action: press and release `key` with `hold` ms in between,
    /// then wait `gap` ms.
    fn tap_macro(key_name: &str, hold: u32, gap: u32, mode: MacroMode) -> Action {
        let key = key(key_name);
        Action::Macro {
            steps: vec![
                Step::KeyDown { key },
                Step::Delay { ms: hold },
                Step::KeyUp { key },
                Step::Delay { ms: gap },
            ]
            .into(),
            mode,
        }
    }

    fn wake_ms(engine: &Engine) -> Option<u128> {
        engine.next_wake().map(|at| at.as_millis())
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
        switch(&mut engine, profile(&[("BTN_SIDE", keys(&["KEY_F13"]))]));
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
        switch(
            &mut engine,
            profile(&[("BTN_SIDE", keys(&["KEY_LEFTALT", "KEY_TAB"]))]),
        );
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
        switch(&mut engine, profile(&[("BTN_SIDE", Action::Disabled)]));
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
        switch(&mut engine, browser.clone());
        assert_eq!(
            run(&mut engine, &[down("BTN_SIDE")]),
            [down("KEY_F13"), SYN]
        );
        switch(&mut engine, game.clone());
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
        switch(&mut engine, game);
        assert_eq!(run(&mut engine, &[up("BTN_SIDE")]), [up("BTN_SIDE"), SYN]);

        // Remapped press, then switch to a profile that disables it.
        let mut engine = Engine::new();
        switch(&mut engine, browser);
        run(&mut engine, &[down("BTN_SIDE")]);
        switch(&mut engine, profile(&[("BTN_SIDE", Action::Disabled)]));
        assert_eq!(run(&mut engine, &[up("BTN_SIDE")]), [up("KEY_F13"), SYN]);
    }

    #[test]
    fn left_click_passes_through_even_if_a_profile_claims_it() {
        let mut engine = Engine::new();
        switch(&mut engine, profile(&[("BTN_LEFT", Action::Disabled)]));
        assert_eq!(
            run(&mut engine, &[down("BTN_LEFT")]),
            [down("BTN_LEFT"), SYN]
        );
        assert_eq!(run(&mut engine, &[up("BTN_LEFT")]), [up("BTN_LEFT"), SYN]);
    }

    #[test]
    fn two_buttons_on_one_key_release_it_only_when_both_are_up() {
        let mut engine = Engine::new();
        switch(
            &mut engine,
            profile(&[
                ("BTN_SIDE", keys(&["KEY_LEFTCTRL"])),
                ("BTN_EXTRA", keys(&["KEY_LEFTCTRL"])),
            ]),
        );
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
        switch(&mut engine, profile(&[("REL_WHEEL:+", keys(&["KEY_F15"]))]));

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
        switch(&mut engine, profile(&[("REL_WHEEL:-", keys(&["KEY_F16"]))]));
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
        switch(&mut engine, profile(&[("REL_WHEEL:+", Action::Disabled)]));
        let tick = [Event::rel(REL_WHEEL, 1), Event::rel(REL_WHEEL_HI_RES, 120)];
        assert_eq!(run(&mut engine, &tick), []);
    }

    #[test]
    fn release_all_lets_go_of_everything_once() {
        let mut engine = Engine::new();
        switch(&mut engine, profile(&[("BTN_SIDE", keys(&["KEY_F13"]))]));
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
        engine.process_frame(&[Event::rel(REL_X, 2)], Duration::ZERO, &mut out);
        assert_eq!(out, [Event::rel(REL_X, 1), SYN, Event::rel(REL_X, 2), SYN]);
    }

    #[test]
    fn a_once_macro_plays_through_with_its_timing_even_after_release() {
        let mut engine = Engine::new();
        switch(
            &mut engine,
            profile(&[("BTN_SIDE", tap_macro("KEY_B", 40, 0, MacroMode::Once))]),
        );
        // The first step plays in the same instant as the press.
        assert_eq!(
            run_at(&mut engine, 1000, &[down("BTN_SIDE")]),
            [down("KEY_B"), SYN]
        );
        assert_eq!(wake_ms(&engine), Some(1040));
        // Releasing the button early changes nothing.
        assert_eq!(run_at(&mut engine, 1010, &[up("BTN_SIDE")]), []);
        assert_eq!(advance_to(&mut engine, 1039), []);
        assert_eq!(advance_to(&mut engine, 1040), [up("KEY_B"), SYN]);
        assert_eq!(wake_ms(&engine), None, "finished");

        // Pressing again while it plays does not start a second copy.
        run_at(&mut engine, 2000, &[down("BTN_SIDE")]);
        run_at(&mut engine, 2005, &[up("BTN_SIDE")]);
        assert_eq!(run_at(&mut engine, 2010, &[down("BTN_SIDE")]), []);
        assert_eq!(advance_to(&mut engine, 2040), [up("KEY_B"), SYN]);
        assert_eq!(wake_ms(&engine), None);
    }

    #[test]
    fn a_repeat_macro_loops_while_held_and_finishes_its_pass_on_release() {
        let mut engine = Engine::new();
        switch(
            &mut engine,
            profile(&[(
                "BTN_SIDE",
                tap_macro("KEY_B", 20, 30, MacroMode::RepeatWhileHeld),
            )]),
        );
        assert_eq!(
            run_at(&mut engine, 0, &[down("BTN_SIDE")]),
            [down("KEY_B"), SYN]
        );
        assert_eq!(advance_to(&mut engine, 20), [up("KEY_B"), SYN]);
        // One pass is 50 ms; the second starts exactly then.
        assert_eq!(advance_to(&mut engine, 49), []);
        assert_eq!(advance_to(&mut engine, 50), [down("KEY_B"), SYN]);
        // Released mid-pass: the key still comes up on schedule, then stop.
        assert_eq!(run_at(&mut engine, 60, &[up("BTN_SIDE")]), []);
        assert_eq!(advance_to(&mut engine, 70), [up("KEY_B"), SYN]);
        assert_eq!(advance_to(&mut engine, 500), []);
        assert_eq!(wake_ms(&engine), None);
    }

    #[test]
    fn a_toggle_macro_runs_until_the_next_press() {
        let mut engine = Engine::new();
        switch(
            &mut engine,
            profile(&[("BTN_SIDE", tap_macro("KEY_B", 10, 10, MacroMode::Toggle))]),
        );
        assert_eq!(
            run_at(&mut engine, 0, &[down("BTN_SIDE")]),
            [down("KEY_B"), SYN]
        );
        assert_eq!(run_at(&mut engine, 5, &[up("BTN_SIDE")]), []);
        // Keeps going with the button up: three more edges by 30 ms.
        assert_eq!(
            advance_to(&mut engine, 30),
            [up("KEY_B"), SYN, down("KEY_B"), SYN, up("KEY_B"), SYN]
        );
        // The second press turns it off after the pass in progress.
        assert_eq!(run_at(&mut engine, 35, &[down("BTN_SIDE")]), []);
        assert_eq!(advance_to(&mut engine, 1000), []);
        assert_eq!(wake_ms(&engine), None);
    }

    #[test]
    fn a_repeating_macro_with_no_delays_is_rate_limited() {
        let b = key("KEY_B");
        let mut engine = Engine::new();
        switch(
            &mut engine,
            profile(&[(
                "BTN_SIDE",
                Action::Macro {
                    steps: vec![Step::KeyDown { key: b }, Step::KeyUp { key: b }].into(),
                    mode: MacroMode::Toggle,
                },
            )]),
        );
        let first = run_at(&mut engine, 0, &[down("BTN_SIDE")]);
        assert_eq!(first, [down("KEY_B"), SYN, up("KEY_B"), SYN]);
        assert_eq!(wake_ms(&engine), Some(10));
        // A long stall does not produce a burst of missed passes.
        let after_stall = advance_to(&mut engine, 5000);
        assert!(after_stall.len() <= 8, "{} events", after_stall.len());
    }

    #[test]
    fn a_macro_that_forgets_a_release_does_not_leave_the_key_stuck() {
        let mut engine = Engine::new();
        switch(
            &mut engine,
            profile(&[(
                "BTN_SIDE",
                Action::Macro {
                    steps: vec![Step::KeyDown { key: key("KEY_B") }, Step::Delay { ms: 5 }].into(),
                    mode: MacroMode::Once,
                },
            )]),
        );
        assert_eq!(
            run_at(&mut engine, 0, &[down("BTN_SIDE")]),
            [down("KEY_B"), SYN]
        );
        assert_eq!(advance_to(&mut engine, 5), [up("KEY_B"), SYN]);
    }

    #[test]
    fn a_profile_switch_or_release_all_cuts_macros_short() {
        let toggling = profile(&[("BTN_SIDE", tap_macro("KEY_B", 50, 50, MacroMode::Toggle))]);

        let mut engine = Engine::new();
        switch(&mut engine, toggling.clone());
        run_at(&mut engine, 0, &[down("BTN_SIDE")]);
        let mut out = Vec::new();
        engine.set_profile(profile(&[]), &mut out);
        assert_eq!(out, [up("KEY_B"), SYN], "the held macro key is released");
        assert_eq!(advance_to(&mut engine, 1000), []);
        assert_eq!(wake_ms(&engine), None);

        let mut engine = Engine::new();
        switch(&mut engine, toggling.clone());
        run_at(&mut engine, 0, &[down("BTN_SIDE")]);
        // Re-applying the very same profile (as every focus change does)
        // must not interrupt it.
        switch(&mut engine, toggling);
        assert_eq!(wake_ms(&engine), Some(50));
        let mut out = Vec::new();
        engine.release_all(&mut out);
        assert_eq!(out, [up("KEY_B"), SYN]);
        assert_eq!(wake_ms(&engine), None);
    }

    #[test]
    fn a_scroll_tick_can_start_a_macro() {
        let mut engine = Engine::new();
        switch(
            &mut engine,
            profile(&[(
                "REL_WHEEL:+",
                tap_macro("KEY_B", 5, 0, MacroMode::RepeatWhileHeld),
            )]),
        );
        let tick = [Event::rel(REL_WHEEL, 1), Event::rel(REL_WHEEL_HI_RES, 120)];
        assert_eq!(run_at(&mut engine, 0, &tick), [down("KEY_B"), SYN]);
        assert_eq!(advance_to(&mut engine, 5), [up("KEY_B"), SYN]);
        assert_eq!(wake_ms(&engine), None, "one pass per tick");
    }
}
