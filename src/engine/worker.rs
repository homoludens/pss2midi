//! GPUI-independent command and event boundary for the sound engine.

use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvError, SendError, Sender, TryRecvError},
    thread::{self, JoinHandle},
};

use alsa::pcm::PCM;

use crate::engine::{config::DetectorMode, detector::Detector, midi::Midi};

/// Commands sent to the engine's resource-owning worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineCommand {
    Start,
    Stop,
    SetDetectorMode(DetectorMode),
    SetAudioDevice { device: String },
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
    NoteOn { midi_note: u8 },
    NoteOff { midi_note: u8 },
    AudioLevel { rms_dbfs: f32, peak: f32 },
    Error { message: String },
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
    ///
    /// This establishes the controller and ownership boundary; command-specific
    /// runtime behavior is implemented by the engine worker in subsequent
    /// engine work.
    pub fn new() -> io::Result<Self> {
        Self::spawn_with(WorkerResources::default, |_command, _resources| {})
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

    fn spawn_with<F, H>(resource_factory: F, mut handle_command: H) -> io::Result<Self>
    where
        F: FnOnce() -> WorkerResources + Send + 'static,
        H: FnMut(EngineCommand, &mut WorkerResources) + Send + 'static,
    {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("pss2midi-engine".to_owned())
            .spawn(move || {
                let resources = resource_factory();
                run_worker(command_rx, event_tx, resources, &mut handle_command);
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

/// All live device and DSP state is constructed, accessed, and dropped on the
/// worker thread. `AudioFrameReader` borrows `capture` and is created there
/// while processing, rather than stored as a self-referential field here.
#[allow(dead_code)] // Populated and consumed by the worker lifecycle handlers.
#[derive(Default)]
struct WorkerResources {
    capture: Option<PCM>,
    detector: Option<Detector>,
    midi: Option<Midi>,
    #[cfg(test)]
    drop_probe: Option<DropProbe>,
}

fn run_worker<H>(
    command_rx: Receiver<EngineCommand>,
    event_tx: Sender<EngineEvent>,
    resources: WorkerResources,
    handle_command: &mut H,
) where
    H: FnMut(EngineCommand, &mut WorkerResources),
{
    let _ = event_tx.send(EngineEvent::WorkerStarted);
    let mut resources = resources;

    while let Ok(command) = command_rx.recv() {
        if command == EngineCommand::Shutdown {
            break;
        }
        handle_command(command, &mut resources);
    }

    // Release worker-owned resources before advertising that shutdown finished.
    drop(resources);
    let _ = event_tx.send(EngineEvent::WorkerStopped);
}

#[cfg(test)]
struct DropProbe(Sender<()>);

#[cfg(test)]
impl Drop for DropProbe {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn delivers_commands_and_joins_worker_without_hardware() {
        let (command_seen_tx, command_seen_rx) = mpsc::channel();
        let (resource_dropped_tx, resource_dropped_rx) = mpsc::channel();
        let mut engine = PssEngine::spawn_with(
            move || WorkerResources {
                drop_probe: Some(DropProbe(resource_dropped_tx)),
                ..WorkerResources::default()
            },
            move |command, _resources| {
                command_seen_tx.send(command).unwrap();
            },
        )
        .unwrap();

        let command = EngineCommand::SetDetectorMode(DetectorMode::Compare);
        engine.send(command.clone()).unwrap();
        assert_eq!(
            command_seen_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            command
        );

        engine.shutdown().unwrap();
        assert!(resource_dropped_rx.try_recv().is_ok());
        assert_eq!(engine.recv_event().unwrap(), EngineEvent::WorkerStarted);
        assert_eq!(engine.recv_event().unwrap(), EngineEvent::WorkerStopped);
        assert!(matches!(
            engine.try_recv_event(),
            Err(TryRecvError::Disconnected)
        ));
    }
}
