use anyhow::Result;
use std::sync::Mutex;
use std::thread::JoinHandle;

/// In-flight state for a streaming transcription pipeline session.
/// Created at recording start, consumed at recording stop.
pub struct StreamingHandle {
    /// Background thread that reads audio chunks and transcribes them
    /// sequentially. Returns the ordered raw texts when the chunk channel
    /// closes (i.e. when the recording stops).
    pub thread: JoinHandle<Result<Vec<String>>>,
    /// Whether the current model is a Whisper engine — controls how chunk
    /// texts are joined (space join vs heuristic punctuation fix-up).
    pub is_whisper: bool,
}

/// Tauri-managed state that bridges `TranscribeAction::start` and `stop`.
/// Only one streaming session is active at a time (enforced by the recording
/// state machine in `AudioRecordingManager`).
pub struct StreamingState {
    handle: Mutex<Option<StreamingHandle>>,
}

impl StreamingState {
    pub fn new() -> Self {
        Self {
            handle: Mutex::new(None),
        }
    }

    pub fn store(&self, handle: StreamingHandle) {
        *self.handle.lock().unwrap() = Some(handle);
    }

    pub fn take(&self) -> Option<StreamingHandle> {
        self.handle.lock().unwrap().take()
    }
}
