//! Chord-system state machine for the transcription coordinator.
//!
//! This is a pure state machine — no Tauri / clock / threading dependencies.
//! The owning coordinator drives it by calling `on_*` methods in response to
//! real events (key presses, timer expiries, etc.) and performing any returned
//! [`Effect`]s.
//!
//! Designed for unit testing: tests construct a [`ChordStateMachine`], feed
//! synthetic event sequences, and assert on stage transitions + emitted
//! effects without touching Tauri or real time.
//!
//! See `.planning/chord-system.md` (Phase 2) for the broader design.

/// Pipeline lifecycle stage. Owned by the state machine; the coordinator
/// only inspects it via [`ChordStateMachine::stage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Idle,
    /// User has tapped at least once; we're waiting on more taps or expiry.
    ChordWindow {
        binding_id: String,
        count: u32,
        last_press_held: bool,
    },
    /// Recording is in progress; preset (if any) was resolved at expiry time.
    Recording {
        binding_id: String,
        preset: Option<String>,
    },
    /// Transcription pipeline is running. New input events are ignored until
    /// [`ChordStateMachine::on_processing_finished`] is called.
    Processing,
}

/// Side effects the coordinator must perform after a state transition.
///
/// The state machine itself is pure — it never touches Tauri, threads, or
/// the clock. Effects are the bridge between pure logic and the real world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    StartRecording {
        binding_id: String,
        preset: Option<String>,
    },
    /// `preset` echoes whatever was active in the [`Stage::Recording`] this
    /// stop is exiting, so the caller can route the recording to plain or
    /// LLM-post-processed transcription without a separate lookup.
    StopRecording {
        binding_id: String,
        preset: Option<String>,
    },
    /// Caller should (re-)schedule the chord-window expiry timer. Cancels
    /// any previously scheduled expiry; only the most recent one matters.
    ScheduleChordExpiry,
}

pub struct ChordStateMachine {
    stage: Stage,
}

impl ChordStateMachine {
    pub fn new() -> Self {
        Self { stage: Stage::Idle }
    }

    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// Handle a key-down event for `binding_id`.
    ///
    /// - From `Idle`: enters the chord window with `count=1, held=true`.
    /// - From `ChordWindow` (same binding): increments count, marks held,
    ///   re-arms the expiry timer.
    /// - From `Recording` (toggle, same binding): stops recording.
    /// - From `Recording` (PTT) or `Processing`: ignored.
    pub fn on_press(&mut self, binding_id: &str, push_to_talk: bool) -> Vec<Effect> {
        match &mut self.stage {
            Stage::Idle => {
                self.stage = Stage::ChordWindow {
                    binding_id: binding_id.to_string(),
                    count: 1,
                    last_press_held: true,
                };
                vec![Effect::ScheduleChordExpiry]
            }
            Stage::ChordWindow {
                binding_id: bid,
                count,
                last_press_held,
            } if bid == binding_id => {
                *count += 1;
                *last_press_held = true;
                vec![Effect::ScheduleChordExpiry]
            }
            Stage::Recording {
                binding_id: bid,
                preset,
            } if !push_to_talk && bid == binding_id => {
                let bid = bid.clone();
                let preset = preset.clone();
                self.stage = Stage::Processing;
                vec![Effect::StopRecording {
                    binding_id: bid,
                    preset,
                }]
            }
            _ => vec![],
        }
    }

    /// Handle a key-up event.
    ///
    /// - In `ChordWindow` (same binding): clears `last_press_held`.
    /// - In `Recording` (PTT, same binding): stops recording.
    /// - All other states: ignored.
    pub fn on_release(&mut self, binding_id: &str, push_to_talk: bool) -> Vec<Effect> {
        match &mut self.stage {
            Stage::ChordWindow {
                binding_id: bid,
                last_press_held,
                ..
            } if bid == binding_id => {
                *last_press_held = false;
                vec![]
            }
            Stage::Recording {
                binding_id: bid,
                preset,
            } if push_to_talk && bid == binding_id => {
                let bid = bid.clone();
                let preset = preset.clone();
                self.stage = Stage::Processing;
                vec![Effect::StopRecording {
                    binding_id: bid,
                    preset,
                }]
            }
            _ => vec![],
        }
    }

    /// Handle the chord-window expiry timer firing.
    ///
    /// Resolves the preset for the accumulated tap count and either:
    /// - PTT + last release left key un-held → silent cancel back to Idle.
    /// - Otherwise → start recording with the resolved preset.
    ///
    /// `resolve` is consulted only when `count >= 2`. For `count == 1`, the
    /// preset is always plain (None) and the resolver is never called.
    /// If `resolve` returns `None` for `count >= 2`, the stage falls back to
    /// plain recording (silent fallback, per chord-system.md open Q4).
    pub fn on_chord_window_expired<F>(&mut self, push_to_talk: bool, resolve: F) -> Vec<Effect>
    where
        F: FnOnce(u32) -> Option<String>,
    {
        let Stage::ChordWindow {
            binding_id,
            count,
            last_press_held,
        } = &self.stage
        else {
            return vec![];
        };

        let binding_id = binding_id.clone();
        let count = *count;
        let held = *last_press_held;

        let preset = if count == 1 { None } else { resolve(count) };

        if push_to_talk && !held {
            self.stage = Stage::Idle;
            vec![]
        } else {
            self.stage = Stage::Recording {
                binding_id: binding_id.clone(),
                preset: preset.clone(),
            };
            vec![Effect::StartRecording { binding_id, preset }]
        }
    }

    /// Cancel the current chord window or recording. Processing is left alone
    /// so the in-flight transcription pipeline can finish on its own.
    pub fn on_cancel(&mut self) -> Vec<Effect> {
        match self.stage {
            Stage::Idle | Stage::Processing => vec![],
            Stage::ChordWindow { .. } | Stage::Recording { .. } => {
                self.stage = Stage::Idle;
                vec![]
            }
        }
    }

    /// Pipeline finished — return to Idle and accept new input.
    pub fn on_processing_finished(&mut self) -> Vec<Effect> {
        self.stage = Stage::Idle;
        vec![]
    }
}

impl Default for ChordStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_preset(_: u32) -> Option<String> {
        None
    }

    /// Resolver where double-tap (count==2) maps to "preset_2", else None.
    fn double_tap_to_preset_2(count: u32) -> Option<String> {
        if count == 2 {
            Some("preset_2".to_string())
        } else {
            None
        }
    }

    // ---- Initial state ------------------------------------------------------

    #[test]
    fn fresh_state_machine_is_idle() {
        let sm = ChordStateMachine::new();
        assert_eq!(sm.stage(), &Stage::Idle);
    }

    // ---- Chord-window entry & accumulation ---------------------------------

    #[test]
    fn single_press_enters_chord_window_with_count_1_held() {
        let mut sm = ChordStateMachine::new();
        let effects = sm.on_press("transcribe", false);

        assert_eq!(
            sm.stage(),
            &Stage::ChordWindow {
                binding_id: "transcribe".to_string(),
                count: 1,
                last_press_held: true,
            }
        );
        assert_eq!(effects, vec![Effect::ScheduleChordExpiry]);
    }

    #[test]
    fn release_in_chord_window_marks_unheld_no_effects() {
        let mut sm = ChordStateMachine::new();
        sm.on_press("transcribe", false);
        let effects = sm.on_release("transcribe", false);

        assert_eq!(
            sm.stage(),
            &Stage::ChordWindow {
                binding_id: "transcribe".to_string(),
                count: 1,
                last_press_held: false,
            }
        );
        assert_eq!(effects, vec![]);
    }

    #[test]
    fn second_press_increments_count_and_marks_held_again() {
        let mut sm = ChordStateMachine::new();
        sm.on_press("transcribe", false);
        sm.on_release("transcribe", false);
        let effects = sm.on_press("transcribe", false);

        assert_eq!(
            sm.stage(),
            &Stage::ChordWindow {
                binding_id: "transcribe".to_string(),
                count: 2,
                last_press_held: true,
            }
        );
        // Each press in the chord window re-arms the expiry timer.
        assert_eq!(effects, vec![Effect::ScheduleChordExpiry]);
    }

    #[test]
    fn triple_tap_accumulates_to_count_3() {
        let mut sm = ChordStateMachine::new();
        for _ in 0..3 {
            sm.on_press("transcribe", false);
            sm.on_release("transcribe", false);
        }
        // Last release leaves held=false; that's fine, expiry-time logic decides.
        match sm.stage() {
            Stage::ChordWindow {
                count,
                last_press_held,
                ..
            } => {
                assert_eq!(*count, 3);
                assert!(!last_press_held);
            }
            other => panic!("expected ChordWindow with count=3, got {other:?}"),
        }
    }

    // ---- Window expiry: PTT mode -------------------------------------------

    #[test]
    fn expiry_in_ptt_with_held_starts_recording_plain_for_count_1() {
        let mut sm = ChordStateMachine::new();
        sm.on_press("transcribe", true);
        let effects = sm.on_chord_window_expired(true, no_preset);

        assert_eq!(
            sm.stage(),
            &Stage::Recording {
                binding_id: "transcribe".to_string(),
                preset: None,
            }
        );
        assert_eq!(
            effects,
            vec![Effect::StartRecording {
                binding_id: "transcribe".to_string(),
                preset: None,
            }]
        );
    }

    #[test]
    fn expiry_in_ptt_with_released_silently_returns_to_idle() {
        let mut sm = ChordStateMachine::new();
        sm.on_press("transcribe", true);
        sm.on_release("transcribe", true);
        let effects = sm.on_chord_window_expired(true, no_preset);

        // Tap-without-hold in PTT: silent cancel — no recording ever starts.
        assert_eq!(sm.stage(), &Stage::Idle);
        assert_eq!(effects, vec![]);
    }

    // ---- Window expiry: toggle mode ----------------------------------------

    #[test]
    fn expiry_in_toggle_starts_recording_even_when_unheld() {
        let mut sm = ChordStateMachine::new();
        sm.on_press("transcribe", false);
        sm.on_release("transcribe", false);
        let effects = sm.on_chord_window_expired(false, no_preset);

        assert_eq!(
            sm.stage(),
            &Stage::Recording {
                binding_id: "transcribe".to_string(),
                preset: None,
            }
        );
        assert_eq!(
            effects,
            vec![Effect::StartRecording {
                binding_id: "transcribe".to_string(),
                preset: None,
            }]
        );
    }

    // ---- Preset resolution -------------------------------------------------

    #[test]
    fn count_2_resolves_via_resolver_to_double_tap_preset() {
        let mut sm = ChordStateMachine::new();
        sm.on_press("transcribe", false);
        sm.on_release("transcribe", false);
        sm.on_press("transcribe", false);
        let effects = sm.on_chord_window_expired(false, double_tap_to_preset_2);

        assert_eq!(
            sm.stage(),
            &Stage::Recording {
                binding_id: "transcribe".to_string(),
                preset: Some("preset_2".to_string()),
            }
        );
        assert_eq!(
            effects,
            vec![Effect::StartRecording {
                binding_id: "transcribe".to_string(),
                preset: Some("preset_2".to_string()),
            }]
        );
    }

    #[test]
    fn count_1_does_not_consult_resolver() {
        let mut sm = ChordStateMachine::new();
        sm.on_press("transcribe", true);
        // Resolver panics if called — count==1 should short-circuit to plain.
        let effects = sm
            .on_chord_window_expired(true, |_| panic!("resolver must not be called for count==1"));

        assert_eq!(
            effects,
            vec![Effect::StartRecording {
                binding_id: "transcribe".to_string(),
                preset: None,
            }]
        );
    }

    #[test]
    fn unknown_preset_for_count_falls_back_to_plain() {
        // Per chord-system.md open question (4), default to silent fallback (a).
        let mut sm = ChordStateMachine::new();
        // Build up to count=3 with a held final press.
        sm.on_press("transcribe", true);
        sm.on_release("transcribe", true);
        sm.on_press("transcribe", true);
        sm.on_release("transcribe", true);
        sm.on_press("transcribe", true);

        // Resolver only knows about count=2; count=3 returns None.
        let effects = sm.on_chord_window_expired(true, double_tap_to_preset_2);

        assert_eq!(
            sm.stage(),
            &Stage::Recording {
                binding_id: "transcribe".to_string(),
                preset: None,
            }
        );
        assert_eq!(
            effects,
            vec![Effect::StartRecording {
                binding_id: "transcribe".to_string(),
                preset: None,
            }]
        );
    }

    // ---- Recording stop ----------------------------------------------------

    fn enter_recording_ptt(sm: &mut ChordStateMachine) {
        sm.on_press("transcribe", true);
        sm.on_chord_window_expired(true, no_preset);
        debug_assert!(matches!(sm.stage(), Stage::Recording { .. }));
    }

    fn enter_recording_toggle(sm: &mut ChordStateMachine) {
        sm.on_press("transcribe", false);
        sm.on_release("transcribe", false);
        sm.on_chord_window_expired(false, no_preset);
        debug_assert!(matches!(sm.stage(), Stage::Recording { .. }));
    }

    #[test]
    fn release_in_ptt_recording_stops_into_processing() {
        let mut sm = ChordStateMachine::new();
        enter_recording_ptt(&mut sm);

        let effects = sm.on_release("transcribe", true);

        assert_eq!(sm.stage(), &Stage::Processing);
        assert_eq!(
            effects,
            vec![Effect::StopRecording {
                binding_id: "transcribe".to_string(),
                preset: None,
            }]
        );
    }

    #[test]
    fn release_in_toggle_recording_is_ignored() {
        let mut sm = ChordStateMachine::new();
        enter_recording_toggle(&mut sm);

        let stage_before = sm.stage().clone();
        let effects = sm.on_release("transcribe", false);

        assert_eq!(sm.stage(), &stage_before, "toggle: release does not stop");
        assert_eq!(effects, vec![]);
    }

    #[test]
    fn press_in_toggle_recording_same_binding_stops() {
        let mut sm = ChordStateMachine::new();
        enter_recording_toggle(&mut sm);

        let effects = sm.on_press("transcribe", false);

        assert_eq!(sm.stage(), &Stage::Processing);
        assert_eq!(
            effects,
            vec![Effect::StopRecording {
                binding_id: "transcribe".to_string(),
                preset: None,
            }]
        );
    }

    #[test]
    fn stop_carries_preset_from_recording_stage_through_to_caller() {
        // Double-tap-and-hold (PTT) → Recording with preset → release → stop.
        // The preset resolved at chord expiry must surface again on stop so
        // the coordinator knows which post-process pipeline to dispatch to.
        let mut sm = ChordStateMachine::new();
        sm.on_press("transcribe", true);
        sm.on_release("transcribe", true);
        sm.on_press("transcribe", true);
        sm.on_chord_window_expired(true, double_tap_to_preset_2);

        let effects = sm.on_release("transcribe", true);

        assert_eq!(
            effects,
            vec![Effect::StopRecording {
                binding_id: "transcribe".to_string(),
                preset: Some("preset_2".to_string()),
            }]
        );
    }

    #[test]
    fn press_in_ptt_recording_is_ignored() {
        let mut sm = ChordStateMachine::new();
        enter_recording_ptt(&mut sm);

        let stage_before = sm.stage().clone();
        let effects = sm.on_press("transcribe", true);

        assert_eq!(sm.stage(), &stage_before);
        assert_eq!(effects, vec![]);
    }

    // ---- Cancel ------------------------------------------------------------

    #[test]
    fn cancel_during_chord_window_returns_to_idle() {
        let mut sm = ChordStateMachine::new();
        sm.on_press("transcribe", false);

        let effects = sm.on_cancel();

        assert_eq!(sm.stage(), &Stage::Idle);
        assert_eq!(effects, vec![]);
    }

    #[test]
    fn cancel_during_recording_returns_to_idle() {
        let mut sm = ChordStateMachine::new();
        enter_recording_ptt(&mut sm);

        let effects = sm.on_cancel();

        assert_eq!(sm.stage(), &Stage::Idle);
        assert_eq!(effects, vec![]);
    }

    #[test]
    fn cancel_during_processing_is_ignored() {
        let mut sm = ChordStateMachine::new();
        enter_recording_ptt(&mut sm);
        sm.on_release("transcribe", true); // → Processing

        let effects = sm.on_cancel();

        assert_eq!(sm.stage(), &Stage::Processing, "let pipeline finish");
        assert_eq!(effects, vec![]);
    }

    // ---- Processing finished ----------------------------------------------

    #[test]
    fn processing_finished_returns_to_idle() {
        let mut sm = ChordStateMachine::new();
        enter_recording_ptt(&mut sm);
        sm.on_release("transcribe", true); // → Processing

        let effects = sm.on_processing_finished();

        assert_eq!(sm.stage(), &Stage::Idle);
        assert_eq!(effects, vec![]);
    }
}
