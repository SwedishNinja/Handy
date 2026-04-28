use crate::actions::ACTION_MAP;
use crate::chord_state::{ChordStateMachine, Effect, Stage};
use crate::managers::audio::AudioRecordingManager;
use crate::settings;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const DEBOUNCE: Duration = Duration::from_millis(30);

/// Time the user has between consecutive taps for the chord-system to keep
/// counting them as a single chord. After this window elapses with no further
/// presses, [`ChordStateMachine::on_chord_window_expired`] decides whether to
/// start recording.
const CHORD_WINDOW: Duration = Duration::from_millis(200);

/// Commands processed sequentially by the coordinator thread.
enum Command {
    Input {
        binding_id: String,
        hotkey_string: String,
        is_pressed: bool,
        push_to_talk: bool,
    },
    Cancel,
    ProcessingFinished,
    /// Fired by a one-shot timer thread once the chord window elapses.
    /// `token` is matched against the coordinator's monotonic counter so that
    /// stale fires (a new press has since rescheduled) are dismissed.
    ChordWindowExpired {
        token: u64,
    },
}

/// Serialises all transcription lifecycle events through a single thread
/// to eliminate race conditions between keyboard shortcuts, signals, and
/// the async transcribe-paste pipeline.
pub struct TranscriptionCoordinator {
    tx: Sender<Command>,
}

pub fn is_transcribe_binding(id: &str) -> bool {
    id == "transcribe"
}

impl TranscriptionCoordinator {
    pub fn new(app: AppHandle) -> Self {
        let (tx, rx) = mpsc::channel();
        let timer_tx = tx.clone();

        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut sm = ChordStateMachine::new();
                let mut last_press: Option<Instant> = None;
                let mut chord_window_token: u64 = 0;
                // Captured at chord-window start. Mode shouldn't change mid-chord;
                // if it does, we lock in the value from the first press.
                let mut chord_push_to_talk: bool = true;
                // Hotkey strings are needed when actions fire (which can happen
                // long after the originating press if the chord window is wide).
                let mut chord_hotkeys: HashMap<String, String> = HashMap::new();

                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        Command::Input {
                            binding_id,
                            hotkey_string,
                            is_pressed,
                            push_to_talk,
                        } => {
                            // Debounce rapid-fire press events (key repeat /
                            // double-tap within hardware repeat interval).
                            // Releases always pass through for push-to-talk.
                            if is_pressed {
                                let now = Instant::now();
                                if last_press.map_or(false, |t| now.duration_since(t) < DEBOUNCE) {
                                    debug!("Debounced press for '{binding_id}'");
                                    continue;
                                }
                                last_press = Some(now);
                            }

                            chord_hotkeys.insert(binding_id.clone(), hotkey_string.clone());
                            if is_pressed && matches!(sm.stage(), Stage::Idle) {
                                chord_push_to_talk = push_to_talk;
                            }

                            let effects = if is_pressed {
                                sm.on_press(&binding_id, push_to_talk)
                            } else {
                                sm.on_release(&binding_id, push_to_talk)
                            };

                            // Surface chord-window state to the log so users
                            // diagnosing "wrong preset / no preset" can see
                            // exactly what tap count was registered.
                            if let Stage::ChordWindow {
                                count,
                                last_press_held,
                                ..
                            } = sm.stage()
                            {
                                debug!(
                                    "chord window: count={count} held={last_press_held} ptt={chord_push_to_talk}"
                                );
                            }

                            apply_effects(
                                &app,
                                effects,
                                &chord_hotkeys,
                                &mut chord_window_token,
                                &timer_tx,
                                &mut sm,
                            );
                        }
                        Command::ChordWindowExpired { token } => {
                            if token != chord_window_token {
                                debug!(
                                    "Stale chord-window expiry (token {token} != {chord_window_token}); ignoring"
                                );
                                continue;
                            }

                            let app_for_resolve = app.clone();
                            // Capture count for logging before the closure consumes it.
                            let logged_count = if let Stage::ChordWindow { count, .. } = sm.stage()
                            {
                                Some(*count)
                            } else {
                                None
                            };
                            let effects = sm.on_chord_window_expired(chord_push_to_talk, |count| {
                                settings::get_settings(&app_for_resolve)
                                    .preset_id_for_chord_count(count)
                            });

                            // Log the resolved preset (if any) — this is the
                            // single source of truth for "did the chord fire
                            // and what did it pick?".
                            if let Some(count) = logged_count {
                                let preset = effects.iter().find_map(|e| match e {
                                    Effect::StartRecording { preset, .. } => preset.clone(),
                                    _ => None,
                                });
                                info!(
                                    "chord resolved: count={count} preset={:?} ptt={chord_push_to_talk}",
                                    preset
                                );
                            }

                            apply_effects(
                                &app,
                                effects,
                                &chord_hotkeys,
                                &mut chord_window_token,
                                &timer_tx,
                                &mut sm,
                            );
                        }
                        Command::Cancel => {
                            // Bump token so any in-flight chord-window timer is
                            // dismissed when it eventually fires.
                            chord_window_token = chord_window_token.wrapping_add(1);
                            let _ = sm.on_cancel();
                        }
                        Command::ProcessingFinished => {
                            chord_window_token = chord_window_token.wrapping_add(1);
                            let _ = sm.on_processing_finished();
                        }
                    }
                }
                debug!("Transcription coordinator exited");
            }));
            if let Err(e) = result {
                error!("Transcription coordinator panicked: {e:?}");
            }
        });

        Self { tx }
    }

    /// Send a keyboard/signal input event for a transcribe binding.
    /// For signal-based toggles, use `is_pressed: true` and `push_to_talk: false`.
    pub fn send_input(
        &self,
        binding_id: &str,
        hotkey_string: &str,
        is_pressed: bool,
        push_to_talk: bool,
    ) {
        if self
            .tx
            .send(Command::Input {
                binding_id: binding_id.to_string(),
                hotkey_string: hotkey_string.to_string(),
                is_pressed,
                push_to_talk,
            })
            .is_err()
        {
            warn!("Transcription coordinator channel closed");
        }
    }

    /// `recording_was_active` was a divergence-recovery flag in the legacy
    /// coordinator. The state machine is now the single source of truth, so
    /// we accept the parameter for caller compatibility but ignore it.
    pub fn notify_cancel(&self, _recording_was_active: bool) {
        if self.tx.send(Command::Cancel).is_err() {
            warn!("Transcription coordinator channel closed");
        }
    }

    pub fn notify_processing_finished(&self) {
        if self.tx.send(Command::ProcessingFinished).is_err() {
            warn!("Transcription coordinator channel closed");
        }
    }
}

/// Apply the side effects emitted by the state machine.
fn apply_effects(
    app: &AppHandle,
    effects: Vec<Effect>,
    hotkeys: &HashMap<String, String>,
    token: &mut u64,
    timer_tx: &Sender<Command>,
    sm: &mut ChordStateMachine,
) {
    for effect in effects {
        match effect {
            Effect::ScheduleChordExpiry => {
                *token = token.wrapping_add(1);
                let token_now = *token;
                let timer_tx = timer_tx.clone();
                thread::spawn(move || {
                    thread::sleep(CHORD_WINDOW);
                    let _ = timer_tx.send(Command::ChordWindowExpired { token: token_now });
                });
            }
            Effect::StartRecording { binding_id, preset } => {
                let Some(action) = ACTION_MAP.get(&binding_id) else {
                    warn!("No action in ACTION_MAP for '{binding_id}'");
                    let _ = sm.on_cancel();
                    continue;
                };
                let hotkey = hotkeys.get(&binding_id).cloned().unwrap_or_default();

                // Emit the human-readable preset name so the recording overlay
                // can display "Recording: Clean-up" instead of just "Recording".
                // `None` clears any previous preset label from a prior recording.
                let preset_name = preset.as_deref().and_then(|prompt_id| {
                    settings::get_settings(app)
                        .post_process_prompts
                        .iter()
                        .find(|p| p.id == prompt_id)
                        .map(|p| p.name.clone())
                });
                let _ = app.emit("chord-preset", preset_name);

                action.start(app, &binding_id, &hotkey, preset.as_deref());

                // Roll back state machine if recording didn't actually begin
                // (e.g. mic permission denied, no input device). The action
                // itself already surfaces the error to the UI.
                let actually_recording = app
                    .try_state::<Arc<AudioRecordingManager>>()
                    .map_or(false, |a| a.is_recording());
                if !actually_recording {
                    debug!("Start for '{binding_id}' did not begin recording; rolling back");
                    let _ = sm.on_cancel();
                }
            }
            Effect::StopRecording { binding_id, preset } => {
                let Some(action) = ACTION_MAP.get(&binding_id) else {
                    warn!("No action in ACTION_MAP for '{binding_id}'");
                    continue;
                };
                let hotkey = hotkeys.get(&binding_id).cloned().unwrap_or_default();
                action.stop(app, &binding_id, &hotkey, preset.as_deref());
            }
        }
    }
}
