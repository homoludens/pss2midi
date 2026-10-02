//! GPUI-independent command and event boundary for the sound engine.

use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, SendError, Sender, TryRecvError},
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Parser;

use crate::engine::{
    audio_device::{
        choose_initial_device, is_selectable_device, AlsaAudioDeviceProvider, AudioDeviceProvider,
        AudioDeviceUnavailable, AudioInputDevice, OpenedAudioCapture, PIPEWIRE_DEFAULT_DEVICE_ID,
    },
    config::{Cli, DetectorMode, RunArgs},
    detector::{Detector, DetectorOutcome, NoteDecision, SpectralResult, YinResult},
    midi::Midi,
};

const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const AUDIO_LEVEL_INTERVAL: Duration = Duration::from_millis(40);

/// Commands sent to the engine's resource-owning worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineCommand {
    Start,
    Stop,
    SetDetectorMode(DetectorMode),
    SetAudioDevice { device: String },
    EnumerateAudioDevices,
    RetryAudioDevice,
    BeginCalibration { samples_per_note: usize },
    RetryCalibrationSample,
    CancelCalibration,
    ReloadTemplates { path: Option<PathBuf> },
    Shutdown,
}

/// Coarse engine lifecycle state reported to consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineState {
    Stopped,
    Running,
    ShuttingDown,
}

/// High-level updates that can be consumed by a UI or another client.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    WorkerStarted,
    WorkerStopped,
    StateChanged(EngineState),
    Onset,
    Detection {
        selected_note: Option<u8>,
        yin: Option<YinResult>,
        spectral: Option<SpectralResult>,
        onset_to_note_latency: Option<Duration>,
    },
    NoteOn {
        midi_note: u8,
    },
    NoteOff {
        midi_note: u8,
    },
    AudioLevel {
        rms_dbfs: f32,
        peak_dbfs: f32,
    },
    AudioDevicesEnumerated {
        devices: Vec<AudioInputDevice>,
        selected_device: String,
    },
    AudioDeviceSelected {
        device_id: String,
    },
    AudioDeviceOpened {
        device_id: String,
    },
    AudioDeviceUnavailable {
        device_id: String,
        message: String,
    },
    AudioDeviceEnumerationFailed {
        message: String,
    },
    Error {
        message: String,
    },
}

/// Controller for the engine worker. Hardware and detector resources never
/// leave that worker; callers exchange commands and events through channels.
pub struct PssEngine {
    command_tx: Sender<EngineCommand>,
    event_rx: Receiver<EngineEvent>,
    worker: Option<JoinHandle<()>>,
}

impl PssEngine {
    /// Start an idle worker without opening audio or MIDI devices.
    pub fn new() -> io::Result<Self> {
        Self::new_with_audio_device_preference(None)
    }

    /// Start an idle worker, preferring `saved_device_id` when ALSA reports it.
    /// If it is unavailable, the established PipeWire default is selected.
    pub fn new_with_audio_device_preference(saved_device_id: Option<String>) -> io::Result<Self> {
        let args = default_run_args();
        let settings = WorkerSettings::from_args_and_preference(&args, saved_device_id);
        Self::spawn_with_runtime(
            move || ProductionRuntime::new(args, Box::<AlsaAudioDeviceProvider>::default()),
            settings,
        )
    }

    /// Send a typed command to the worker.
    pub fn send(&self, command: EngineCommand) -> Result<(), SendError<EngineCommand>> {
        self.command_tx.send(command)
    }

    /// Wait for the next high-level worker event.
    pub fn recv_event(&self) -> Result<EngineEvent, RecvError> {
        self.event_rx.recv()
    }

    /// Check for a worker event without blocking.
    pub fn try_recv_event(&self) -> Result<EngineEvent, TryRecvError> {
        self.event_rx.try_recv()
    }

    /// Ask the worker to stop and join it. Repeated calls are harmless.
    pub fn shutdown(&mut self) -> io::Result<()> {
        // A disconnected worker is still joined below, so shutdown remains
        // reliable if it has already exited or panicked.
        let _ = self.command_tx.send(EngineCommand::Shutdown);

        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "engine worker panicked"))?;
        }

        Ok(())
    }

    fn spawn_with_runtime<F, R>(runtime_factory: F, settings: WorkerSettings) -> io::Result<Self>
    where
        F: FnOnce() -> R + Send + 'static,
        R: WorkerRuntime,
    {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("pss2midi-engine".to_owned())
            .spawn(move || {
                let runtime = runtime_factory();
                run_worker(command_rx, event_tx, runtime, settings);
            })?;

        Ok(Self {
            command_tx,
            event_rx,
            worker: Some(worker),
        })
    }
}

impl Drop for PssEngine {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkerSettings {
    mode: DetectorMode,
    device: String,
    preferred_device: Option<String>,
    pending_device: Option<String>,
    device_selection_resolved: bool,
}

impl WorkerSettings {
    #[cfg(test)]
    fn from_args(args: &RunArgs) -> Self {
        Self::from_args_and_preference(args, None)
    }

    fn from_args_and_preference(args: &RunArgs, preferred_device: Option<String>) -> Self {
        Self {
            mode: args.detector,
            device: args.audio.device.clone(),
            preferred_device,
            pending_device: None,
            device_selection_resolved: false,
        }
    }
}

#[derive(Debug)]
enum RuntimeFailure {
    AudioDevice(AudioDeviceUnavailable),
    Other(String),
}

impl RuntimeFailure {
    fn audio_device(device_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self::AudioDevice(AudioDeviceUnavailable {
            device_id: device_id.into(),
            message: message.into(),
        })
    }
}

/// Runtime operations are injectable so the worker's lifecycle and command
/// scheduling can be exercised without ALSA or MIDI hardware.
trait WorkerRuntime: 'static {
    fn enumerate_audio_devices(&mut self) -> Result<Vec<AudioInputDevice>, String>;
    fn start(&mut self, settings: &WorkerSettings) -> Result<(), RuntimeFailure>;
    fn stop(&mut self, events: &mut Vec<EngineEvent>) -> Result<(), String>;
    fn capture_step(
        &mut self,
        max_wait: Duration,
        events: &mut Vec<EngineEvent>,
    ) -> Result<(), RuntimeFailure>;
}

fn run_worker<R>(
    command_rx: Receiver<EngineCommand>,
    event_tx: Sender<EngineEvent>,
    mut runtime: R,
    mut settings: WorkerSettings,
) where
    R: WorkerRuntime,
{
    let _ = event_tx.send(EngineEvent::WorkerStarted);
    let mut running = false;

    loop {
        let command = if running {
            match command_rx.recv_timeout(COMMAND_POLL_INTERVAL) {
                Ok(command) => command,
                Err(RecvTimeoutError::Timeout) => {
                    let mut events = Vec::new();
                    if let Err(error) = runtime.capture_step(CAPTURE_POLL_INTERVAL, &mut events) {
                        publish_events(&event_tx, events);
                        publish_runtime_failure(&event_tx, error);
                        stop_runtime(&mut runtime, &event_tx);
                        running = false;
                        let _ = event_tx.send(EngineEvent::StateChanged(EngineState::Stopped));
                    } else {
                        publish_events(&event_tx, events);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match command_rx.recv() {
                Ok(command) => command,
                Err(_) => break,
            }
        };

        match command {
            EngineCommand::Start if !running => {
                resolve_initial_device(&mut runtime, &event_tx, &mut settings);
                running = start_runtime(&mut runtime, &settings, &event_tx);
            }
            EngineCommand::Start => {}
            EngineCommand::Stop if running => {
                stop_runtime(&mut runtime, &event_tx);
                running = false;
                let _ = event_tx.send(EngineEvent::StateChanged(EngineState::Stopped));
            }
            EngineCommand::Stop => {}
            EngineCommand::SetDetectorMode(mode) if settings.mode != mode => {
                let mut next_settings = settings.clone();
                next_settings.mode = mode;
                running = reconfigure_runtime(
                    &mut runtime,
                    &event_tx,
                    &mut settings,
                    next_settings,
                    running,
                );
            }
            EngineCommand::SetDetectorMode(_) => {}
            EngineCommand::SetAudioDevice { device } if settings.device != device => {
                running =
                    select_audio_device(&mut runtime, &event_tx, &mut settings, running, device);
            }
            EngineCommand::SetAudioDevice { device } => {
                let _ =
                    select_audio_device(&mut runtime, &event_tx, &mut settings, running, device);
            }
            EngineCommand::EnumerateAudioDevices => {
                enumerate_audio_devices(&mut runtime, &event_tx, &mut settings);
            }
            EngineCommand::RetryAudioDevice => {
                running = retry_audio_device(&mut runtime, &event_tx, &mut settings, running);
            }
            EngineCommand::Shutdown => break,
            EngineCommand::BeginCalibration { .. }
            | EngineCommand::RetryCalibrationSample
            | EngineCommand::CancelCalibration
            | EngineCommand::ReloadTemplates { .. } => {
                let _ = event_tx.send(EngineEvent::Error {
                    message: "Calibration and template reload commands are not available yet"
                        .to_owned(),
                });
            }
        }
    }

    let _ = event_tx.send(EngineEvent::StateChanged(EngineState::ShuttingDown));
    if running {
        stop_runtime(&mut runtime, &event_tx);
        let _ = event_tx.send(EngineEvent::StateChanged(EngineState::Stopped));
    }
    drop(runtime);
    let _ = event_tx.send(EngineEvent::WorkerStopped);
}

fn start_runtime<R: WorkerRuntime>(
    runtime: &mut R,
    settings: &WorkerSettings,
    event_tx: &Sender<EngineEvent>,
) -> bool {
    match runtime.start(settings) {
        Ok(()) => {
            let _ = event_tx.send(EngineEvent::AudioDeviceOpened {
                device_id: settings.device.clone(),
            });
            let _ = event_tx.send(EngineEvent::StateChanged(EngineState::Running));
            true
        }
        Err(error) => {
            publish_runtime_failure(event_tx, error);
            let _ = event_tx.send(EngineEvent::StateChanged(EngineState::Stopped));
            false
        }
    }
}

fn resolve_initial_device<R: WorkerRuntime>(
    runtime: &mut R,
    event_tx: &Sender<EngineEvent>,
    settings: &mut WorkerSettings,
) {
    if settings.device_selection_resolved {
        return;
    }

    match runtime.enumerate_audio_devices() {
        Ok(devices) => {
            settings.device = choose_initial_device(&devices, settings.preferred_device.as_deref());
            settings.preferred_device = None;
            settings.device_selection_resolved = true;
            let _ = event_tx.send(EngineEvent::AudioDevicesEnumerated {
                devices,
                selected_device: settings.device.clone(),
            });
        }
        Err(message) => {
            let _ = event_tx.send(EngineEvent::AudioDeviceEnumerationFailed { message });
            // Preserve the historical PipeWire default if discovery itself is
            // temporarily unavailable. Opening it still has to succeed before
            // the worker reports the device as active.
            settings.device = PIPEWIRE_DEFAULT_DEVICE_ID.to_owned();
            settings.preferred_device = None;
            settings.device_selection_resolved = true;
        }
    }
}

fn enumerate_audio_devices<R: WorkerRuntime>(
    runtime: &mut R,
    event_tx: &Sender<EngineEvent>,
    settings: &mut WorkerSettings,
) {
    match runtime.enumerate_audio_devices() {
        Ok(devices) => {
            if !settings.device_selection_resolved {
                settings.device =
                    choose_initial_device(&devices, settings.preferred_device.as_deref());
                settings.preferred_device = None;
                settings.pending_device = None;
                settings.device_selection_resolved = true;
            }
            let _ = event_tx.send(EngineEvent::AudioDevicesEnumerated {
                devices,
                selected_device: settings.device.clone(),
            });
        }
        Err(message) => {
            let _ = event_tx.send(EngineEvent::AudioDeviceEnumerationFailed { message });
        }
    }
}

fn select_audio_device<R: WorkerRuntime>(
    runtime: &mut R,
    event_tx: &Sender<EngineEvent>,
    settings: &mut WorkerSettings,
    was_running: bool,
    device_id: String,
) -> bool {
    let devices = match runtime.enumerate_audio_devices() {
        Ok(devices) => devices,
        Err(message) => {
            let _ = event_tx.send(EngineEvent::AudioDeviceEnumerationFailed { message });
            return was_running;
        }
    };
    if !is_selectable_device(&devices, &device_id) {
        settings.pending_device = Some(device_id.clone());
        let _ = event_tx.send(EngineEvent::AudioDeviceUnavailable {
            device_id: device_id.clone(),
            message: "The selected audio input is not currently available".to_owned(),
        });
        return was_running;
    }

    let mut next_settings = settings.clone();
    next_settings.device = device_id.clone();
    next_settings.preferred_device = None;
    next_settings.pending_device = None;
    next_settings.device_selection_resolved = true;
    let _ = event_tx.send(EngineEvent::AudioDeviceSelected { device_id });
    if next_settings.device == settings.device {
        return was_running;
    }

    reconfigure_runtime(runtime, event_tx, settings, next_settings, was_running)
}

fn retry_audio_device<R: WorkerRuntime>(
    runtime: &mut R,
    event_tx: &Sender<EngineEvent>,
    settings: &mut WorkerSettings,
    was_running: bool,
) -> bool {
    if was_running && settings.pending_device.is_none() {
        return true;
    }
    let target_device = settings
        .pending_device
        .clone()
        .unwrap_or_else(|| settings.device.clone());
    let devices = match runtime.enumerate_audio_devices() {
        Ok(devices) => devices,
        Err(message) => {
            let _ = event_tx.send(EngineEvent::AudioDeviceEnumerationFailed { message });
            return was_running;
        }
    };
    if !is_selectable_device(&devices, &target_device) {
        let _ = event_tx.send(EngineEvent::AudioDeviceUnavailable {
            device_id: target_device,
            message: "The selected audio input is not currently available".to_owned(),
        });
        return was_running;
    }

    let mut next_settings = settings.clone();
    next_settings.device = target_device.clone();
    next_settings.preferred_device = None;
    next_settings.pending_device = None;
    next_settings.device_selection_resolved = true;
    if next_settings.device != settings.device {
        let _ = event_tx.send(EngineEvent::AudioDeviceSelected {
            device_id: target_device,
        });
        if was_running {
            reconfigure_runtime(runtime, event_tx, settings, next_settings, true)
        } else {
            *settings = next_settings;
            start_runtime(runtime, settings, event_tx)
        }
    } else {
        settings.pending_device = None;
        if was_running {
            true
        } else {
            start_runtime(runtime, settings, event_tx)
        }
    }
}

fn publish_runtime_failure(event_tx: &Sender<EngineEvent>, error: RuntimeFailure) {
    match error {
        RuntimeFailure::AudioDevice(error) => {
            let _ = event_tx.send(EngineEvent::AudioDeviceUnavailable {
                device_id: error.device_id,
                message: error.message,
            });
        }
        RuntimeFailure::Other(message) => {
            let _ = event_tx.send(EngineEvent::Error { message });
        }
    }
}

fn stop_runtime<R: WorkerRuntime>(runtime: &mut R, event_tx: &Sender<EngineEvent>) -> bool {
    let mut events = Vec::new();
    let result = runtime.stop(&mut events);
    publish_events(event_tx, events);
    if let Err(message) = result {
        let _ = event_tx.send(EngineEvent::Error { message });
        false
    } else {
        true
    }
}

fn reconfigure_runtime<R: WorkerRuntime>(
    runtime: &mut R,
    event_tx: &Sender<EngineEvent>,
    settings: &mut WorkerSettings,
    next_settings: WorkerSettings,
    was_running: bool,
) -> bool {
    let stopped_cleanly = if was_running {
        let stopped_cleanly = stop_runtime(runtime, event_tx);
        let _ = event_tx.send(EngineEvent::StateChanged(EngineState::Stopped));
        stopped_cleanly
    } else {
        true
    };
    *settings = next_settings;

    if was_running && stopped_cleanly {
        start_runtime(runtime, settings, event_tx)
    } else {
        false
    }
}

fn publish_events(event_tx: &Sender<EngineEvent>, events: Vec<EngineEvent>) {
    for event in events {
        let _ = event_tx.send(event);
    }
}

struct ProductionRuntime {
    args: RunArgs,
    audio_device_provider: Box<dyn AudioDeviceProvider>,
    resources: WorkerResources,
    outcome_publisher: OutcomeEventPublisher,
}

impl ProductionRuntime {
    fn new(args: RunArgs, audio_device_provider: Box<dyn AudioDeviceProvider>) -> Self {
        Self {
            args,
            audio_device_provider,
            resources: WorkerResources::default(),
            outcome_publisher: OutcomeEventPublisher::default(),
        }
    }

    fn apply_decisions(
        &mut self,
        decisions: &[NoteDecision],
        events: &mut Vec<EngineEvent>,
    ) -> Result<()> {
        if decisions.is_empty() {
            return Ok(());
        }
        let resources = &mut self.resources;
        let midi = resources.midi.as_mut().context("MIDI output is not open")?;
        apply_note_decisions(midi, &mut resources.active_note, decisions, events)
    }
}

impl WorkerRuntime for ProductionRuntime {
    fn enumerate_audio_devices(&mut self) -> Result<Vec<AudioInputDevice>, String> {
        self.audio_device_provider.enumerate_capture_devices()
    }

    fn start(&mut self, settings: &WorkerSettings) -> Result<(), RuntimeFailure> {
        let mut args = self.args.clone();
        args.detector = settings.mode;
        args.audio.device = settings.device.clone();

        // Construct into locals so a partial failure drops everything opened
        // for this attempted start and leaves the runtime stopped.
        let capture = self
            .audio_device_provider
            .open_capture(&settings.device, &args.audio)
            .map_err(RuntimeFailure::AudioDevice)?;
        let sample_rate = capture.sample_rate;
        let hop = capture.hop;
        let midi = Midi::new().map_err(|error| RuntimeFailure::Other(format!("{error:#}")))?;
        let detector = Detector::new(&args, sample_rate, hop)
            .context("Cannot create detector")
            .map_err(|error| RuntimeFailure::Other(format!("{error:#}")))?;

        self.args = args;
        self.resources = WorkerResources {
            capture: Some(capture),
            detector: Some(detector),
            midi: Some(midi),
            active_note: None,
            frame: vec![0.0; hop],
            active_device: Some(settings.device.clone()),
        };
        self.outcome_publisher = OutcomeEventPublisher::default();
        Ok(())
    }

    fn stop(&mut self, events: &mut Vec<EngineEvent>) -> Result<(), String> {
        let decisions = self
            .resources
            .detector
            .as_mut()
            .map(Detector::shutdown)
            .unwrap_or_default();
        let mut first_error = self
            .apply_decisions(&decisions, events)
            .err()
            .map(|error| format!("{error:#}"));

        if self.resources.active_note.is_some() {
            let release_result = match self.resources.midi.as_mut() {
                Some(midi) => release_active_note(midi, &mut self.resources.active_note, events),
                None => Err(anyhow::anyhow!(
                    "MIDI output is not open for active note release"
                )),
            };
            if let Err(error) = release_result {
                if first_error.is_none() {
                    first_error = Some(format!("{error:#}"));
                }
            }
        }

        // Notes are released before any of the old resources are dropped.
        self.resources.detector.take();
        self.resources.capture.take();
        self.resources.midi.take();
        self.resources.active_note = None;
        self.resources.active_device = None;
        self.resources.frame.clear();
        self.outcome_publisher = OutcomeEventPublisher::default();

        match first_error {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }

    fn capture_step(
        &mut self,
        max_wait: Duration,
        events: &mut Vec<EngineEvent>,
    ) -> Result<(), RuntimeFailure> {
        let frame_ready = {
            let resources = &mut self.resources;
            let Some(capture) = resources.capture.as_mut() else {
                return Err(RuntimeFailure::Other(
                    "Capture stream is not open".to_owned(),
                ));
            };
            capture
                .try_next_frame(&mut resources.frame, max_wait)
                .map_err(|error| {
                    RuntimeFailure::audio_device(
                        resources.active_device.as_deref().unwrap_or("unknown"),
                        format!("{error:#}"),
                    )
                })?
                .is_some()
        };
        if !frame_ready {
            return Ok(());
        }

        let outcome = self
            .resources
            .detector
            .as_mut()
            .context("Detector is not running")
            .and_then(|detector| detector.process(&self.resources.frame))
            .map_err(|error| RuntimeFailure::Other(format!("{error:#}")))?;

        let resources = &mut self.resources;
        self.outcome_publisher
            .publish(
                &outcome,
                resources.midi.as_mut(),
                &mut resources.active_note,
                events,
            )
            .map_err(|error| RuntimeFailure::Other(format!("{error:#}")))
    }
}

#[derive(Default)]
struct WorkerResources {
    capture: Option<OpenedAudioCapture>,
    detector: Option<Detector>,
    midi: Option<Midi>,
    active_note: Option<u8>,
    frame: Vec<f32>,
    active_device: Option<String>,
}

trait MidiPort {
    fn note_on(&mut self, note: u8) -> Result<()>;
    fn note_off(&mut self, note: u8) -> Result<()>;
}

impl MidiPort for Midi {
    fn note_on(&mut self, note: u8) -> Result<()> {
        Midi::note_on(self, note)
    }

    fn note_off(&mut self, note: u8) -> Result<()> {
        Midi::note_off(self, note)
    }
}

fn apply_note_decisions<M: MidiPort>(
    midi: &mut M,
    active_note: &mut Option<u8>,
    decisions: &[NoteDecision],
    events: &mut Vec<EngineEvent>,
) -> Result<()> {
    for decision in decisions {
        match *decision {
            NoteDecision::NoteOn { note } => {
                midi.note_on(note)?;
                *active_note = Some(note);
                events.push(EngineEvent::NoteOn { midi_note: note });
            }
            NoteDecision::NoteOff { note } => {
                midi.note_off(note)?;
                if *active_note == Some(note) {
                    *active_note = None;
                }
                events.push(EngineEvent::NoteOff { midi_note: note });
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct OutcomeEventPublisher {
    last_audio_level_at: Option<std::time::Instant>,
}

impl OutcomeEventPublisher {
    fn publish<M: MidiPort>(
        &mut self,
        outcome: &DetectorOutcome,
        midi: Option<&mut M>,
        active_note: &mut Option<u8>,
        events: &mut Vec<EngineEvent>,
    ) -> Result<()> {
        self.publish_at(
            outcome,
            midi,
            active_note,
            events,
            std::time::Instant::now(),
        )
    }

    fn publish_at<M: MidiPort>(
        &mut self,
        outcome: &DetectorOutcome,
        midi: Option<&mut M>,
        active_note: &mut Option<u8>,
        events: &mut Vec<EngineEvent>,
        now: std::time::Instant,
    ) -> Result<()> {
        let emit_audio_level = self.last_audio_level_at.map_or(true, |last| {
            now.saturating_duration_since(last) >= AUDIO_LEVEL_INTERVAL
        });
        if emit_audio_level {
            self.last_audio_level_at = Some(now);
        }
        append_detector_outcome_events(outcome, midi, active_note, events, emit_audio_level)
    }
}

fn append_detector_outcome_events<M: MidiPort>(
    outcome: &DetectorOutcome,
    midi: Option<&mut M>,
    active_note: &mut Option<u8>,
    events: &mut Vec<EngineEvent>,
    emit_audio_level: bool,
) -> Result<()> {
    if outcome.onset_detected {
        events.push(EngineEvent::Onset);
    }
    if !outcome.note_decisions.is_empty() {
        let midi = midi.context("MIDI output is not open")?;
        apply_note_decisions(midi, active_note, &outcome.note_decisions, events)?;
    }
    let has_detection_update = outcome.onset_detected
        || !outcome.note_decisions.is_empty()
        || outcome
            .yin
            .as_ref()
            .is_some_and(|yin| yin.decision.is_some())
        || outcome.spectral.is_some();
    if has_detection_update {
        events.push(EngineEvent::Detection {
            selected_note: outcome.selected_note,
            yin: outcome.yin.clone(),
            spectral: outcome.spectral.clone(),
            onset_to_note_latency: outcome.onset_to_note_latency,
        });
    }
    if emit_audio_level {
        events.push(EngineEvent::AudioLevel {
            rms_dbfs: outcome.rms_dbfs,
            peak_dbfs: peak_to_dbfs(outcome.peak),
        });
    }
    Ok(())
}

fn peak_to_dbfs(peak: f32) -> f32 {
    // Keep exact silence aligned with the detector's -120 dBFS RMS floor.
    20.0 * peak.max(1e-6).log10()
}

fn release_active_note<M: MidiPort>(
    midi: &mut M,
    active_note: &mut Option<u8>,
    events: &mut Vec<EngineEvent>,
) -> Result<()> {
    if let Some(note) = *active_note {
        midi.note_off(note)?;
        *active_note = None;
        events.push(EngineEvent::NoteOff { midi_note: note });
    }
    Ok(())
}

fn default_run_args() -> RunArgs {
    Cli::try_parse_from(["pss2midi"])
        .expect("the built-in engine defaults must be valid")
        .run
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
        time::Instant,
    };

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum FakeAction {
        Start { mode: DetectorMode, device: String },
        Stop { released_note: Option<u8> },
        CaptureWait(Duration),
    }

    struct FakeRuntime {
        actions: Arc<Mutex<Vec<FakeAction>>>,
        provider_state: Arc<Mutex<FakeProviderState>>,
        capture_entered: Option<Sender<()>>,
        fail_first_start: bool,
        active_note: Option<u8>,
    }

    #[derive(Default)]
    struct FakeProviderState {
        devices: Vec<AudioInputDevice>,
        fail_starts: HashMap<String, usize>,
    }

    impl FakeRuntime {
        fn new(actions: Arc<Mutex<Vec<FakeAction>>>) -> Self {
            Self {
                actions,
                provider_state: Arc::new(Mutex::new(FakeProviderState {
                    devices: vec![
                        AudioInputDevice {
                            id: "pipewire".to_owned(),
                            label: "PipeWire default".to_owned(),
                        },
                        AudioInputDevice {
                            id: "hw:2,0".to_owned(),
                            label: "Mock capture 2".to_owned(),
                        },
                        AudioInputDevice {
                            id: "hw:9,0".to_owned(),
                            label: "Mock capture 9".to_owned(),
                        },
                    ],
                    fail_starts: HashMap::new(),
                })),
                capture_entered: None,
                fail_first_start: false,
                active_note: None,
            }
        }

        fn with_devices(self, devices: Vec<AudioInputDevice>) -> Self {
            self.provider_state.lock().unwrap().devices = devices;
            self
        }

        fn fail_device_start(&mut self, device_id: &str, attempts: usize) {
            self.provider_state
                .lock()
                .unwrap()
                .fail_starts
                .insert(device_id.to_owned(), attempts);
        }

        fn with_capture_signal(mut self, capture_entered: Sender<()>) -> Self {
            self.capture_entered = Some(capture_entered);
            self
        }
    }

    impl WorkerRuntime for FakeRuntime {
        fn enumerate_audio_devices(&mut self) -> Result<Vec<AudioInputDevice>, String> {
            Ok(self.provider_state.lock().unwrap().devices.clone())
        }

        fn start(&mut self, settings: &WorkerSettings) -> Result<(), RuntimeFailure> {
            self.actions.lock().unwrap().push(FakeAction::Start {
                mode: settings.mode,
                device: settings.device.clone(),
            });
            if self.fail_first_start {
                self.fail_first_start = false;
                return Err(RuntimeFailure::Other("fake start failed".to_owned()));
            }
            let mut provider_state = self.provider_state.lock().unwrap();
            if let Some(remaining) = provider_state.fail_starts.get_mut(&settings.device) {
                if *remaining > 0 {
                    *remaining -= 1;
                    return Err(RuntimeFailure::audio_device(
                        &settings.device,
                        "mock capture open failed",
                    ));
                }
            }
            Ok(())
        }

        fn stop(&mut self, events: &mut Vec<EngineEvent>) -> Result<(), String> {
            self.actions.lock().unwrap().push(FakeAction::Stop {
                released_note: self.active_note,
            });
            if let Some(note) = self.active_note.take() {
                events.push(EngineEvent::NoteOff { midi_note: note });
            }
            Ok(())
        }

        fn capture_step(
            &mut self,
            max_wait: Duration,
            events: &mut Vec<EngineEvent>,
        ) -> Result<(), RuntimeFailure> {
            self.actions
                .lock()
                .unwrap()
                .push(FakeAction::CaptureWait(max_wait));
            if let Some(capture_entered) = self.capture_entered.take() {
                let _ = capture_entered.send(());
            }
            if self.active_note.is_none() {
                self.active_note = Some(60);
                events.push(EngineEvent::NoteOn { midi_note: 60 });
            }
            thread::sleep(max_wait);
            Ok(())
        }
    }

    struct FakeMidi {
        writes: Vec<(bool, u8)>,
    }

    impl MidiPort for FakeMidi {
        fn note_on(&mut self, note: u8) -> Result<()> {
            self.writes.push((true, note));
            Ok(())
        }

        fn note_off(&mut self, note: u8) -> Result<()> {
            self.writes.push((false, note));
            Ok(())
        }
    }

    fn spawn_fake(runtime: FakeRuntime) -> PssEngine {
        let args = default_run_args();
        PssEngine::spawn_with_runtime(move || runtime, WorkerSettings::from_args(&args)).unwrap()
    }

    fn spawn_fake_with_settings(runtime: FakeRuntime, settings: WorkerSettings) -> PssEngine {
        PssEngine::spawn_with_runtime(move || runtime, settings).unwrap()
    }

    fn mock_device(id: &str, label: &str) -> AudioInputDevice {
        AudioInputDevice {
            id: id.to_owned(),
            label: label.to_owned(),
        }
    }

    fn recv_until(
        engine: &PssEngine,
        mut predicate: impl FnMut(&EngineEvent) -> bool,
    ) -> EngineEvent {
        loop {
            let event = engine
                .event_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("expected engine event before timeout");
            if predicate(&event) {
                return event;
            }
        }
    }

    #[test]
    fn enumeration_prefers_a_saved_device_reported_by_the_provider() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let runtime = FakeRuntime::new(Arc::clone(&actions)).with_devices(vec![
            mock_device("hw:1,0", "Built-in capture"),
            mock_device("hw:2,0", "USB capture"),
        ]);
        let args = default_run_args();
        let settings = WorkerSettings::from_args_and_preference(&args, Some("hw:2,0".to_owned()));
        let mut engine = spawn_fake_with_settings(runtime, settings);

        engine.send(EngineCommand::EnumerateAudioDevices).unwrap();
        assert_eq!(
            recv_until(&engine, |event| matches!(
                event,
                EngineEvent::AudioDevicesEnumerated { .. }
            )),
            EngineEvent::AudioDevicesEnumerated {
                devices: vec![
                    mock_device("hw:1,0", "Built-in capture"),
                    mock_device("hw:2,0", "USB capture"),
                ],
                selected_device: "hw:2,0".to_owned(),
            }
        );
        engine.shutdown().unwrap();
    }

    #[test]
    fn unavailable_saved_device_falls_back_to_pipewire_without_fabricating_entry() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let enumerated = vec![mock_device("hw:1,0", "Built-in capture")];
        let runtime = FakeRuntime::new(Arc::clone(&actions)).with_devices(enumerated.clone());
        let args = default_run_args();
        let settings =
            WorkerSettings::from_args_and_preference(&args, Some("hw:missing,0".to_owned()));
        let mut engine = spawn_fake_with_settings(runtime, settings);

        engine.send(EngineCommand::EnumerateAudioDevices).unwrap();
        assert_eq!(
            recv_until(&engine, |event| matches!(
                event,
                EngineEvent::AudioDevicesEnumerated { .. }
            )),
            EngineEvent::AudioDevicesEnumerated {
                devices: enumerated,
                selected_device: "pipewire".to_owned(),
            }
        );
        engine.shutdown().unwrap();
    }

    #[test]
    fn unavailable_selection_reports_error_and_failed_open_can_be_retried() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = FakeRuntime::new(Arc::clone(&actions))
            .with_devices(vec![mock_device("hw:1,0", "Built-in capture")]);
        runtime.fail_device_start("hw:gone,0", 1);
        let provider_state = Arc::clone(&runtime.provider_state);
        let mut engine = spawn_fake(runtime);

        engine
            .send(EngineCommand::SetAudioDevice {
                device: "hw:gone,0".to_owned(),
            })
            .unwrap();
        assert!(matches!(
            recv_until(&engine, |event| matches!(
                event,
                EngineEvent::AudioDeviceUnavailable { .. }
            )),
            EngineEvent::AudioDeviceUnavailable { device_id, .. } if device_id == "hw:gone,0"
        ));

        provider_state
            .lock()
            .unwrap()
            .devices
            .push(mock_device("hw:gone,0", "Reconnected capture"));
        engine.send(EngineCommand::RetryAudioDevice).unwrap();
        assert_eq!(
            recv_until(&engine, |event| matches!(
                event,
                EngineEvent::AudioDeviceSelected { .. }
            )),
            EngineEvent::AudioDeviceSelected {
                device_id: "hw:gone,0".to_owned(),
            }
        );
        assert!(matches!(
            recv_until(&engine, |event| matches!(
                event,
                EngineEvent::AudioDeviceUnavailable { .. }
            )),
            EngineEvent::AudioDeviceUnavailable { device_id, message }
                if device_id == "hw:gone,0" && message == "mock capture open failed"
        ));
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::StateChanged(EngineState::Stopped)),
            EngineEvent::StateChanged(EngineState::Stopped)
        );
        assert!(matches!(engine.try_recv_event(), Err(TryRecvError::Empty)));

        engine.send(EngineCommand::RetryAudioDevice).unwrap();
        assert_eq!(
            recv_until(&engine, |event| matches!(
                event,
                EngineEvent::AudioDeviceOpened { .. }
            )),
            EngineEvent::AudioDeviceOpened {
                device_id: "hw:gone,0".to_owned(),
            }
        );
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::StateChanged(EngineState::Running)),
            EngineEvent::StateChanged(EngineState::Running)
        );
        engine.shutdown().unwrap();
    }

    #[test]
    fn start_stop_and_shutdown_drive_the_runtime_lifecycle() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let mut engine = spawn_fake(FakeRuntime::new(Arc::clone(&actions)));

        engine.send(EngineCommand::Start).unwrap();
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::StateChanged(EngineState::Running)),
            EngineEvent::StateChanged(EngineState::Running)
        );
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::NoteOn { midi_note: 60 }),
            EngineEvent::NoteOn { midi_note: 60 }
        );
        engine.send(EngineCommand::Stop).unwrap();
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::NoteOff { midi_note: 60 }),
            EngineEvent::NoteOff { midi_note: 60 }
        );
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::StateChanged(EngineState::Stopped)),
            EngineEvent::StateChanged(EngineState::Stopped)
        );

        engine.send(EngineCommand::Start).unwrap();
        recv_until(&engine, |event| {
            *event == EngineEvent::StateChanged(EngineState::Running)
        });
        let capture_note = recv_until(&engine, |event| {
            *event == EngineEvent::NoteOn { midi_note: 60 }
        });
        assert_eq!(capture_note, EngineEvent::NoteOn { midi_note: 60 });

        engine.shutdown().unwrap();
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::NoteOff { midi_note: 60 }),
            EngineEvent::NoteOff { midi_note: 60 }
        );
        assert_eq!(
            recv_until(&engine, |event| *event == EngineEvent::WorkerStopped),
            EngineEvent::WorkerStopped
        );
        assert_eq!(
            *actions.lock().unwrap(),
            vec![
                FakeAction::Start {
                    mode: DetectorMode::Yin,
                    device: "pipewire".to_owned(),
                },
                FakeAction::CaptureWait(CAPTURE_POLL_INTERVAL),
                FakeAction::Stop {
                    released_note: Some(60),
                },
                FakeAction::Start {
                    mode: DetectorMode::Yin,
                    device: "pipewire".to_owned(),
                },
                FakeAction::CaptureWait(CAPTURE_POLL_INTERVAL),
                FakeAction::Stop {
                    released_note: Some(60),
                },
            ]
        );
    }

    #[test]
    fn mode_and_device_changes_release_notes_and_restart_without_worker_restart() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let (capture_entered_tx, capture_entered_rx) = mpsc::channel();
        let runtime =
            FakeRuntime::new(Arc::clone(&actions)).with_capture_signal(capture_entered_tx);
        let mut engine = spawn_fake(runtime);

        engine.send(EngineCommand::Start).unwrap();
        recv_until(&engine, |event| {
            *event == EngineEvent::StateChanged(EngineState::Running)
        });
        capture_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::NoteOn { midi_note: 60 }),
            EngineEvent::NoteOn { midi_note: 60 }
        );

        engine
            .send(EngineCommand::SetDetectorMode(DetectorMode::Compare))
            .unwrap();
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::NoteOff { midi_note: 60 }),
            EngineEvent::NoteOff { midi_note: 60 }
        );
        recv_until(&engine, |event| {
            *event == EngineEvent::StateChanged(EngineState::Running)
        });
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::NoteOn { midi_note: 60 }),
            EngineEvent::NoteOn { midi_note: 60 }
        );

        engine
            .send(EngineCommand::SetAudioDevice {
                device: "hw:2,0".to_owned(),
            })
            .unwrap();
        recv_until(&engine, |event| {
            *event == EngineEvent::StateChanged(EngineState::Running)
        });
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::NoteOn { midi_note: 60 }),
            EngineEvent::NoteOn { midi_note: 60 }
        );
        engine.shutdown().unwrap();

        let actions = actions.lock().unwrap();
        let starts: Vec<_> = actions
            .iter()
            .filter_map(|action| match action {
                FakeAction::Start { mode, device } => Some((*mode, device.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            starts,
            vec![
                (DetectorMode::Yin, "pipewire"),
                (DetectorMode::Compare, "pipewire"),
                (DetectorMode::Compare, "hw:2,0"),
            ]
        );
        let stop_notes: Vec<_> = actions
            .iter()
            .filter_map(|action| match action {
                FakeAction::Stop { released_note } => Some(*released_note),
                _ => None,
            })
            .collect();
        assert_eq!(stop_notes, vec![Some(60), Some(60), Some(60)]);
    }

    #[test]
    fn mode_transition_services_commands_after_a_bounded_capture_wait() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let (capture_entered_tx, capture_entered_rx) = mpsc::channel();
        let runtime =
            FakeRuntime::new(Arc::clone(&actions)).with_capture_signal(capture_entered_tx);
        let mut engine = spawn_fake(runtime);
        engine.send(EngineCommand::Start).unwrap();
        recv_until(&engine, |event| {
            *event == EngineEvent::StateChanged(EngineState::Running)
        });
        capture_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        let sent_at = Instant::now();
        engine
            .send(EngineCommand::SetAudioDevice {
                device: "hw:9,0".to_owned(),
            })
            .unwrap();
        recv_until(&engine, |event| {
            *event == EngineEvent::StateChanged(EngineState::Running)
        });
        assert!(
            sent_at.elapsed() < Duration::from_millis(250),
            "command was not serviced within the bounded capture interval"
        );
        engine.shutdown().unwrap();

        assert!(actions
            .lock()
            .unwrap()
            .contains(&FakeAction::CaptureWait(CAPTURE_POLL_INTERVAL)));
    }

    #[test]
    fn start_errors_are_reported_and_the_worker_remains_usable() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = FakeRuntime::new(Arc::clone(&actions));
        runtime.fail_first_start = true;
        let mut engine = spawn_fake(runtime);

        engine.send(EngineCommand::Start).unwrap();
        assert_eq!(
            recv_until(&engine, |event| matches!(event, EngineEvent::Error { .. })),
            EngineEvent::Error {
                message: "fake start failed".to_owned(),
            }
        );
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::StateChanged(EngineState::Stopped)),
            EngineEvent::StateChanged(EngineState::Stopped)
        );

        engine.send(EngineCommand::Start).unwrap();
        recv_until(&engine, |event| {
            *event == EngineEvent::StateChanged(EngineState::Running)
        });
        engine.shutdown().unwrap();
        assert_eq!(
            actions
                .lock()
                .unwrap()
                .iter()
                .filter(|action| matches!(action, FakeAction::Start { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn detector_note_replacement_preserves_note_off_before_note_on() {
        let mut midi = FakeMidi { writes: Vec::new() };
        let mut active_note = Some(48);
        let mut events = Vec::new();
        apply_note_decisions(
            &mut midi,
            &mut active_note,
            &[
                NoteDecision::NoteOff { note: 48 },
                NoteDecision::NoteOn { note: 49 },
            ],
            &mut events,
        )
        .unwrap();

        assert_eq!(midi.writes, vec![(false, 48), (true, 49)]);
        assert_eq!(active_note, Some(49));
        assert_eq!(
            events,
            vec![
                EngineEvent::NoteOff { midi_note: 48 },
                EngineEvent::NoteOn { midi_note: 49 },
            ]
        );
    }

    #[test]
    fn detector_outcome_applies_ordered_notes_and_publishes_detection_data() {
        let outcome = DetectorOutcome {
            onset_detected: true,
            selected_note: Some(49),
            note_decisions: vec![
                NoteDecision::NoteOff { note: 48 },
                NoteDecision::NoteOn { note: 49 },
            ],
            yin: Some(YinResult {
                midi_pitch: Some(49.0),
                note: Some(49),
                cents: Some(0.0),
                decision: None,
            }),
            spectral: None,
            onset_to_note_latency: Some(Duration::from_millis(15)),
            rms_dbfs: -20.0,
            peak: 0.4,
        };
        let mut midi = FakeMidi { writes: Vec::new() };
        let mut active_note = Some(48);
        let mut events = Vec::new();
        let mut publisher = OutcomeEventPublisher::default();
        publisher
            .publish_at(
                &outcome,
                Some(&mut midi),
                &mut active_note,
                &mut events,
                Instant::now(),
            )
            .unwrap();

        assert_eq!(midi.writes, vec![(false, 48), (true, 49)]);
        assert_eq!(active_note, Some(49));
        assert_eq!(
            events,
            vec![
                EngineEvent::Onset,
                EngineEvent::NoteOff { midi_note: 48 },
                EngineEvent::NoteOn { midi_note: 49 },
                EngineEvent::Detection {
                    selected_note: Some(49),
                    yin: outcome.yin,
                    spectral: None,
                    onset_to_note_latency: Some(Duration::from_millis(15)),
                },
                EngineEvent::AudioLevel {
                    rms_dbfs: -20.0,
                    peak_dbfs: peak_to_dbfs(0.4),
                },
            ]
        );
    }

    #[test]
    fn idle_frames_throttle_levels_without_detection_flood_and_note_events_stay_immediate() {
        let mut publisher = OutcomeEventPublisher::default();
        let mut events = Vec::new();
        let mut active_note = None;
        let mut midi = FakeMidi { writes: Vec::new() };
        let start = Instant::now();
        let idle_outcome = DetectorOutcome {
            onset_detected: false,
            selected_note: None,
            note_decisions: Vec::new(),
            yin: Some(YinResult {
                midi_pitch: Some(60.2),
                note: Some(60),
                cents: Some(2.0),
                decision: None,
            }),
            spectral: None,
            onset_to_note_latency: None,
            rms_dbfs: -30.0,
            peak: 0.25,
        };

        for frame in 0..100 {
            publisher
                .publish_at(
                    &idle_outcome,
                    None::<&mut FakeMidi>,
                    &mut active_note,
                    &mut events,
                    start + Duration::from_millis(frame * 2),
                )
                .unwrap();
        }

        assert!(!events
            .iter()
            .any(|event| matches!(event, EngineEvent::Detection { .. })));
        let levels: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::AudioLevel {
                    rms_dbfs,
                    peak_dbfs,
                } => Some((*rms_dbfs, *peak_dbfs)),
                _ => None,
            })
            .collect();
        assert_eq!(levels.len(), 5);
        assert!(levels
            .iter()
            .all(|(rms, peak)| *rms == -30.0 && (*peak - peak_to_dbfs(0.25)).abs() < 1e-6));

        let event_count_before_note = events.len();
        let note_outcome = DetectorOutcome {
            onset_detected: false,
            selected_note: Some(60),
            note_decisions: vec![NoteDecision::NoteOn { note: 60 }],
            yin: Some(YinResult {
                midi_pitch: Some(60.0),
                note: Some(60),
                cents: Some(0.0),
                decision: None,
            }),
            spectral: None,
            onset_to_note_latency: None,
            rms_dbfs: -18.0,
            peak: 0.5,
        };
        publisher
            .publish_at(
                &note_outcome,
                Some(&mut midi),
                &mut active_note,
                &mut events,
                start + Duration::from_millis(199),
            )
            .unwrap();

        let note_events = &events[event_count_before_note..];
        assert!(note_events
            .iter()
            .any(|event| *event == EngineEvent::NoteOn { midi_note: 60 }));
        assert!(note_events
            .iter()
            .any(|event| matches!(event, EngineEvent::Detection { .. })));
        assert!(!note_events
            .iter()
            .any(|event| matches!(event, EngineEvent::AudioLevel { .. })));
        assert_eq!(midi.writes, vec![(true, 60)]);
    }

    #[test]
    fn completed_yin_decisions_and_spectral_classifications_publish_detection_updates() {
        let mut publisher = OutcomeEventPublisher::default();
        let mut events = Vec::new();
        let mut active_note = None;
        let base = Instant::now();
        let mut outcome = DetectorOutcome {
            onset_detected: false,
            selected_note: None,
            note_decisions: Vec::new(),
            yin: Some(YinResult {
                midi_pitch: Some(48.0),
                note: Some(48),
                cents: Some(0.0),
                decision: Some(crate::engine::detector::YinDecision {
                    candidate_note: Some(48),
                    accepted: true,
                    vote_count: 6,
                    total_votes: 8,
                    vote_ratio: Some(0.75),
                }),
            }),
            spectral: None,
            onset_to_note_latency: None,
            rms_dbfs: -20.0,
            peak: 0.5,
        };

        publisher
            .publish_at(
                &outcome,
                None::<&mut FakeMidi>,
                &mut active_note,
                &mut events,
                base,
            )
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, EngineEvent::Detection { .. }))
                .count(),
            1
        );

        outcome.yin.as_mut().unwrap().decision = None;
        outcome.spectral = Some(crate::engine::detector::SpectralResult {
            note: 48,
            confidence: 0.9,
            second_score: 0.2,
            margin: 0.7,
            accepted: true,
            selected_note: Some(48),
            ranked_matches: Vec::new(),
        });
        publisher
            .publish_at(
                &outcome,
                None::<&mut FakeMidi>,
                &mut active_note,
                &mut events,
                base + AUDIO_LEVEL_INTERVAL,
            )
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, EngineEvent::Detection { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn peak_level_is_reported_in_dbfs_with_a_silence_floor() {
        assert!((peak_to_dbfs(0.5) - -6.0206).abs() < 0.001);
        assert_eq!(peak_to_dbfs(0.0), -120.0);
    }

    #[test]
    fn active_note_release_is_sent_before_the_released_state_is_cleared() {
        let mut midi = FakeMidi { writes: Vec::new() };
        let mut active_note = Some(60);
        let mut events = Vec::new();
        release_active_note(&mut midi, &mut active_note, &mut events).unwrap();

        assert_eq!(midi.writes, vec![(false, 60)]);
        assert_eq!(active_note, None);
        assert_eq!(events, vec![EngineEvent::NoteOff { midi_note: 60 }]);
    }
}
