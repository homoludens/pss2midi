//! GPUI-independent command and event boundary for the sound engine.

use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, SendError, Sender, TryRecvError},
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result};
use async_channel::{Receiver as AsyncReceiver, Sender as AsyncSender};
use clap::Parser;

use crate::engine::{
    audio_device::{
        choose_initial_device, is_selectable_device, AlsaAudioDeviceProvider, AudioDeviceProvider,
        AudioDeviceUnavailable, AudioInputDevice, OpenedAudioCapture, PIPEWIRE_DEFAULT_DEVICE_ID,
    },
    calibration::{
        CalibrationNoteCompletion, CalibrationProgress, CalibrationResult, CalibrationSession,
        CalibrationUpdate, DEFAULT_SAMPLES_PER_NOTE,
    },
    config::{default_template_path, Cli, DetectorMode, RunArgs},
    detector::{Detector, DetectorOutcome, NoteDecision, SpectralResult, YinResult},
    midi::{Midi, OUTPUT_NAME as MIDI_OUTPUT_NAME},
    note::rms_db,
    persistence::TemplatePersistenceTask,
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
    SetTemplatePath { path: PathBuf },
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
    DetectorModeChanged(DetectorMode),
    MidiOutputOpened {
        name: String,
    },
    MidiOutputClosed,
    MidiOutputUnavailable {
        message: String,
    },
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
    CalibrationProgress(CalibrationProgress),
    CalibrationNoteCompleted(CalibrationNoteCompletion),
    CalibrationCompleted {
        note_count: usize,
        sample_count: usize,
        template_path: PathBuf,
    },
    CalibrationCancelled,
    CalibrationError {
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
    event_rx: AsyncReceiver<EngineEvent>,
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

    /// Begin calibration with the default five accepted samples per note.
    pub fn begin_calibration(&self) -> Result<(), SendError<EngineCommand>> {
        self.begin_calibration_with_samples(DEFAULT_SAMPLES_PER_NOTE)
    }

    pub fn begin_calibration_with_samples(
        &self,
        samples_per_note: usize,
    ) -> Result<(), SendError<EngineCommand>> {
        self.send(EngineCommand::BeginCalibration { samples_per_note })
    }

    /// Select the template destination used by subsequent calibration runs.
    pub fn set_template_path(&self, path: PathBuf) -> Result<(), SendError<EngineCommand>> {
        self.send(EngineCommand::SetTemplatePath { path })
    }

    /// Wait for the next high-level worker event.
    pub fn recv_event(&self) -> Result<EngineEvent, RecvError> {
        self.event_rx.recv_blocking().map_err(|_| RecvError)
    }

    /// Check for a worker event without blocking.
    pub fn try_recv_event(&self) -> Result<EngineEvent, TryRecvError> {
        self.event_rx.try_recv().map_err(|error| match error {
            async_channel::TryRecvError::Empty => TryRecvError::Empty,
            async_channel::TryRecvError::Closed => TryRecvError::Disconnected,
        })
    }

    /// Clone the event receiver for asynchronous consumers such as a UI task.
    /// The existing blocking receive methods remain available to non-UI clients.
    pub fn event_receiver(&self) -> AsyncReceiver<EngineEvent> {
        self.event_rx.clone()
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
        let (event_tx, event_rx) = async_channel::unbounded();
        let worker = thread::Builder::new()
            .name("pss2midi-engine".to_owned())
            .spawn(move || {
                let runtime = runtime_factory();
                run_worker(command_rx, EngineEventSender(event_tx), runtime, settings);
            })?;

        Ok(Self {
            command_tx,
            event_rx,
            worker: Some(worker),
        })
    }
}

/// Synchronous, nonblocking publication into the unbounded async event queue.
/// `send` only enqueues; it never waits for the UI or another event consumer.
#[derive(Clone)]
struct EngineEventSender(AsyncSender<EngineEvent>);

impl EngineEventSender {
    fn send(&self, event: EngineEvent) -> Result<(), ()> {
        self.0.try_send(event).map_err(|_| ())
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
    template_path: PathBuf,
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
            template_path: default_template_path(),
            preferred_device,
            pending_device: None,
            device_selection_resolved: false,
        }
    }
}

#[derive(Debug)]
enum RuntimeFailure {
    AudioDevice(AudioDeviceUnavailable),
    MidiOutput(String),
    Calibration(String),
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
    fn begin_calibration(
        &mut self,
        _settings: &WorkerSettings,
        _samples_per_note: usize,
        _events: &mut Vec<EngineEvent>,
    ) -> Result<(), RuntimeFailure> {
        Err(RuntimeFailure::Other(
            "Calibration is not supported by this runtime".to_owned(),
        ))
    }
    fn retry_calibration_sample(&mut self, _events: &mut Vec<EngineEvent>) -> Result<(), String> {
        Err("No calibration session is active".to_owned())
    }
    /// Cancel an active session and return whether asynchronous save work still
    /// needs polling by the worker.
    fn cancel_calibration(&mut self, _events: &mut Vec<EngineEvent>) -> bool {
        false
    }
    fn abort_calibration(&mut self) {}
    fn calibration_active(&self) -> bool {
        false
    }
    fn capture_step(
        &mut self,
        max_wait: Duration,
        events: &mut Vec<EngineEvent>,
    ) -> Result<CaptureStepStatus, RuntimeFailure>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CaptureStepStatus {
    calibration_active: bool,
}

fn run_worker<R>(
    command_rx: Receiver<EngineCommand>,
    event_tx: EngineEventSender,
    mut runtime: R,
    mut settings: WorkerSettings,
) where
    R: WorkerRuntime,
{
    let _ = event_tx.send(EngineEvent::WorkerStarted);
    let mut running = false;
    let mut calibrating = false;

    loop {
        let command = if running || calibrating {
            match command_rx.recv_timeout(COMMAND_POLL_INTERVAL) {
                Ok(command) => command,
                Err(RecvTimeoutError::Timeout) => {
                    let mut events = Vec::new();
                    match runtime.capture_step(CAPTURE_POLL_INTERVAL, &mut events) {
                        Ok(status) => {
                            // The status is also reported while normal capture
                            // is running so asynchronous template persistence
                            // can finish without blocking audio commands.
                            calibrating = status.calibration_active;
                            publish_events(&event_tx, events);
                        }
                        Err(error) => {
                            publish_events(&event_tx, events);
                            match error {
                                RuntimeFailure::Calibration(message) => {
                                    let _ =
                                        event_tx.send(EngineEvent::CalibrationError { message });
                                    runtime.abort_calibration();
                                    calibrating = runtime.calibration_active();
                                }
                                error => publish_runtime_failure(&event_tx, error),
                            }
                            if running {
                                stop_runtime(&mut runtime, &event_tx);
                                running = false;
                                let _ =
                                    event_tx.send(EngineEvent::StateChanged(EngineState::Stopped));
                            }
                        }
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
                if calibrating {
                    let mut events = Vec::new();
                    runtime.cancel_calibration(&mut events);
                    calibrating = runtime.calibration_active();
                    publish_events(&event_tx, events);
                }
                resolve_initial_device(&mut runtime, &event_tx, &mut settings);
                running = start_runtime(&mut runtime, &settings, &event_tx);
            }
            EngineCommand::Start => {}
            EngineCommand::Stop if running => {
                stop_runtime(&mut runtime, &event_tx);
                running = false;
                calibrating = runtime.calibration_active();
                let _ = event_tx.send(EngineEvent::StateChanged(EngineState::Stopped));
            }
            EngineCommand::Stop if calibrating => {
                let mut events = Vec::new();
                runtime.cancel_calibration(&mut events);
                calibrating = runtime.calibration_active();
                publish_events(&event_tx, events);
            }
            EngineCommand::Stop => {}
            EngineCommand::SetDetectorMode(mode) if settings.mode != mode => {
                if calibrating {
                    let mut events = Vec::new();
                    runtime.cancel_calibration(&mut events);
                    calibrating = runtime.calibration_active();
                    publish_events(&event_tx, events);
                }
                let mut next_settings = settings.clone();
                next_settings.mode = mode;
                running = reconfigure_runtime(
                    &mut runtime,
                    &event_tx,
                    &mut settings,
                    next_settings,
                    running,
                );
                let _ = event_tx.send(EngineEvent::DetectorModeChanged(settings.mode));
            }
            EngineCommand::SetDetectorMode(_) => {}
            EngineCommand::SetAudioDevice { device } if settings.device != device => {
                if calibrating {
                    let mut events = Vec::new();
                    runtime.cancel_calibration(&mut events);
                    calibrating = runtime.calibration_active();
                    publish_events(&event_tx, events);
                }
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
            EngineCommand::SetTemplatePath { path } => settings.template_path = path,
            EngineCommand::BeginCalibration { samples_per_note } => {
                if running {
                    stop_runtime(&mut runtime, &event_tx);
                    running = false;
                    let _ = event_tx.send(EngineEvent::StateChanged(EngineState::Stopped));
                }
                if calibrating {
                    let mut events = Vec::new();
                    runtime.cancel_calibration(&mut events);
                    calibrating = runtime.calibration_active();
                    publish_events(&event_tx, events);
                }
                if samples_per_note == 0 {
                    let _ = event_tx.send(EngineEvent::CalibrationError {
                        message: "samples per note must be greater than zero".to_owned(),
                    });
                    continue;
                }
                resolve_initial_device(&mut runtime, &event_tx, &mut settings);
                let mut events = Vec::new();
                match runtime.begin_calibration(&settings, samples_per_note, &mut events) {
                    Ok(()) => {
                        calibrating = runtime.calibration_active();
                        publish_events(&event_tx, events);
                    }
                    Err(error) => {
                        publish_events(&event_tx, events);
                        let message = runtime_failure_message(&error);
                        let _ = event_tx.send(EngineEvent::CalibrationError { message });
                        publish_runtime_failure(&event_tx, error);
                    }
                }
            }
            EngineCommand::RetryCalibrationSample if calibrating => {
                let mut events = Vec::new();
                match runtime.retry_calibration_sample(&mut events) {
                    Ok(()) => publish_events(&event_tx, events),
                    Err(message) => {
                        let _ = event_tx.send(EngineEvent::CalibrationError { message });
                    }
                }
            }
            EngineCommand::RetryCalibrationSample => {
                let _ = event_tx.send(EngineEvent::CalibrationError {
                    message: "No calibration session is active".to_owned(),
                });
            }
            EngineCommand::CancelCalibration if calibrating => {
                let mut events = Vec::new();
                runtime.cancel_calibration(&mut events);
                calibrating = runtime.calibration_active();
                publish_events(&event_tx, events);
            }
            EngineCommand::CancelCalibration => {}
            EngineCommand::ReloadTemplates { .. } => {
                let _ = event_tx.send(EngineEvent::Error {
                    message: "Template reload is not available yet".to_owned(),
                });
            }
            EngineCommand::Shutdown => break,
        }
    }

    let _ = event_tx.send(EngineEvent::StateChanged(EngineState::ShuttingDown));
    if running {
        stop_runtime(&mut runtime, &event_tx);
        let _ = event_tx.send(EngineEvent::StateChanged(EngineState::Stopped));
    }
    if calibrating {
        let mut events = Vec::new();
        runtime.cancel_calibration(&mut events);
        publish_events(&event_tx, events);
    }
    drop(runtime);
    let _ = event_tx.send(EngineEvent::WorkerStopped);
}

fn start_runtime<R: WorkerRuntime>(
    runtime: &mut R,
    settings: &WorkerSettings,
    event_tx: &EngineEventSender,
) -> bool {
    match runtime.start(settings) {
        Ok(()) => {
            let _ = event_tx.send(EngineEvent::MidiOutputOpened {
                name: MIDI_OUTPUT_NAME.to_owned(),
            });
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
    event_tx: &EngineEventSender,
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
    event_tx: &EngineEventSender,
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
    event_tx: &EngineEventSender,
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
    event_tx: &EngineEventSender,
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

fn publish_runtime_failure(event_tx: &EngineEventSender, error: RuntimeFailure) {
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
        RuntimeFailure::MidiOutput(message) => {
            let _ = event_tx.send(EngineEvent::MidiOutputUnavailable { message });
        }
        RuntimeFailure::Calibration(message) => {
            let _ = event_tx.send(EngineEvent::CalibrationError { message });
        }
    }
}

fn runtime_failure_message(error: &RuntimeFailure) -> String {
    match error {
        RuntimeFailure::AudioDevice(error) => {
            format!("{}: {}", error.device_id, error.message)
        }
        RuntimeFailure::MidiOutput(message)
        | RuntimeFailure::Calibration(message)
        | RuntimeFailure::Other(message) => message.clone(),
    }
}

fn stop_runtime<R: WorkerRuntime>(runtime: &mut R, event_tx: &EngineEventSender) -> bool {
    let mut events = Vec::new();
    let result = runtime.stop(&mut events);
    publish_events(event_tx, events);
    let _ = event_tx.send(EngineEvent::MidiOutputClosed);
    if let Err(message) = result {
        let _ = event_tx.send(EngineEvent::Error { message });
        false
    } else {
        true
    }
}

fn reconfigure_runtime<R: WorkerRuntime>(
    runtime: &mut R,
    event_tx: &EngineEventSender,
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

fn publish_events(event_tx: &EngineEventSender, events: Vec<EngineEvent>) {
    for event in events {
        let _ = event_tx.send(event);
    }
}

struct ProductionRuntime {
    args: RunArgs,
    audio_device_provider: Box<dyn AudioDeviceProvider>,
    resources: WorkerResources,
    outcome_publisher: OutcomeEventPublisher,
    calibration: Option<CalibrationSession>,
    pending_template_saves: Vec<PendingTemplateSave>,
    current_template_path: PathBuf,
}

struct PendingTemplateSave {
    task: TemplatePersistenceTask,
    note_count: usize,
    sample_count: usize,
    cancelled: bool,
}

impl ProductionRuntime {
    fn new(args: RunArgs, audio_device_provider: Box<dyn AudioDeviceProvider>) -> Self {
        Self {
            args,
            audio_device_provider,
            resources: WorkerResources::default(),
            outcome_publisher: OutcomeEventPublisher::default(),
            calibration: None,
            pending_template_saves: Vec::new(),
            current_template_path: default_template_path(),
        }
    }

    fn clear_calibration_capture(&mut self) {
        self.calibration.take();
        self.resources.capture.take();
        self.resources.active_device = None;
        self.resources.frame.clear();
    }

    fn poll_template_saves(&mut self, events: &mut Vec<EngineEvent>) {
        let mut index = 0;
        while index < self.pending_template_saves.len() {
            let result = self.pending_template_saves[index].task.try_result();
            let Some(result) = result else {
                index += 1;
                continue;
            };

            let pending = self.pending_template_saves.remove(index);
            if pending.cancelled {
                continue;
            }
            match result {
                Ok(template_path) => events.push(EngineEvent::CalibrationCompleted {
                    note_count: pending.note_count,
                    sample_count: pending.sample_count,
                    template_path,
                }),
                Err(message) => events.push(EngineEvent::CalibrationError { message }),
            }
        }
    }

    fn capture_calibration_step(
        &mut self,
        max_wait: Duration,
        events: &mut Vec<EngineEvent>,
    ) -> Result<(), RuntimeFailure> {
        let frame_start = {
            let resources = &mut self.resources;
            let Some(capture) = resources.capture.as_mut() else {
                return Err(RuntimeFailure::Calibration(
                    "Calibration capture stream is not open".to_owned(),
                ));
            };
            match capture.try_next_frame(&mut resources.frame, max_wait) {
                Ok(frame_start) => frame_start,
                Err(error) => {
                    let device_id = resources
                        .active_device
                        .as_deref()
                        .unwrap_or("unknown")
                        .to_owned();
                    events.push(EngineEvent::AudioDeviceUnavailable {
                        device_id,
                        message: format!("{error:#}"),
                    });
                    return Err(RuntimeFailure::Calibration(format!(
                        "Audio capture failed during calibration: {error:#}"
                    )));
                }
            }
        };
        let Some(frame_start) = frame_start else {
            return Ok(());
        };

        self.outcome_publisher
            .publish_calibration_frame(&self.resources.frame, events);
        let updates = self
            .calibration
            .as_mut()
            .context("Calibration session is not active")
            .and_then(|session| session.process_frame(frame_start, &self.resources.frame))
            .map_err(|error| RuntimeFailure::Calibration(format!("{error:#}")))?;

        for update in updates {
            if let Some(result) = append_calibration_update(update, events) {
                let note_count = result.note_count;
                let sample_count = result.sample_count;
                let template_path = self.current_template_path.clone();
                self.clear_calibration_capture();
                match TemplatePersistenceTask::spawn(result, template_path.clone()) {
                    Ok(task) => self.pending_template_saves.push(PendingTemplateSave {
                        task,
                        note_count,
                        sample_count,
                        cancelled: false,
                    }),
                    Err(error) => events.push(EngineEvent::CalibrationError {
                        message: format!("Could not start template persistence task: {error}"),
                    }),
                }
            }
        }
        Ok(())
    }

    fn calibration_active(&self) -> bool {
        self.calibration.is_some() || !self.pending_template_saves.is_empty()
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
        let midi = Midi::new().map_err(|error| RuntimeFailure::MidiOutput(format!("{error:#}")))?;
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

    fn begin_calibration(
        &mut self,
        settings: &WorkerSettings,
        samples_per_note: usize,
        events: &mut Vec<EngineEvent>,
    ) -> Result<(), RuntimeFailure> {
        let mut audio = self.args.audio.clone();
        audio.device = settings.device.clone();
        let capture = self
            .audio_device_provider
            .open_capture(&settings.device, &audio)
            .map_err(RuntimeFailure::AudioDevice)?;
        let sample_rate = capture.sample_rate;
        let hop = capture.hop;
        let session = CalibrationSession::new(
            &audio,
            sample_rate,
            hop,
            &self.args.spectral,
            samples_per_note,
        )
        .map_err(|error| RuntimeFailure::Calibration(format!("{error:#}")))?;

        self.resources = WorkerResources {
            capture: Some(capture),
            detector: None,
            midi: None,
            active_note: None,
            frame: vec![0.0; hop],
            active_device: Some(settings.device.clone()),
        };
        self.current_template_path = settings.template_path.clone();
        self.calibration = Some(session);
        self.outcome_publisher = OutcomeEventPublisher::default();
        events.push(EngineEvent::CalibrationProgress(
            self.calibration
                .as_ref()
                .expect("session was just installed")
                .initial_progress(),
        ));
        Ok(())
    }

    fn retry_calibration_sample(&mut self, events: &mut Vec<EngineEvent>) -> Result<(), String> {
        let session = self
            .calibration
            .as_mut()
            .ok_or_else(|| "No calibration sample is awaiting capture".to_owned())?;
        events.push(EngineEvent::CalibrationProgress(
            session.retry_current_sample(),
        ));
        Ok(())
    }

    fn cancel_calibration(&mut self, events: &mut Vec<EngineEvent>) -> bool {
        let had_work = self.calibration_active();
        if self.calibration.is_some() {
            self.clear_calibration_capture();
        }
        for pending in &mut self.pending_template_saves {
            pending.cancelled = true;
        }
        if had_work {
            events.push(EngineEvent::CalibrationCancelled);
        }
        self.calibration_active()
    }

    fn abort_calibration(&mut self) {
        self.clear_calibration_capture();
    }

    fn calibration_active(&self) -> bool {
        ProductionRuntime::calibration_active(self)
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
        self.calibration.take();

        match first_error {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }

    fn capture_step(
        &mut self,
        max_wait: Duration,
        events: &mut Vec<EngineEvent>,
    ) -> Result<CaptureStepStatus, RuntimeFailure> {
        self.poll_template_saves(events);
        if self.calibration.is_some() {
            self.capture_calibration_step(max_wait, events)?;
            self.poll_template_saves(events);
            return Ok(CaptureStepStatus {
                calibration_active: self.calibration_active(),
            });
        }

        if self.resources.detector.is_none() {
            return Ok(CaptureStepStatus {
                calibration_active: self.calibration_active(),
            });
        }

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
            return Ok(CaptureStepStatus {
                calibration_active: self.calibration_active(),
            });
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
            .map_err(|error| RuntimeFailure::Other(format!("{error:#}")))?;
        Ok(CaptureStepStatus {
            calibration_active: self.calibration_active(),
        })
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
        let emit_audio_level = self.should_emit_audio_level_at(now);
        append_detector_outcome_events(outcome, midi, active_note, events, emit_audio_level)
    }

    fn publish_calibration_frame(&mut self, frame: &[f32], events: &mut Vec<EngineEvent>) {
        self.publish_calibration_frame_at(frame, events, std::time::Instant::now());
    }

    fn publish_calibration_frame_at(
        &mut self,
        frame: &[f32],
        events: &mut Vec<EngineEvent>,
        now: std::time::Instant,
    ) {
        if !self.should_emit_audio_level_at(now) {
            return;
        }

        let peak = frame
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        events.push(EngineEvent::AudioLevel {
            rms_dbfs: rms_db(frame),
            peak_dbfs: peak_to_dbfs(peak),
        });
    }

    fn should_emit_audio_level_at(&mut self, now: std::time::Instant) -> bool {
        let emit = self.last_audio_level_at.map_or(true, |last| {
            now.saturating_duration_since(last) >= AUDIO_LEVEL_INTERVAL
        });
        if emit {
            self.last_audio_level_at = Some(now);
        }
        emit
    }
}

fn append_calibration_update(
    update: CalibrationUpdate,
    events: &mut Vec<EngineEvent>,
) -> Option<CalibrationResult> {
    match update {
        CalibrationUpdate::Progress(progress) => {
            events.push(EngineEvent::CalibrationProgress(progress));
            None
        }
        CalibrationUpdate::NoteCompleted(note) => {
            events.push(EngineEvent::CalibrationNoteCompleted(note));
            None
        }
        CalibrationUpdate::Completed(result) => Some(result),
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
    use crate::engine::calibration::CalibrationSampleQuality;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
        time::Instant,
    };

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum FakeAction {
        Start {
            mode: DetectorMode,
            device: String,
        },
        Stop {
            released_note: Option<u8>,
        },
        CaptureWait(Duration),
        BeginCalibration {
            samples_per_note: usize,
            template_path: PathBuf,
        },
        RetryCalibrationSample,
        CancelCalibration,
    }

    struct FakeRuntime {
        actions: Arc<Mutex<Vec<FakeAction>>>,
        provider_state: Arc<Mutex<FakeProviderState>>,
        capture_entered: Option<Sender<()>>,
        fail_first_start: bool,
        fail_midi_start: bool,
        active_note: Option<u8>,
        calibration_active: bool,
        calibration_samples_per_note: usize,
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
                fail_midi_start: false,
                active_note: None,
                calibration_active: false,
                calibration_samples_per_note: DEFAULT_SAMPLES_PER_NOTE,
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

        fn fail_midi_start(&mut self) {
            self.fail_midi_start = true;
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
            if self.fail_midi_start {
                self.fail_midi_start = false;
                return Err(RuntimeFailure::MidiOutput(
                    "fake MIDI output unavailable".to_owned(),
                ));
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

        fn begin_calibration(
            &mut self,
            settings: &WorkerSettings,
            samples_per_note: usize,
            events: &mut Vec<EngineEvent>,
        ) -> Result<(), RuntimeFailure> {
            self.actions
                .lock()
                .unwrap()
                .push(FakeAction::BeginCalibration {
                    samples_per_note,
                    template_path: settings.template_path.clone(),
                });
            self.calibration_active = true;
            self.calibration_samples_per_note = samples_per_note;
            events.push(EngineEvent::CalibrationProgress(CalibrationProgress {
                requested_note: 36,
                sample_index: 1,
                samples_per_note,
                accepted_samples_for_note: 0,
                accepted_samples: 0,
                required_samples: 37 * samples_per_note,
                completed_notes: 0,
                rms_dbfs: None,
                peak: None,
                quality: CalibrationSampleQuality::AwaitingOnset,
            }));
            Ok(())
        }

        fn retry_calibration_sample(
            &mut self,
            events: &mut Vec<EngineEvent>,
        ) -> Result<(), String> {
            if !self.calibration_active {
                return Err("No calibration session is active".to_owned());
            }
            self.actions
                .lock()
                .unwrap()
                .push(FakeAction::RetryCalibrationSample);
            events.push(EngineEvent::CalibrationProgress(CalibrationProgress {
                requested_note: 36,
                sample_index: 1,
                samples_per_note: self.calibration_samples_per_note,
                accepted_samples_for_note: 0,
                accepted_samples: 0,
                required_samples: 37 * self.calibration_samples_per_note,
                completed_notes: 0,
                rms_dbfs: None,
                peak: None,
                quality: CalibrationSampleQuality::AwaitingOnset,
            }));
            Ok(())
        }

        fn cancel_calibration(&mut self, events: &mut Vec<EngineEvent>) -> bool {
            if self.calibration_active {
                self.actions
                    .lock()
                    .unwrap()
                    .push(FakeAction::CancelCalibration);
                self.calibration_active = false;
                events.push(EngineEvent::CalibrationCancelled);
            }
            false
        }

        fn calibration_active(&self) -> bool {
            self.calibration_active
        }

        fn capture_step(
            &mut self,
            max_wait: Duration,
            events: &mut Vec<EngineEvent>,
        ) -> Result<CaptureStepStatus, RuntimeFailure> {
            self.actions
                .lock()
                .unwrap()
                .push(FakeAction::CaptureWait(max_wait));
            if let Some(capture_entered) = self.capture_entered.take() {
                let _ = capture_entered.send(());
            }
            if !self.calibration_active && self.active_note.is_none() {
                self.active_note = Some(60);
                events.push(EngineEvent::NoteOn { midi_note: 60 });
            }
            thread::sleep(max_wait);
            Ok(CaptureStepStatus {
                calibration_active: self.calibration_active,
            })
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
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match engine.try_recv_event() {
                Ok(event) if predicate(&event) => return event,
                Ok(_) | Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => panic!("engine event channel disconnected"),
            }
            assert!(
                Instant::now() < deadline,
                "expected engine event before timeout"
            );
            thread::sleep(Duration::from_millis(1));
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
    fn midi_output_events_report_open_close_and_recoverable_creation_failure() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = FakeRuntime::new(Arc::clone(&actions));
        runtime.fail_midi_start();
        let mut engine = spawn_fake(runtime);

        engine.send(EngineCommand::Start).unwrap();
        assert_eq!(
            recv_until(&engine, |event| matches!(
                event,
                EngineEvent::MidiOutputUnavailable { .. }
            )),
            EngineEvent::MidiOutputUnavailable {
                message: "fake MIDI output unavailable".to_owned(),
            }
        );
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::StateChanged(EngineState::Stopped)),
            EngineEvent::StateChanged(EngineState::Stopped)
        );

        engine.send(EngineCommand::Start).unwrap();
        assert_eq!(
            recv_until(&engine, |event| matches!(
                event,
                EngineEvent::MidiOutputOpened { .. }
            )),
            EngineEvent::MidiOutputOpened {
                name: MIDI_OUTPUT_NAME.to_owned(),
            }
        );
        recv_until(&engine, |event| {
            *event == EngineEvent::StateChanged(EngineState::Running)
        });

        engine.send(EngineCommand::Stop).unwrap();
        assert_eq!(
            recv_until(&engine, |event| *event == EngineEvent::MidiOutputClosed),
            EngineEvent::MidiOutputClosed
        );
        engine.shutdown().unwrap();
    }

    #[test]
    fn calibration_commands_release_midi_and_keep_retry_on_the_requested_sample() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let mut engine = spawn_fake(FakeRuntime::new(Arc::clone(&actions)));
        let template_path = PathBuf::from("/tmp/custom-pss-templates.json");
        engine
            .send(EngineCommand::SetTemplatePath {
                path: template_path.clone(),
            })
            .unwrap();
        engine.send(EngineCommand::Start).unwrap();
        recv_until(&engine, |event| {
            *event == EngineEvent::StateChanged(EngineState::Running)
        });
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::NoteOn { midi_note: 60 }),
            EngineEvent::NoteOn { midi_note: 60 }
        );

        engine
            .send(EngineCommand::BeginCalibration {
                samples_per_note: 3,
            })
            .unwrap();
        assert_eq!(
            recv_until(&engine, |event| *event
                == EngineEvent::NoteOff { midi_note: 60 }),
            EngineEvent::NoteOff { midi_note: 60 }
        );
        recv_until(&engine, |event| {
            *event == EngineEvent::StateChanged(EngineState::Stopped)
        });
        let initial_progress = recv_until(&engine, |event| {
            matches!(event, EngineEvent::CalibrationProgress(_))
        });
        assert_eq!(
            initial_progress,
            EngineEvent::CalibrationProgress(CalibrationProgress {
                requested_note: 36,
                sample_index: 1,
                samples_per_note: 3,
                accepted_samples_for_note: 0,
                accepted_samples: 0,
                required_samples: 111,
                completed_notes: 0,
                rms_dbfs: None,
                peak: None,
                quality: CalibrationSampleQuality::AwaitingOnset,
            })
        );

        engine.send(EngineCommand::RetryCalibrationSample).unwrap();
        let retry_progress = recv_until(&engine, |event| {
            matches!(event, EngineEvent::CalibrationProgress(_))
        });
        assert!(matches!(
            retry_progress,
            EngineEvent::CalibrationProgress(CalibrationProgress {
                requested_note: 36,
                sample_index: 1,
                accepted_samples: 0,
                quality: CalibrationSampleQuality::AwaitingOnset,
                ..
            })
        ));

        engine.send(EngineCommand::CancelCalibration).unwrap();
        assert_eq!(
            recv_until(&engine, |event| *event == EngineEvent::CalibrationCancelled),
            EngineEvent::CalibrationCancelled
        );
        assert!(matches!(engine.try_recv_event(), Err(TryRecvError::Empty)));
        engine.shutdown().unwrap();

        let actions = actions.lock().unwrap();
        assert!(actions.contains(&FakeAction::Stop {
            released_note: Some(60),
        }));
        assert!(actions.contains(&FakeAction::BeginCalibration {
            samples_per_note: 3,
            template_path,
        }));
        assert!(actions.contains(&FakeAction::RetryCalibrationSample));
        assert!(actions.contains(&FakeAction::CancelCalibration));
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
            recv_until(&engine, |event| matches!(
                event,
                EngineEvent::DetectorModeChanged(_)
            )),
            EngineEvent::DetectorModeChanged(DetectorMode::Compare)
        );
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
    fn calibration_levels_are_measured_and_throttled_while_progress_stays_immediate() {
        let mut publisher = OutcomeEventPublisher::default();
        let mut events = Vec::new();
        let start = Instant::now();
        let progress = |quality| {
            CalibrationUpdate::Progress(CalibrationProgress {
                requested_note: 36,
                sample_index: 1,
                samples_per_note: 1,
                accepted_samples_for_note: usize::from(
                    quality == CalibrationSampleQuality::Accepted,
                ),
                accepted_samples: usize::from(quality == CalibrationSampleQuality::Accepted),
                required_samples: 37,
                completed_notes: 0,
                rms_dbfs: None,
                peak: None,
                quality,
            })
        };

        publisher.publish_calibration_frame_at(&[0.5, -0.5], &mut events, start);
        append_calibration_update(
            progress(CalibrationSampleQuality::AwaitingOnset),
            &mut events,
        );

        // A frame 10 ms later is suppressed for the meter, but its progress
        // event still passes through immediately.
        publisher.publish_calibration_frame_at(
            &[0.25, -0.25],
            &mut events,
            start + Duration::from_millis(10),
        );
        append_calibration_update(
            progress(CalibrationSampleQuality::AwaitingOnset),
            &mut events,
        );

        publisher.publish_calibration_frame_at(
            &[0.25, -0.25],
            &mut events,
            start + AUDIO_LEVEL_INTERVAL,
        );
        append_calibration_update(progress(CalibrationSampleQuality::Accepted), &mut events);
        append_calibration_update(
            CalibrationUpdate::NoteCompleted(CalibrationNoteCompletion {
                midi_note: 36,
                completed_notes: 1,
                total_notes: 37,
                accepted_samples: 1,
            }),
            &mut events,
        );

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
        assert_eq!(levels.len(), 2);
        assert!((levels[0].0 - -6.0206).abs() < 0.001);
        assert!((levels[0].1 - -6.0206).abs() < 0.001);
        assert!((levels[1].0 - -12.0412).abs() < 0.001);
        assert!((levels[1].1 - -12.0412).abs() < 0.001);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, EngineEvent::CalibrationProgress(_)))
                .count(),
            3
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, EngineEvent::CalibrationNoteCompleted(_)))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    EngineEvent::CalibrationProgress(progress) => Some(&progress.quality),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![
                &CalibrationSampleQuality::AwaitingOnset,
                &CalibrationSampleQuality::AwaitingOnset,
                &CalibrationSampleQuality::Accepted,
            ]
        );
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
