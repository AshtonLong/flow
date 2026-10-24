//! The hotkey state machine. It is fed physical key-downs and key-ups and
//! returns dictation events. Pure: no timers, no native calls, and no memory of
//! keys beyond the set currently held. Mirrors `src/main/input/matcher.ts`.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use indexmap::IndexSet;

use crate::config::HotkeysConfig;
use crate::hotkeys::{Binding, binding_satisfied, format_binding, generic_of, key_matches_token, parse_binding};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyEvent {
    HoldStart,
    /// `aborted`: a foreign key was pressed during the hold, so it was a different shortcut.
    HoldEnd { aborted: bool },
    /// Double-tap of the hold binding: start hands-free.
    Lock,
    Toggle,
    Cancel,
    PasteLast,
}

/// A clock in milliseconds.
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;
/// Asks the OS whether a key is down (`None`: cannot tell).
pub type KeyProbe = Arc<dyn Fn(&str) -> Option<bool> + Send + Sync>;

#[derive(Clone)]
pub struct MatcherOptions {
    pub bindings: HotkeysConfig,
    /// Milliseconds, injectable for tests. Defaults to a monotonic clock.
    pub now: Option<Clock>,
    /// A second press this soon after a tap's release is a double-tap. Default 350.
    pub double_tap_ms: f64,
    /// A press shorter than this counts as a tap. Default 300.
    pub tap_max_ms: f64,
    /// Asks the OS whether a key is down (`None`: cannot tell). Used by
    /// `verify_held` to notice releases the hook never delivered.
    pub is_key_down: Option<KeyProbe>,
}

impl Default for MatcherOptions {
    fn default() -> Self {
        Self {
            bindings: HotkeysConfig::default(),
            now: None,
            double_tap_ms: 350.0,
            tap_max_ms: 300.0,
            is_key_down: None,
        }
    }
}

impl MatcherOptions {
    pub fn new(bindings: HotkeysConfig) -> Self {
        Self { bindings, ..Self::default() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HoldState {
    Idle,
    Active,
    /// The hold keys are still down but this press no longer counts (locked or aborted).
    Spent,
}

fn matches_binding(key: &str, binding: &Binding) -> bool {
    binding.tokens.iter().any(|token| key_matches_token(key, token))
}

fn same_binding(a: Option<&Binding>, b: Option<&Binding>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => format_binding(a) == format_binding(b),
        (None, None) => true,
        _ => false,
    }
}

fn monotonic_ms() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

pub struct HotkeyMatcher {
    now: Clock,
    double_tap_ms: f64,
    tap_max_ms: f64,
    is_key_down: Option<KeyProbe>,

    hold: Option<Binding>,
    toggle: Option<Binding>,
    cancel: Option<Binding>,
    paste_last: Option<Binding>,
    double_tap_lock: bool,
    cancel_armed: bool,

    /// Held keys, in the order they went down.
    down: IndexSet<String>,
    /// Held keys the OS agreed were down, so a later "up" from it can be trusted.
    confirmed: IndexSet<String>,
    /// Held keys the OS reported up at the last `verify_held`.
    suspects: IndexSet<String>,

    hold_state: HoldState,
    hold_started_at: f64,
    last_tap_at: Option<f64>,
}

impl HotkeyMatcher {
    pub fn new(options: MatcherOptions) -> Self {
        let mut matcher = Self {
            now: options.now.unwrap_or_else(|| Arc::new(monotonic_ms)),
            double_tap_ms: options.double_tap_ms,
            tap_max_ms: options.tap_max_ms,
            is_key_down: options.is_key_down,
            hold: None,
            toggle: None,
            cancel: None,
            paste_last: None,
            double_tap_lock: true,
            cancel_armed: false,
            down: IndexSet::new(),
            confirmed: IndexSet::new(),
            suspects: IndexSet::new(),
            hold_state: HoldState::Idle,
            hold_started_at: 0.0,
            last_tap_at: None,
        };
        // Nothing is held yet, so this cannot produce an event.
        matcher.set_bindings(&options.bindings);
        matcher
    }

    /// An unparsable or empty binding disables that action only.
    pub fn set_bindings(&mut self, bindings: &HotkeysConfig) -> Vec<HotkeyEvent> {
        let hold = parse_binding(&bindings.hold_to_talk);
        let hold_changed = !same_binding(hold.as_ref(), self.hold.as_ref());
        self.hold = hold;
        self.toggle = parse_binding(&bindings.toggle);
        self.cancel = parse_binding(&bindings.cancel);
        self.paste_last = parse_binding(&bindings.paste_last);
        self.double_tap_lock = bindings.double_tap_lock;
        let mut events = Vec::new();
        if !hold_changed {
            return events;
        }
        // The keys being held no longer mean "hold to talk".
        let was_active = self.hold_state == HoldState::Active;
        self.hold_state = HoldState::Idle;
        self.last_tap_at = None;
        if was_active {
            events.push(HotkeyEvent::HoldEnd { aborted: true });
        }
        events
    }

    /// `cancel` only fires while armed (the session is recording or transcribing).
    pub fn set_cancel_armed(&mut self, armed: bool) {
        self.cancel_armed = armed;
    }

    /// Auto-repeat downs for a key already held are ignored.
    pub fn key_down(&mut self, key: &str) -> Vec<HotkeyEvent> {
        let mut events = Vec::new();
        if self.down.contains(key) {
            return events;
        }
        self.down.insert(key.to_string());
        if self.probe(key) == Some(true) {
            self.confirmed.insert(key.to_string());
        }

        let in_hold = self.hold.as_ref().is_some_and(|hold| matches_binding(key, hold));

        if self.hold_state == HoldState::Active {
            if in_hold {
                return events;
            }
            // A key outside the hold binding either completes a longer chord
            // (Ctrl+Win, then Space for toggle) or means the user is doing some
            // other shortcut (Ctrl+Win+Left) and is not dictating.
            if let Some(event) = self.edge_event(key, true) {
                events.push(event);
                return events;
            }
            self.hold_state = HoldState::Spent;
            self.last_tap_at = None;
            events.push(HotkeyEvent::HoldEnd { aborted: true });
            return events;
        }

        if !in_hold {
            self.last_tap_at = None;
        }

        if let Some(event) = self.edge_event(key, false) {
            events.push(event);
            return events;
        }

        // Hold starts only on the key-down that completes it, and only when
        // nothing else is down: releasing a foreign key must not start a recording.
        let exactly = self.hold.as_ref().is_some_and(|hold| self.exactly(hold));
        if in_hold && self.hold_state == HoldState::Idle && exactly {
            let t = (self.now)();
            let double_tap =
                self.double_tap_lock && self.last_tap_at.is_some_and(|last| t - last <= self.double_tap_ms);
            self.last_tap_at = None;
            if double_tap {
                self.hold_state = HoldState::Spent;
                events.push(HotkeyEvent::Lock);
            } else {
                self.hold_state = HoldState::Active;
                self.hold_started_at = t;
                events.push(HotkeyEvent::HoldStart);
            }
        }
        events
    }

    pub fn key_up(&mut self, key: &str) -> Vec<HotkeyEvent> {
        let mut events = Vec::new();
        self.confirmed.shift_remove(key);
        self.suspects.shift_remove(key);
        if !self.down.shift_remove(key) {
            return events;
        }
        if self.hold_state == HoldState::Idle {
            return events;
        }
        let Some(hold) = self.hold.as_ref() else { return events };
        // With a generic modifier the other side may still satisfy the binding.
        if binding_satisfied(hold, self.down.iter().map(String::as_str)) {
            return events;
        }
        let was_active = self.hold_state == HoldState::Active;
        self.hold_state = HoldState::Idle;
        if !was_active {
            return events;
        }
        let t = (self.now)();
        let tapped = self.double_tap_lock && t - self.hold_started_at < self.tap_max_ms;
        self.last_tap_at = if tapped { Some(t) } else { None };
        events.push(HotkeyEvent::HoldEnd { aborted: false });
        events
    }

    /// Releases held keys the OS says are up. The hook misses key-ups that go to
    /// an elevated window or the lock screen, which would otherwise leave a key
    /// "held" forever. A key is only dropped when the OS saw it down earlier and
    /// reports it up on two calls in a row, so a key-up still queued behind this
    /// call, or a key another tool hides from the OS, is never dropped by mistake.
    pub fn verify_held(&mut self) -> Vec<HotkeyEvent> {
        let mut events = Vec::new();
        if self.is_key_down.is_none() {
            return events;
        }
        let keys: Vec<String> = self.down.iter().cloned().collect();
        for key in keys {
            match self.probe(&key) {
                Some(true) => {
                    self.confirmed.insert(key.clone());
                    self.suspects.shift_remove(&key);
                }
                Some(false) if self.confirmed.contains(&key) => {
                    if self.suspects.contains(&key) {
                        events.extend(self.key_up(&key));
                    } else {
                        self.suspects.insert(key);
                    }
                }
                _ => {}
            }
        }
        events
    }

    /// Forget all held keys (after sleep/resume, or when the hook restarts). Ends an active hold as aborted.
    pub fn reset(&mut self) -> Vec<HotkeyEvent> {
        let was_active = self.hold_state == HoldState::Active;
        self.down.clear();
        self.confirmed.clear();
        self.suspects.clear();
        self.hold_state = HoldState::Idle;
        self.last_tap_at = None;
        if was_active { vec![HotkeyEvent::HoldEnd { aborted: true }] } else { Vec::new() }
    }

    /// True while any binding's keys are partially or fully held (used to delay paste until released).
    pub fn any_held(&self) -> bool {
        let bindings = [&self.hold, &self.toggle, &self.cancel, &self.paste_last];
        self.down.iter().any(|key| bindings.iter().any(|b| b.as_ref().is_some_and(|b| matches_binding(key, b))))
    }

    /// How many keys are currently down.
    pub fn held_count(&self) -> usize {
        self.down.len()
    }

    fn probe(&self, key: &str) -> Option<bool> {
        self.is_key_down.as_ref().and_then(|probe| probe(key))
    }

    /// The edge-triggered action this key-down completes, if any.
    fn edge_event(&self, key: &str, during_hold: bool) -> Option<HotkeyEvent> {
        let candidates = [
            (if self.cancel_armed { self.cancel.as_ref() } else { None }, HotkeyEvent::Cancel),
            (self.toggle.as_ref(), HotkeyEvent::Toggle),
            (self.paste_last.as_ref(), HotkeyEvent::PasteLast),
        ];
        for (binding, event) in candidates {
            let Some(binding) = binding else { continue };
            if !matches_binding(key, binding) {
                continue;
            }
            if !binding_satisfied(binding, self.down.iter().map(String::as_str)) {
                continue;
            }
            if self.modifiers_exact(binding, None) {
                return Some(event);
            }
            // The hold's own keys are not "extra" modifiers: Esc still cancels while Ctrl+Win is held.
            if during_hold && self.modifiers_exact(binding, self.hold.as_ref()) {
                return Some(event);
            }
        }
        None
    }

    /// No modifier is held beyond those the binding (or `allow`) names: Esc is not Ctrl+Shift+Esc.
    fn modifiers_exact(&self, binding: &Binding, allow: Option<&Binding>) -> bool {
        self.down.iter().all(|key| {
            generic_of(key).is_none()
                || matches_binding(key, binding)
                || allow.is_some_and(|allow| matches_binding(key, allow))
        })
    }

    /// Every token is down and no other key is.
    fn exactly(&self, binding: &Binding) -> bool {
        binding_satisfied(binding, self.down.iter().map(String::as_str))
            && self.down.iter().all(|key| matches_binding(key, binding))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    use parking_lot::Mutex;

    use super::*;

    const START: HotkeyEvent = HotkeyEvent::HoldStart;
    const END: HotkeyEvent = HotkeyEvent::HoldEnd { aborted: false };
    const ABORT: HotkeyEvent = HotkeyEvent::HoldEnd { aborted: true };
    const LOCK: HotkeyEvent = HotkeyEvent::Lock;
    const TOGGLE: HotkeyEvent = HotkeyEvent::Toggle;
    const CANCEL: HotkeyEvent = HotkeyEvent::Cancel;
    const PASTE: HotkeyEvent = HotkeyEvent::PasteLast;

    /// What the fake OS reports for each key; a missing key is "cannot tell".
    type OsKeys = Arc<Mutex<HashMap<String, Option<bool>>>>;

    fn os_keys(entries: &[(&str, bool)]) -> OsKeys {
        Arc::new(Mutex::new(entries.iter().map(|(k, v)| (k.to_string(), Some(*v))).collect()))
    }

    #[derive(Default)]
    struct Overrides {
        hold_to_talk: Option<&'static str>,
        toggle: Option<&'static str>,
        cancel: Option<&'static str>,
        paste_last: Option<&'static str>,
        double_tap_lock: Option<bool>,
    }

    fn bindings(o: &Overrides) -> HotkeysConfig {
        let d = HotkeysConfig::default();
        HotkeysConfig {
            hold_to_talk: o.hold_to_talk.map(str::to_string).unwrap_or(d.hold_to_talk),
            toggle: o.toggle.map(str::to_string).unwrap_or(d.toggle),
            cancel: o.cancel.map(str::to_string).unwrap_or(d.cancel),
            paste_last: o.paste_last.map(str::to_string).unwrap_or(d.paste_last),
            double_tap_lock: o.double_tap_lock.unwrap_or(d.double_tap_lock),
        }
    }

    struct T {
        matcher: HotkeyMatcher,
        events: Vec<HotkeyEvent>,
        /// Milliseconds as raw f64 bits.
        time: Arc<AtomicU64>,
    }

    impl T {
        /// Returns the events produced since the last call.
        fn take(&mut self) -> Vec<HotkeyEvent> {
            std::mem::take(&mut self.events)
        }
        fn advance(&self, ms: f64) {
            let t = f64::from_bits(self.time.load(Ordering::SeqCst)) + ms;
            self.time.store(t.to_bits(), Ordering::SeqCst);
        }
        fn down(&mut self, keys: &[&str]) {
            for key in keys {
                let events = self.matcher.key_down(key);
                self.events.extend(events);
            }
        }
        fn up(&mut self, keys: &[&str]) {
            for key in keys {
                let events = self.matcher.key_up(key);
                self.events.extend(events);
            }
        }
        fn set_cancel_armed(&mut self, armed: bool) {
            self.matcher.set_cancel_armed(armed);
        }
        fn set_bindings(&mut self, o: Overrides) {
            let events = self.matcher.set_bindings(&bindings(&o));
            self.events.extend(events);
        }
        fn verify_held(&mut self) {
            let events = self.matcher.verify_held();
            self.events.extend(events);
        }
        fn reset(&mut self) {
            let events = self.matcher.reset();
            self.events.extend(events);
        }
    }

    fn setup_with(o: Overrides, os: Option<OsKeys>) -> T {
        let time = Arc::new(AtomicU64::new(1000f64.to_bits()));
        let clock = time.clone();
        let is_key_down: Option<KeyProbe> = os.map(|os| {
            let probe: KeyProbe = Arc::new(move |key: &str| os.lock().get(key).copied().flatten());
            probe
        });
        let matcher = HotkeyMatcher::new(MatcherOptions {
            bindings: bindings(&o),
            now: Some(Arc::new(move || f64::from_bits(clock.load(Ordering::SeqCst)))),
            is_key_down,
            ..MatcherOptions::default()
        });
        T { matcher, events: Vec::new(), time }
    }

    fn setup(o: Overrides) -> T {
        setup_with(o, None)
    }

    fn defaults() -> T {
        setup(Overrides::default())
    }

    fn hold(binding: &'static str) -> Overrides {
        Overrides { hold_to_talk: Some(binding), ..Overrides::default() }
    }

    mod hold_to_talk {
        use super::*;

        #[test]
        fn starts_when_the_binding_is_exactly_satisfied_and_ends_on_release() {
            let mut t = defaults();
            t.down(&["LCtrl"]);
            assert_eq!(t.take(), vec![]);
            t.down(&["LWin"]);
            assert_eq!(t.take(), vec![START]);
            t.advance(2000.0);
            t.up(&["LWin"]);
            assert_eq!(t.take(), vec![END]);
            t.up(&["LCtrl"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn works_in_either_press_order_and_with_either_side_of_a_generic_modifier() {
            let mut t = defaults();
            t.down(&["RWin", "RCtrl"]);
            assert_eq!(t.take(), vec![START]);
            t.up(&["RCtrl"]);
            assert_eq!(t.take(), vec![END]);
        }

        #[test]
        fn ends_when_either_key_is_released_once() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin"]);
            t.advance(1000.0);
            t.up(&["LCtrl"]);
            assert_eq!(t.take(), vec![START, END]);
            t.up(&["LWin"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn ignores_auto_repeat() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin", "LWin", "LWin", "LCtrl"]);
            assert_eq!(t.take(), vec![START]);
        }

        #[test]
        fn does_not_start_when_another_key_is_already_down() {
            let mut t = defaults();
            t.down(&["A", "LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![]);
            t.up(&["A"]);
            // Releasing the foreign key must not start a recording either.
            assert_eq!(t.take(), vec![]);
            t.up(&["LWin", "LCtrl"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn does_not_start_with_an_extra_modifier_held() {
            let mut t = defaults();
            t.down(&["LShift", "LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn keeps_going_while_the_other_side_of_a_generic_modifier_is_pressed_and_released() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin"]);
            t.advance(1000.0);
            t.down(&["RCtrl"]);
            t.up(&["LCtrl"]);
            assert_eq!(t.take(), vec![START]);
            t.up(&["RCtrl"]);
            assert_eq!(t.take(), vec![END]);
        }

        #[test]
        fn can_restart_by_re_pressing_one_key_while_the_other_stays_down() {
            let mut t = setup(Overrides { double_tap_lock: Some(false), ..Overrides::default() });
            t.down(&["LCtrl", "LWin"]);
            t.up(&["LWin"]);
            t.down(&["LWin"]);
            t.up(&["LWin"]);
            assert_eq!(t.take(), vec![START, END, START, END]);
        }

        #[test]
        fn a_sided_binding_only_matches_that_side() {
            let mut t = setup(hold("RCtrl"));
            t.down(&["LCtrl"]);
            t.up(&["LCtrl"]);
            assert_eq!(t.take(), vec![]);
            t.down(&["RCtrl"]);
            t.advance(1000.0);
            t.up(&["RCtrl"]);
            assert_eq!(t.take(), vec![START, END]);
        }

        #[test]
        fn a_lone_modifier_hold_does_not_start_when_part_of_a_larger_chord() {
            let mut t = setup(hold("RCtrl"));
            t.down(&["LShift", "RCtrl"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn works_with_a_mouse_button() {
            let mut t = setup(hold("Mouse4"));
            t.down(&["Mouse4"]);
            t.advance(900.0);
            t.up(&["Mouse4"]);
            assert_eq!(t.take(), vec![START, END]);
        }

        #[test]
        fn works_with_a_modifier_plus_a_mouse_button() {
            let mut t = setup(hold("Ctrl+Mouse5"));
            t.down(&["Mouse5"]);
            t.up(&["Mouse5"]);
            assert_eq!(t.take(), vec![]);
            t.down(&["LCtrl", "Mouse5"]);
            t.advance(900.0);
            t.up(&["Mouse5"]);
            assert_eq!(t.take(), vec![START, END]);
        }

        #[test]
        fn works_with_a_plain_key() {
            let mut t = setup(hold("F9"));
            t.down(&["F9"]);
            t.advance(900.0);
            t.up(&["F9"]);
            assert_eq!(t.take(), vec![START, END]);
        }
    }

    mod foreign_keys_during_a_hold {
        use super::*;

        #[test]
        fn aborts_the_hold_and_emits_no_second_hold_end_on_release() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin"]);
            t.down(&["Left"]);
            assert_eq!(t.take(), vec![START, ABORT]);
            t.up(&["Left", "LWin", "LCtrl"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn does_not_restart_while_the_hold_keys_stay_down_after_an_abort() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin", "Left"]);
            t.up(&["Left"]);
            t.down(&["Left"]);
            t.up(&["Left"]);
            t.down(&["RCtrl"]);
            t.up(&["RCtrl"]);
            assert_eq!(t.take(), vec![START, ABORT]);
            // Fully releasing and pressing again starts a fresh hold.
            t.up(&["LWin", "LCtrl"]);
            t.down(&["LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![START]);
        }

        #[test]
        fn an_extra_modifier_is_a_foreign_key_too() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin", "LShift"]);
            assert_eq!(t.take(), vec![START, ABORT]);
        }

        #[test]
        fn a_mouse_button_is_a_foreign_key_too() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin", "Mouse4"]);
            assert_eq!(t.take(), vec![START, ABORT]);
        }

        #[test]
        fn a_key_that_completes_another_binding_fires_it_and_keeps_the_hold_going() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin"]);
            t.down(&["Space"]);
            assert_eq!(t.take(), vec![START, TOGGLE]);
            t.up(&["Space"]);
            assert_eq!(t.take(), vec![]);
            t.advance(1000.0);
            t.up(&["LWin"]);
            assert_eq!(t.take(), vec![END]);
        }

        #[test]
        fn a_foreign_key_after_a_completed_chord_still_aborts() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin", "Space", "A"]);
            assert_eq!(t.take(), vec![START, TOGGLE, ABORT]);
        }

        #[test]
        fn the_cancel_key_cancels_during_a_hold_even_though_the_hold_modifiers_are_down() {
            let mut t = defaults();
            t.set_cancel_armed(true);
            t.down(&["LCtrl", "LWin", "Esc"]);
            assert_eq!(t.take(), vec![START, CANCEL]);
            t.advance(1000.0);
            t.up(&["Esc", "LWin"]);
            assert_eq!(t.take(), vec![END]);
        }

        #[test]
        fn the_cancel_key_is_a_foreign_key_when_cancel_is_not_armed() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin", "Esc"]);
            assert_eq!(t.take(), vec![START, ABORT]);
        }
    }

    mod double_tap_lock {
        use super::*;

        fn tap(t: &mut T, held_ms: f64) {
            t.down(&["LCtrl", "LWin"]);
            t.advance(held_ms);
            t.up(&["LWin", "LCtrl"]);
        }

        #[test]
        fn emits_lock_instead_of_hold_start_for_the_second_press_and_no_hold_end_for_it() {
            let mut t = defaults();
            tap(&mut t, 80.0);
            assert_eq!(t.take(), vec![START, END]);
            t.advance(150.0);
            t.down(&["LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![LOCK]);
            t.advance(80.0);
            t.up(&["LWin", "LCtrl"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn works_when_only_one_key_of_the_chord_is_tapped_twice() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin"]);
            t.advance(60.0);
            t.up(&["LWin"]);
            t.advance(100.0);
            t.down(&["LWin"]);
            assert_eq!(t.take(), vec![START, END, LOCK]);
        }

        #[test]
        fn a_second_press_after_the_window_is_an_ordinary_hold() {
            let mut t = defaults();
            tap(&mut t, 80.0);
            t.advance(351.0);
            t.down(&["LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![START, END, START]);
        }

        #[test]
        fn a_second_press_exactly_at_the_window_still_locks() {
            let mut t = defaults();
            tap(&mut t, 80.0);
            t.advance(350.0);
            t.down(&["LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![START, END, LOCK]);
        }

        #[test]
        fn a_long_first_press_is_not_a_tap() {
            let mut t = defaults();
            tap(&mut t, 300.0);
            t.advance(50.0);
            t.down(&["LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![START, END, START]);
        }

        #[test]
        fn is_disabled_by_double_tap_lock_false() {
            let mut t = setup(Overrides { double_tap_lock: Some(false), ..Overrides::default() });
            tap(&mut t, 80.0);
            t.advance(100.0);
            tap(&mut t, 80.0);
            assert_eq!(t.take(), vec![START, END, START, END]);
        }

        #[test]
        fn a_key_pressed_between_the_taps_breaks_the_double_tap() {
            let mut t = defaults();
            tap(&mut t, 80.0);
            t.advance(50.0);
            t.down(&["A"]);
            t.up(&["A"]);
            t.advance(50.0);
            t.down(&["LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![START, END, START]);
        }

        #[test]
        fn an_aborted_hold_is_not_a_tap() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin", "Left"]);
            t.up(&["Left", "LWin", "LCtrl"]);
            t.advance(50.0);
            t.down(&["LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![START, ABORT, START]);
        }

        #[test]
        fn a_third_tap_after_a_lock_is_a_plain_tap_and_a_fourth_locks_again() {
            let mut t = defaults();
            tap(&mut t, 80.0);
            t.advance(100.0);
            tap(&mut t, 80.0);
            assert_eq!(t.take(), vec![START, END, LOCK]);
            t.advance(100.0);
            tap(&mut t, 80.0);
            assert_eq!(t.take(), vec![START, END]);
            t.advance(100.0);
            tap(&mut t, 80.0);
            assert_eq!(t.take(), vec![LOCK]);
        }

        #[test]
        fn honours_custom_timings() {
            let time = Arc::new(AtomicU64::new(0f64.to_bits()));
            let clock = time.clone();
            let advance = |ms: f64| {
                let t = f64::from_bits(time.load(Ordering::SeqCst)) + ms;
                time.store(t.to_bits(), Ordering::SeqCst);
            };
            let mut matcher = HotkeyMatcher::new(MatcherOptions {
                bindings: bindings(&hold("F9")),
                now: Some(Arc::new(move || f64::from_bits(clock.load(Ordering::SeqCst)))),
                double_tap_ms: 100.0,
                tap_max_ms: 50.0,
                is_key_down: None,
            });
            let mut events = Vec::new();
            events.extend(matcher.key_down("F9"));
            advance(40.0);
            events.extend(matcher.key_up("F9"));
            advance(101.0);
            events.extend(matcher.key_down("F9"));
            advance(60.0);
            events.extend(matcher.key_up("F9"));
            advance(10.0);
            events.extend(matcher.key_down("F9"));
            assert_eq!(events, vec![START, END, START, END, START]);
        }

        #[test]
        fn works_with_a_mouse_button() {
            let mut t = setup(hold("Mouse4"));
            t.down(&["Mouse4"]);
            t.advance(50.0);
            t.up(&["Mouse4"]);
            t.advance(50.0);
            t.down(&["Mouse4"]);
            assert_eq!(t.take(), vec![START, END, LOCK]);
        }
    }

    mod edge_triggered_bindings {
        use super::*;

        fn f9_and_toggle(toggle: &'static str) -> Overrides {
            Overrides { hold_to_talk: Some("F9"), toggle: Some(toggle), ..Overrides::default() }
        }

        #[test]
        fn toggle_fires_on_the_key_down_that_completes_the_chord_once() {
            let mut t = setup(f9_and_toggle("Ctrl+Shift+D"));
            t.down(&["LCtrl", "LShift"]);
            assert_eq!(t.take(), vec![]);
            t.down(&["D", "D", "D"]);
            assert_eq!(t.take(), vec![TOGGLE]);
            t.up(&["D"]);
            t.down(&["D"]);
            assert_eq!(t.take(), vec![TOGGLE]);
        }

        #[test]
        fn fires_whichever_key_completes_the_chord_last() {
            let mut t = setup(f9_and_toggle("Ctrl+Shift+D"));
            t.down(&["D", "LShift", "RCtrl"]);
            assert_eq!(t.take(), vec![TOGGLE]);
        }

        #[test]
        fn paste_last_requires_exactly_its_modifiers() {
            let mut t = defaults();
            t.down(&["LAlt", "LShift", "V"]);
            assert_eq!(t.take(), vec![PASTE]);
            t.up(&["V", "LShift", "LAlt"]);

            t.down(&["LCtrl", "LAlt", "LShift", "V"]);
            assert_eq!(t.take(), vec![]);
            t.up(&["V", "LShift", "LAlt", "LCtrl"]);

            t.down(&["LAlt", "V"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn cancel_fires_only_while_armed() {
            let mut t = defaults();
            t.down(&["Esc"]);
            t.up(&["Esc"]);
            assert_eq!(t.take(), vec![]);
            t.set_cancel_armed(true);
            t.down(&["Esc"]);
            t.up(&["Esc"]);
            assert_eq!(t.take(), vec![CANCEL]);
            t.set_cancel_armed(false);
            t.down(&["Esc"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn esc_does_not_cancel_on_ctrl_shift_esc() {
            let mut t = defaults();
            t.set_cancel_armed(true);
            t.down(&["LCtrl", "LShift", "Esc"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn a_non_modifier_key_still_down_from_typing_does_not_block_a_chord() {
            let mut t = defaults();
            t.set_cancel_armed(true);
            t.down(&["A", "Esc"]);
            assert_eq!(t.take(), vec![CANCEL]);
        }

        #[test]
        fn generic_ctrl_matches_either_side_rctrl_only_the_right_key() {
            let mut generic = setup(f9_and_toggle("Ctrl+D"));
            generic.down(&["RCtrl", "D"]);
            assert_eq!(generic.take(), vec![TOGGLE]);

            let mut sided = setup(f9_and_toggle("RCtrl+D"));
            sided.down(&["LCtrl", "D"]);
            assert_eq!(sided.take(), vec![]);
            sided.up(&["D", "LCtrl"]);
            sided.down(&["RCtrl", "D"]);
            assert_eq!(sided.take(), vec![TOGGLE]);
        }

        #[test]
        fn lone_modifier_and_mouse_bindings_work_for_every_action() {
            let mut t = setup(Overrides {
                hold_to_talk: Some("F9"),
                toggle: Some("RCtrl"),
                cancel: Some("Mouse3"),
                paste_last: Some("RAlt"),
                ..Overrides::default()
            });
            t.set_cancel_armed(true);
            t.down(&["RCtrl"]);
            t.up(&["RCtrl"]);
            t.down(&["Mouse3"]);
            t.up(&["Mouse3"]);
            t.down(&["RAlt"]);
            t.up(&["RAlt"]);
            assert_eq!(t.take(), vec![TOGGLE, CANCEL, PASTE]);
            // The other side, or an extra modifier, does not count.
            t.down(&["LCtrl"]);
            t.up(&["LCtrl"]);
            t.down(&["LShift", "RCtrl"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn a_modifier_plus_mouse_chord_needs_the_modifier() {
            let mut t = setup(Overrides { toggle: Some("Ctrl+Mouse4"), ..Overrides::default() });
            t.down(&["Mouse4"]);
            t.up(&["Mouse4"]);
            assert_eq!(t.take(), vec![]);
            t.down(&["RCtrl", "Mouse4"]);
            assert_eq!(t.take(), vec![TOGGLE]);
        }

        #[test]
        fn toggle_through_the_default_chord_passes_through_hold_start_first() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin", "Space"]);
            t.advance(120.0);
            t.up(&["Space", "LWin", "LCtrl"]);
            assert_eq!(t.take(), vec![START, TOGGLE, END]);
        }

        #[test]
        fn toggle_fires_even_when_the_shorter_hold_chord_was_aborted() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin", "Left"]);
            t.up(&["Left"]);
            t.down(&["Space"]);
            assert_eq!(t.take(), vec![START, ABORT, TOGGLE]);
        }
    }

    mod bindings {
        use super::*;

        #[test]
        fn an_unparsable_or_empty_binding_disables_only_that_action() {
            let mut t = setup(Overrides {
                hold_to_talk: Some("Ctrl+Nope"),
                toggle: Some(""),
                paste_last: Some("Alt+Shift+V"),
                ..Overrides::default()
            });
            t.down(&["LCtrl", "LWin"]);
            t.up(&["LWin", "LCtrl"]);
            t.down(&["LCtrl", "LWin", "Space"]);
            t.up(&["Space", "LWin", "LCtrl"]);
            assert_eq!(t.take(), vec![]);
            t.down(&["LAlt", "LShift", "V"]);
            assert_eq!(t.take(), vec![PASTE]);
        }

        #[test]
        fn a_disabled_hold_leaves_the_other_actions_working() {
            let mut t = setup(hold(""));
            t.down(&["LCtrl", "LWin", "Space"]);
            assert_eq!(t.take(), vec![TOGGLE]);
        }

        #[test]
        fn set_bindings_takes_effect_immediately() {
            let mut t = defaults();
            t.set_bindings(Overrides { hold_to_talk: Some("F9"), toggle: Some("F10"), ..Overrides::default() });
            t.down(&["LCtrl", "LWin"]);
            t.up(&["LWin", "LCtrl"]);
            assert_eq!(t.take(), vec![]);
            t.down(&["F9"]);
            t.advance(500.0);
            t.up(&["F9"]);
            t.down(&["F10"]);
            assert_eq!(t.take(), vec![START, END, TOGGLE]);
        }

        #[test]
        fn changing_the_hold_binding_during_a_hold_ends_it_as_aborted() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin"]);
            t.set_bindings(hold("F9"));
            assert_eq!(t.take(), vec![START, ABORT]);
            t.up(&["LWin", "LCtrl"]);
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn changing_another_binding_during_a_hold_leaves_the_hold_alone() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin"]);
            // Same hold binding, written differently.
            t.set_bindings(Overrides { hold_to_talk: Some("win + ctrl"), toggle: Some("F10"), ..Overrides::default() });
            t.advance(1000.0);
            t.up(&["LWin"]);
            assert_eq!(t.take(), vec![START, END]);
        }

        #[test]
        fn accepts_aliases_and_any_token_order() {
            let mut t = setup(Overrides {
                hold_to_talk: Some("super+control"),
                paste_last: Some("v+shift+option"),
                ..Overrides::default()
            });
            t.down(&["LCtrl", "LWin"]);
            t.advance(1000.0);
            t.up(&["LWin", "LCtrl"]);
            t.down(&["RAlt", "RShift", "V"]);
            assert_eq!(t.take(), vec![START, END, PASTE]);
        }
    }

    mod reset_and_any_held {
        use super::*;

        #[test]
        fn reset_forgets_held_keys_and_ends_an_active_hold_as_aborted() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin"]);
            t.reset();
            assert_eq!(t.take(), vec![START, ABORT]);
            assert_eq!(t.matcher.held_count(), 0);
            // The stale key-ups that follow are harmless.
            t.up(&["LWin", "LCtrl"]);
            assert_eq!(t.take(), vec![]);
            t.down(&["LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![START]);
        }

        #[test]
        fn reset_with_nothing_active_emits_nothing_and_clears_a_pending_double_tap() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin"]);
            t.advance(50.0);
            t.up(&["LWin", "LCtrl"]);
            t.take();
            t.reset();
            t.advance(50.0);
            t.down(&["LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![START]);
        }

        #[test]
        fn a_key_up_for_a_key_that_was_never_down_is_ignored() {
            let mut t = defaults();
            t.up(&["LWin", "A"]);
            assert_eq!(t.take(), vec![]);
            assert_eq!(t.matcher.held_count(), 0);
        }

        #[test]
        fn any_held_is_true_while_any_binding_key_is_down() {
            let mut t = defaults();
            assert!(!t.matcher.any_held());
            t.down(&["A"]);
            assert!(!t.matcher.any_held());
            t.down(&["LShift"]);
            // Shift is part of Alt+Shift+V.
            assert!(t.matcher.any_held());
            t.up(&["LShift", "A"]);
            t.down(&["LCtrl", "LWin"]);
            assert!(t.matcher.any_held());
            t.up(&["LWin"]);
            assert!(t.matcher.any_held());
            t.up(&["LCtrl"]);
            assert!(!t.matcher.any_held());
        }

        #[test]
        fn any_held_ignores_keys_of_disabled_bindings() {
            let mut t = setup(Overrides {
                hold_to_talk: Some("F9"),
                toggle: Some(""),
                cancel: Some(""),
                paste_last: Some("not a key"),
                ..Overrides::default()
            });
            t.down(&["LCtrl", "LShift", "Esc"]);
            assert!(!t.matcher.any_held());
            t.down(&["F9"]);
            assert!(t.matcher.any_held());
        }

        #[test]
        fn keeps_nothing_but_the_keys_currently_held() {
            let mut t = defaults();
            for key in ["A", "B", "LShift", "Enter", "Mouse4"] {
                t.down(&[key]);
                t.up(&[key]);
            }
            assert_eq!(t.matcher.held_count(), 0);
            t.down(&["A", "B"]);
            assert_eq!(t.matcher.held_count(), 2);
        }
    }

    mod verify_held {
        use super::*;

        fn set(os: &OsKeys, key: &str, down: bool) {
            os.lock().insert(key.to_string(), Some(down));
        }

        #[test]
        fn ends_a_hold_whose_release_was_missed_after_two_checks() {
            let os = os_keys(&[("LCtrl", true), ("LWin", true)]);
            let mut t = setup_with(Overrides::default(), Some(os.clone()));
            t.down(&["LCtrl", "LWin"]);
            t.advance(1000.0);
            t.verify_held();
            assert_eq!(t.take(), vec![START]);
            // The user let go while an elevated window had focus: no key-up arrives.
            set(&os, "LWin", false);
            set(&os, "LCtrl", false);
            t.verify_held();
            assert_eq!(t.take(), vec![]);
            t.verify_held();
            assert_eq!(t.take(), vec![END]);
            assert_eq!(t.matcher.held_count(), 0);
        }

        #[test]
        fn clears_a_stuck_foreign_key_so_the_hold_works_again() {
            let os = os_keys(&[("Esc", true)]);
            let mut t = setup_with(Overrides::default(), Some(os.clone()));
            t.down(&["Esc"]);
            set(&os, "Esc", false);
            t.verify_held();
            t.verify_held();
            set(&os, "LCtrl", true);
            set(&os, "LWin", true);
            t.down(&["LCtrl", "LWin"]);
            assert_eq!(t.take(), vec![START]);
        }

        #[test]
        fn a_key_that_reads_down_again_between_checks_is_kept() {
            let os = os_keys(&[("F9", true)]);
            let mut t = setup_with(hold("F9"), Some(os.clone()));
            t.down(&["F9"]);
            set(&os, "F9", false);
            t.verify_held();
            set(&os, "F9", true);
            t.verify_held();
            set(&os, "F9", false);
            t.verify_held();
            assert_eq!(t.take(), vec![START]);
            assert_eq!(t.matcher.held_count(), 1);
        }

        #[test]
        fn never_drops_a_key_the_os_did_not_see_go_down_hidden_by_another_tool() {
            let os = os_keys(&[("CapsLock", false)]);
            let mut t = setup_with(hold("CapsLock"), Some(os));
            t.down(&["CapsLock"]);
            t.verify_held();
            t.verify_held();
            t.verify_held();
            assert_eq!(t.take(), vec![START]);
            t.advance(1000.0);
            t.up(&["CapsLock"]);
            assert_eq!(t.take(), vec![END]);
        }

        #[test]
        fn never_drops_a_key_the_os_cannot_report_on() {
            let os = os_keys(&[]);
            let mut t = setup_with(hold("F9"), Some(os));
            t.down(&["F9"]);
            t.verify_held();
            t.verify_held();
            assert_eq!(t.matcher.held_count(), 1);
        }

        #[test]
        fn does_nothing_without_an_is_key_down_probe() {
            let mut t = defaults();
            t.down(&["LCtrl", "LWin"]);
            t.verify_held();
            t.verify_held();
            assert_eq!(t.take(), vec![START]);
        }

        #[test]
        fn a_real_key_up_between_checks_is_not_double_counted() {
            let os = os_keys(&[("F9", true)]);
            let mut t = setup_with(hold("F9"), Some(os.clone()));
            t.down(&["F9"]);
            t.advance(1000.0);
            set(&os, "F9", false);
            t.verify_held();
            t.up(&["F9"]);
            t.verify_held();
            assert_eq!(t.take(), vec![START, END]);
        }
    }
}
