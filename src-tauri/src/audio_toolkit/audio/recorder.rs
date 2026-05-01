use std::{
    collections::VecDeque,
    io::Error,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, Sample, SizedSample,
};

use crate::audio_toolkit::{
    audio::{AudioVisualiser, FrameResampler},
    constants,
    vad::{self, VadFrame},
    VoiceActivityDetector,
};

enum Cmd {
    /// `None` = non-streaming (accumulate locally); `Some(tx)` = streaming
    /// (clone each completed chunk to the pipeline thread via `tx`).
    Start(Option<mpsc::Sender<Vec<f32>>>),
    Stop(mpsc::Sender<Vec<Vec<f32>>>),
    Shutdown,
}

enum AudioChunk {
    Samples(Vec<f32>),
    EndOfStream,
}

pub struct AudioRecorder {
    device: Option<Device>,
    cmd_tx: Option<mpsc::Sender<Cmd>>,
    worker_handle: Option<std::thread::JoinHandle<()>>,
    vad: Option<Arc<Mutex<Box<dyn vad::VoiceActivityDetector>>>>,
    level_cb: Option<Arc<dyn Fn(Vec<f32>) + Send + Sync + 'static>>,
    /// 0 = no chunking; >0 = emit a new chunk every N samples (at 16kHz).
    chunk_threshold: usize,
}

impl AudioRecorder {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(AudioRecorder {
            device: None,
            cmd_tx: None,
            worker_handle: None,
            vad: None,
            level_cb: None,
            chunk_threshold: 0,
        })
    }

    /// Emit a new audio chunk every `seconds` seconds of recorded (VAD-filtered)
    /// audio. Chunks are returned by [`stop`] in order; the final call returns
    /// whatever remains (may be shorter than `seconds`). Set `seconds = 0` to
    /// disable chunking (the default — returns a single-element `Vec`).
    pub fn with_chunk_duration_s(mut self, seconds: u32) -> Self {
        self.chunk_threshold =
            crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE as usize * seconds as usize;
        self
    }

    pub fn with_vad(mut self, vad: Box<dyn VoiceActivityDetector>) -> Self {
        self.vad = Some(Arc::new(Mutex::new(vad)));
        self
    }

    pub fn with_level_callback<F>(mut self, cb: F) -> Self
    where
        F: Fn(Vec<f32>) + Send + Sync + 'static,
    {
        self.level_cb = Some(Arc::new(cb));
        self
    }

    pub fn open(&mut self, device: Option<Device>) -> Result<(), Box<dyn std::error::Error>> {
        if self.worker_handle.is_some() {
            return Ok(()); // already open
        }

        let (sample_tx, sample_rx) = mpsc::channel::<AudioChunk>();
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        let host = crate::audio_toolkit::get_cpal_host();
        let device = match device {
            Some(dev) => dev,
            None => host
                .default_input_device()
                .ok_or_else(|| Error::new(std::io::ErrorKind::NotFound, "No input device found"))?,
        };

        let thread_device = device.clone();
        let vad = self.vad.clone();
        // Move the optional level callback into the worker thread
        let level_cb = self.level_cb.clone();
        let chunk_threshold = self.chunk_threshold;

        let worker = std::thread::spawn(move || {
            let stop_flag = Arc::new(AtomicBool::new(false));
            let stop_flag_for_stream = stop_flag.clone();
            let init_result = (|| -> Result<(cpal::Stream, u32), String> {
                let config = AudioRecorder::get_preferred_config(&thread_device)
                    .map_err(|e| format!("Failed to fetch preferred config: {e}"))?;

                let sample_rate = config.sample_rate().0;
                let channels = config.channels() as usize;

                log::info!(
                    "Using device: {:?}\nSample rate: {}\nChannels: {}\nFormat: {:?}",
                    thread_device.name(),
                    sample_rate,
                    channels,
                    config.sample_format()
                );

                let stream = match config.sample_format() {
                    cpal::SampleFormat::U8 => AudioRecorder::build_stream::<u8>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::I8 => AudioRecorder::build_stream::<i8>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::I16 => AudioRecorder::build_stream::<i16>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::I32 => AudioRecorder::build_stream::<i32>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::F32 => AudioRecorder::build_stream::<f32>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    sample_format => {
                        return Err(format!("Unsupported sample format: {sample_format:?}"));
                    }
                };

                stream
                    .play()
                    .map_err(|e| format!("Failed to start microphone stream: {e}"))?;

                Ok((stream, sample_rate))
            })();

            match init_result {
                Ok((stream, sample_rate)) => {
                    let _ = init_tx.send(Ok(()));
                    // Keep the stream alive while we process samples.
                    run_consumer(
                        sample_rate,
                        chunk_threshold,
                        vad,
                        sample_rx,
                        cmd_rx,
                        level_cb,
                        stop_flag,
                    );
                    drop(stream);
                }
                Err(error_message) => {
                    log::error!("{error_message}");
                    let _ = init_tx.send(Err(error_message));
                }
            }
        });

        match init_rx.recv() {
            Ok(Ok(())) => {
                self.device = Some(device);
                self.cmd_tx = Some(cmd_tx);
                self.worker_handle = Some(worker);
                Ok(())
            }
            Ok(Err(error_message)) => {
                let _ = worker.join();
                let kind = if is_microphone_access_denied(&error_message) {
                    std::io::ErrorKind::PermissionDenied
                } else {
                    std::io::ErrorKind::Other
                };
                Err(Box::new(Error::new(kind, error_message)))
            }
            Err(recv_error) => {
                let _ = worker.join();
                Err(Box::new(Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to initialize microphone worker: {recv_error}"),
                )))
            }
        }
    }

    pub fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(tx) = &self.cmd_tx {
            tx.send(Cmd::Start(None))?;
        }
        Ok(())
    }

    /// Like [`start`] but each completed audio chunk is also cloned to
    /// `chunk_tx` as soon as it fills up, so a pipeline thread can begin
    /// transcribing chunk N while the microphone keeps recording chunk N+1.
    /// All chunks are still kept in `pending_chunks` internally and returned
    /// by [`stop`] for WAV saving.
    pub fn start_streaming(
        &self,
        chunk_tx: mpsc::Sender<Vec<f32>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(tx) = &self.cmd_tx {
            tx.send(Cmd::Start(Some(chunk_tx)))?;
        }
        Ok(())
    }

    /// Stop recording and return all audio chunks. Without chunking (default),
    /// returns a single-element `Vec` containing all recorded samples.
    /// With chunking enabled via [`with_chunk_duration_s`], returns one element
    /// per full chunk plus a final element for any remaining samples.
    pub fn stop(&self) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        let (resp_tx, resp_rx) = mpsc::channel::<Vec<Vec<f32>>>();
        if let Some(tx) = &self.cmd_tx {
            tx.send(Cmd::Stop(resp_tx))?;
        }
        Ok(resp_rx.recv()?)
    }

    pub fn close(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(Cmd::Shutdown);
        }
        if let Some(h) = self.worker_handle.take() {
            let _ = h.join();
        }
        self.device = None;
        Ok(())
    }

    fn build_stream<T>(
        device: &cpal::Device,
        config: &cpal::SupportedStreamConfig,
        sample_tx: mpsc::Sender<AudioChunk>,
        channels: usize,
        stop_flag: Arc<AtomicBool>,
    ) -> Result<cpal::Stream, cpal::BuildStreamError>
    where
        T: Sample + SizedSample + Send + 'static,
        f32: cpal::FromSample<T>,
    {
        let mut output_buffer = Vec::new();
        let mut eos_sent = false;

        let stream_cb = move |data: &[T], _: &cpal::InputCallbackInfo| {
            if stop_flag.load(Ordering::Relaxed) {
                if !eos_sent {
                    let _ = sample_tx.send(AudioChunk::EndOfStream);
                    eos_sent = true;
                }
                return;
            }
            eos_sent = false;

            output_buffer.clear();

            if channels == 1 {
                output_buffer.extend(data.iter().map(|&sample| sample.to_sample::<f32>()));
            } else {
                let frame_count = data.len() / channels;
                output_buffer.reserve(frame_count);

                for frame in data.chunks_exact(channels) {
                    let mono_sample = frame
                        .iter()
                        .map(|&sample| sample.to_sample::<f32>())
                        .sum::<f32>()
                        / channels as f32;
                    output_buffer.push(mono_sample);
                }
            }

            if sample_tx
                .send(AudioChunk::Samples(output_buffer.clone()))
                .is_err()
            {
                log::error!("Failed to send samples");
            }
        };

        device.build_input_stream(
            &config.clone().into(),
            stream_cb,
            |err| log::error!("Stream error: {}", err),
            None,
        )
    }

    fn get_preferred_config(
        device: &cpal::Device,
    ) -> Result<cpal::SupportedStreamConfig, Box<dyn std::error::Error>> {
        // Use the device's native/default sample rate and let the FrameResampler
        // in run_consumer() downsample to 16kHz. This avoids forcing hardware into
        // a non-native rate which can cause issues on some devices (Bluetooth
        // codecs, certain ALSA drivers, etc.).
        let default_config = device.default_input_config()?;
        let target_rate = default_config.sample_rate();

        // Try to find the best sample format at the device's default rate
        let supported_configs = match device.supported_input_configs() {
            Ok(configs) => configs,
            Err(e) => {
                log::warn!("Could not enumerate input configs ({e}), using device default");
                return Ok(default_config);
            }
        };
        let mut best_config: Option<cpal::SupportedStreamConfigRange> = None;

        for config_range in supported_configs {
            if config_range.min_sample_rate() <= target_rate
                && config_range.max_sample_rate() >= target_rate
            {
                match best_config {
                    None => best_config = Some(config_range),
                    Some(ref current) => {
                        // Prioritize F32 > I16 > I32 > others
                        let score = |fmt: cpal::SampleFormat| match fmt {
                            cpal::SampleFormat::F32 => 4,
                            cpal::SampleFormat::I16 => 3,
                            cpal::SampleFormat::I32 => 2,
                            _ => 1,
                        };

                        if score(config_range.sample_format()) > score(current.sample_format()) {
                            best_config = Some(config_range);
                        }
                    }
                }
            }
        }

        if let Some(config) = best_config {
            return Ok(config.with_sample_rate(target_rate));
        }

        // Fall back to device default if no config matched (exotic/virtual devices)
        log::warn!(
            "No supported config matched device default rate {:?}, using default config",
            target_rate
        );
        Ok(default_config)
    }
}

pub fn is_microphone_access_denied(error_message: &str) -> bool {
    let normalized = error_message.to_lowercase();
    normalized.contains("access is denied")
        || normalized.contains("permission denied")
        || normalized.contains("0x80070005")
}

pub fn is_no_input_device_error(error_message: &str) -> bool {
    let normalized = error_message.to_lowercase();
    normalized.contains("no input device found")
        || (normalized.contains("failed to fetch preferred config")
            && normalized.contains("coreaudio"))
}

#[cfg(test)]
mod tests {
    use super::{is_microphone_access_denied, is_no_input_device_error, PreRollBuffer};

    // ---- PreRollBuffer (chord-system pre-roll ring buffer) ------------------
    //
    // The buffer accumulates audio that arrived BEFORE recording was committed
    // (during the chord window) and gets spliced into the recording at start
    // time, compensating for the chord-window latency. It must keep only the
    // most recent `target` samples regardless of how much is pushed.
    //
    // See `.planning/chord-system.md` (Phase 3) for the design.

    #[test]
    fn preroll_new_buffer_is_empty() {
        let buf = PreRollBuffer::new(100);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn preroll_push_below_target_retains_all() {
        let mut buf = PreRollBuffer::new(100);
        buf.push(&[1.0_f32, 2.0, 3.0]);
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn preroll_push_at_target_retains_all() {
        let mut buf = PreRollBuffer::new(3);
        buf.push(&[1.0_f32, 2.0, 3.0]);
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn preroll_push_over_target_keeps_only_most_recent() {
        let mut buf = PreRollBuffer::new(3);
        buf.push(&[1.0_f32, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(buf.len(), 3);

        let mut drained = Vec::new();
        buf.drain_into(&mut drained);
        assert_eq!(drained, vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn preroll_accumulating_pushes_are_bounded_to_target() {
        // Push 1000 samples in 100-sample chunks — buffer must cap at target.
        let mut buf = PreRollBuffer::new(50);
        for chunk_start in 0..10 {
            let chunk: Vec<f32> = (0..100).map(|i| (chunk_start * 100 + i) as f32).collect();
            buf.push(&chunk);
        }
        assert_eq!(buf.len(), 50, "buffer must stay bounded to target");

        let mut drained = Vec::new();
        buf.drain_into(&mut drained);
        // Last 50 samples were values 950..1000.
        let expected: Vec<f32> = (950..1000).map(|i| i as f32).collect();
        assert_eq!(drained, expected);
    }

    #[test]
    fn preroll_drain_into_empties_buffer() {
        let mut buf = PreRollBuffer::new(10);
        buf.push(&[1.0_f32, 2.0, 3.0]);

        let mut out = vec![99.0_f32]; // ensure drain APPENDS, not replaces
        buf.drain_into(&mut out);

        assert_eq!(out, vec![99.0, 1.0, 2.0, 3.0]);
        assert_eq!(buf.len(), 0, "buffer empty after drain");
    }

    #[test]
    fn preroll_push_after_drain_works() {
        let mut buf = PreRollBuffer::new(3);
        buf.push(&[1.0_f32, 2.0]);
        let mut tmp = Vec::new();
        buf.drain_into(&mut tmp);

        // Reuse buffer for next chord.
        buf.push(&[10.0_f32, 20.0, 30.0, 40.0]);
        let mut second = Vec::new();
        buf.drain_into(&mut second);
        assert_eq!(second, vec![20.0, 30.0, 40.0]);
    }

    #[test]
    fn preroll_zero_target_keeps_nothing() {
        // Pre-roll disabled (target=0). Pushes are no-ops; drain is empty.
        let mut buf = PreRollBuffer::new(0);
        buf.push(&[1.0_f32, 2.0, 3.0]);
        assert_eq!(buf.len(), 0);

        let mut out = Vec::new();
        buf.drain_into(&mut out);
        assert!(out.is_empty());
    }

    // ---- existing error-string detection tests ------------------------------

    #[test]
    fn detects_access_is_denied() {
        assert!(is_microphone_access_denied("Access is denied"));
    }

    #[test]
    fn detects_permission_denied() {
        assert!(is_microphone_access_denied("permission denied"));
    }

    #[test]
    fn detects_windows_error_code() {
        assert!(is_microphone_access_denied("WASAPI error: 0x80070005"));
    }

    #[test]
    fn does_not_match_unrelated_errors() {
        assert!(!is_microphone_access_denied("device not found"));
    }

    #[test]
    fn detects_no_input_device() {
        assert!(is_no_input_device_error("No input device found"));
    }

    #[test]
    fn detects_coreaudio_config_error() {
        assert!(is_no_input_device_error(
            "Failed to fetch preferred config: A backend-specific error has occurred: An unknown error unknown to the coreaudio-rs API occurred"
        ));
    }

    #[test]
    fn does_not_match_other_errors_for_no_device() {
        assert!(!is_no_input_device_error("permission denied"));
        assert!(!is_no_input_device_error("device not found"));
    }
}

/// Bounded ring buffer of recent audio samples that arrived BEFORE recording
/// was committed. Used by [`run_consumer`] to splice the chord-window's audio
/// into the start of a recording so the user doesn't lose the leading ~200ms
/// of speech to the chord-window latency.
///
/// `target` is the desired retention size in samples (typically
/// `WHISPER_SAMPLE_RATE * PREROLL_MS / 1000`). After every push the buffer is
/// truncated from the front so it never exceeds `target`. Memory is small —
/// 300ms at 16kHz is ~10KB.
///
/// Pre-roll bypasses VAD by design — VAD is for trimming silence within a
/// recording, not for pre-deciding what to capture.
struct PreRollBuffer {
    buf: VecDeque<f32>,
    target: usize,
}

impl PreRollBuffer {
    fn new(target: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(target),
            target,
        }
    }

    fn push(&mut self, samples: &[f32]) {
        if self.target == 0 {
            return;
        }
        self.buf.extend(samples);
        let len = self.buf.len();
        if len > self.target {
            self.buf.drain(..len - self.target);
        }
    }

    fn drain_into(&mut self, out: &mut Vec<f32>) {
        out.extend(self.buf.drain(..));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.buf.len()
    }
}

/// Pre-roll retention window — captures audio that arrived during the
/// chord-system's tap-counting window so a recording started after the
/// window closes still includes the user's first words.
///
/// Stored at the post-resample (16kHz) sample rate, so 300ms ≈ 4800 samples
/// (~19KB). Tunable; see `.planning/chord-system.md` open question (2).
const PREROLL_MS: u32 = 300;

/// Dispatch a completed audio chunk: always push to `pending_chunks` (for WAV
/// saving), and in streaming mode also clone it to the pipeline thread via
/// `live_chunk_tx`. If the receiver has been dropped the sender is cleared and
/// we fall back to local-only accumulation for the rest of the recording.
fn dispatch_chunk(
    chunk: Vec<f32>,
    live_chunk_tx: &mut Option<mpsc::Sender<Vec<f32>>>,
    pending_chunks: &mut Vec<Vec<f32>>,
) {
    if let Some(ref tx) = *live_chunk_tx {
        if tx.send(chunk.clone()).is_err() {
            log::warn!("Streaming pipeline disconnected; falling back to local accumulation");
            *live_chunk_tx = None;
        }
    }
    pending_chunks.push(chunk);
}

fn run_consumer(
    in_sample_rate: u32,
    chunk_threshold: usize,
    vad: Option<Arc<Mutex<Box<dyn vad::VoiceActivityDetector>>>>,
    sample_rx: mpsc::Receiver<AudioChunk>,
    cmd_rx: mpsc::Receiver<Cmd>,
    level_cb: Option<Arc<dyn Fn(Vec<f32>) + Send + Sync + 'static>>,
    stop_flag: Arc<AtomicBool>,
) {
    let mut frame_resampler = FrameResampler::new(
        in_sample_rate as usize,
        constants::WHISPER_SAMPLE_RATE as usize,
        Duration::from_millis(30),
    );

    let preroll_target = (constants::WHISPER_SAMPLE_RATE * PREROLL_MS / 1000) as usize;
    let mut preroll = PreRollBuffer::new(preroll_target);
    let mut processed_samples = Vec::<f32>::new();
    let mut pending_chunks: Vec<Vec<f32>> = Vec::new();
    /// In streaming mode this holds the pipeline thread's sender; `None` in
    /// non-streaming mode.  Dropping it signals EOF to the pipeline receiver.
    let mut live_chunk_tx: Option<mpsc::Sender<Vec<f32>>> = None;
    let mut recording = false;

    // ---------- spectrum visualisation setup ---------------------------- //
    const BUCKETS: usize = 16;
    const WINDOW_SIZE: usize = 512;
    let mut visualizer = AudioVisualiser::new(
        in_sample_rate,
        WINDOW_SIZE,
        BUCKETS,
        400.0,  // vocal_min_hz
        4000.0, // vocal_max_hz
    );

    fn handle_frame(
        samples: &[f32],
        recording: bool,
        vad: &Option<Arc<Mutex<Box<dyn vad::VoiceActivityDetector>>>>,
        out_buf: &mut Vec<f32>,
        preroll: &mut PreRollBuffer,
    ) {
        if !recording {
            preroll.push(samples);
            return;
        }

        if let Some(vad_arc) = vad {
            let mut det = vad_arc.lock().unwrap();
            match det.push_frame(samples).unwrap_or(VadFrame::Speech(samples)) {
                VadFrame::Speech(buf) => out_buf.extend_from_slice(buf),
                VadFrame::Noise => {}
            }
        } else {
            out_buf.extend_from_slice(samples);
        }
    }

    loop {
        let chunk = match sample_rx.recv() {
            Ok(c) => c,
            Err(_) => break, // stream closed
        };

        let raw = match chunk {
            AudioChunk::Samples(s) => s,
            AudioChunk::EndOfStream => continue,
        };

        // ---------- spectrum processing ---------------------------------- //
        if let Some(buckets) = visualizer.feed(&raw) {
            if let Some(cb) = &level_cb {
                cb(buckets);
            }
        }

        // ---------- pipeline -------------------------------------------- //
        frame_resampler.push(&raw, &mut |frame: &[f32]| {
            handle_frame(frame, recording, &vad, &mut processed_samples, &mut preroll)
        });

        // Dispatch completed chunks (streaming: clone to pipeline + keep for
        // WAV; non-streaming: accumulate locally only).
        if chunk_threshold > 0 && recording {
            while processed_samples.len() >= chunk_threshold {
                let chunk: Vec<f32> = processed_samples.drain(..chunk_threshold).collect();
                dispatch_chunk(chunk, &mut live_chunk_tx, &mut pending_chunks);
            }
        }

        // non-blocking check for a command
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Start(chunk_tx_opt) => {
                    stop_flag.store(false, Ordering::Relaxed);
                    processed_samples.clear();
                    pending_chunks.clear();
                    live_chunk_tx = chunk_tx_opt;
                    // Splice the chord-window's audio into the start of the
                    // recording so the user's first words aren't lost to the
                    // 200ms chord-resolve latency.
                    preroll.drain_into(&mut processed_samples);
                    recording = true;
                    visualizer.reset();
                    if let Some(v) = &vad {
                        v.lock().unwrap().reset();
                    }
                }
                Cmd::Stop(reply_tx) => {
                    recording = false;
                    stop_flag.store(true, Ordering::Relaxed);

                    // Drain all remaining audio until the producer confirms end-of-stream.
                    // The cpal callback sees the stop flag, sends EndOfStream, and goes
                    // silent — guaranteeing every captured sample is in the channel
                    // ahead of the sentinel.
                    loop {
                        match sample_rx.recv_timeout(Duration::from_secs(2)) {
                            Ok(AudioChunk::Samples(remaining)) => {
                                frame_resampler.push(&remaining, &mut |frame: &[f32]| {
                                    handle_frame(
                                        frame,
                                        true,
                                        &vad,
                                        &mut processed_samples,
                                        &mut preroll,
                                    )
                                });
                                // Dispatch any newly completed chunks from the drain.
                                if chunk_threshold > 0 {
                                    while processed_samples.len() >= chunk_threshold {
                                        let c: Vec<f32> =
                                            processed_samples.drain(..chunk_threshold).collect();
                                        dispatch_chunk(c, &mut live_chunk_tx, &mut pending_chunks);
                                    }
                                }
                            }
                            Ok(AudioChunk::EndOfStream) => break,
                            Err(_) => {
                                log::warn!("Timed out waiting for EndOfStream from audio callback");
                                break;
                            }
                        }
                    }

                    frame_resampler.finish(&mut |frame: &[f32]| {
                        handle_frame(frame, true, &vad, &mut processed_samples, &mut preroll)
                    });

                    // Drain any full chunks produced by the final resampler flush.
                    if chunk_threshold > 0 {
                        while processed_samples.len() >= chunk_threshold {
                            let c: Vec<f32> = processed_samples.drain(..chunk_threshold).collect();
                            dispatch_chunk(c, &mut live_chunk_tx, &mut pending_chunks);
                        }
                    }

                    // Final partial chunk (or entire audio when chunking is off).
                    let remaining = std::mem::take(&mut processed_samples);
                    if !remaining.is_empty() || pending_chunks.is_empty() {
                        dispatch_chunk(remaining, &mut live_chunk_tx, &mut pending_chunks);
                    }

                    // Drop the pipeline sender — this closes the channel and
                    // signals EOF to the pipeline thread (which then exits its
                    // recv loop and the JoinHandle becomes joinable).
                    live_chunk_tx = None;

                    // Return pending_chunks — contains all audio regardless of
                    // streaming mode, used by the caller for WAV saving.
                    let _ = reply_tx.send(std::mem::take(&mut pending_chunks));

                    // Resume the audio callback so the consumer loop can continue
                    // receiving chunks (important for always-on microphone mode).
                    stop_flag.store(false, Ordering::Relaxed);
                }
                Cmd::Shutdown => {
                    stop_flag.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
    }
}
