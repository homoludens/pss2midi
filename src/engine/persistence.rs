//! Template validation and JSON persistence on a dedicated non-audio task.

use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread::{self, JoinHandle},
};

#[cfg(test)]
use std::thread::ThreadId;

use anyhow::{ensure, Context, Result};

use crate::engine::{
    calibration::CalibrationResult,
    note::{MAX_MIDI, MIN_MIDI, NOTE_COUNT},
};

pub(crate) struct TemplatePersistenceTask {
    result_rx: Receiver<std::result::Result<PathBuf, String>>,
    worker: Option<JoinHandle<()>>,
}

impl TemplatePersistenceTask {
    pub(crate) fn spawn(result: CalibrationResult, path: PathBuf) -> io::Result<Self> {
        let (result_tx, result_rx) = mpsc::channel();
        let worker_path = path.clone();
        let worker = thread::Builder::new()
            .name("pss2midi-template-persistence".to_owned())
            .spawn(move || {
                let persisted = validate_and_save(result, &worker_path)
                    .map(|()| worker_path)
                    .map_err(|error| format!("{error:#}"));
                let _ = result_tx.send(persisted);
            })?;

        Ok(Self {
            result_rx,
            worker: Some(worker),
        })
    }

    pub(crate) fn try_result(&mut self) -> Option<std::result::Result<PathBuf, String>> {
        match self.result_rx.try_recv() {
            Ok(result) => Some(self.join_worker().map(|()| result).unwrap_or_else(Err)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(self
                .join_worker()
                .err()
                .unwrap_or_else(|| "template persistence task exited without a result".into()))),
        }
    }

    #[cfg(test)]
    pub(crate) fn wait(mut self) -> std::result::Result<PathBuf, String> {
        let result = self
            .result_rx
            .recv()
            .map_err(|_| "template persistence task exited without a result".to_owned());
        let joined = self.join_worker();
        match (result, joined) {
            (Ok(result), Ok(())) => result,
            (Err(message), _) | (_, Err(message)) => Err(message),
        }
    }

    #[cfg(test)]
    pub(crate) fn thread_id(&self) -> ThreadId {
        self.worker
            .as_ref()
            .expect("persistence worker has not been joined")
            .thread()
            .id()
    }

    fn join_worker(&mut self) -> std::result::Result<(), String> {
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "template persistence task panicked".to_owned())?;
        }
        Ok(())
    }
}

impl Drop for TemplatePersistenceTask {
    fn drop(&mut self) {
        let _ = self.join_worker();
    }
}

fn validate_and_save(result: CalibrationResult, path: &std::path::Path) -> Result<()> {
    ensure!(
        result.note_count == NOTE_COUNT,
        "calibration must contain all 37 notes"
    );
    ensure!(
        result.samples_per_note > 0,
        "calibration samples per note must be greater than zero"
    );
    ensure!(
        result.sample_count == NOTE_COUNT * result.samples_per_note,
        "calibration sample count does not match accepted samples per note"
    );
    ensure!(
        result.templates.notes.len() == NOTE_COUNT,
        "template file must contain exactly 37 notes"
    );
    for note in MIN_MIDI..=MAX_MIDI {
        let examples = result
            .templates
            .notes
            .get(&(note as u8))
            .with_context(|| format!("template file is missing MIDI note {note}"))?;
        ensure!(
            examples.len() == result.samples_per_note,
            "MIDI note {note} must contain exactly {} accepted samples",
            result.samples_per_note
        );
    }

    result.templates.validate(
        result.sample_rate,
        result.fft_size,
        result.window_ms,
        result.delay_ms,
        result.feature_len,
    )?;
    result.templates.save(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::templates::TemplateFile;

    #[test]
    fn validation_rejects_incomplete_calibration_data_before_writing() {
        let mut templates = TemplateFile::empty(48_000, 2048, 30.0, 8.0);
        for note in MIN_MIDI..=MAX_MIDI - 1 {
            templates.notes.insert(note as u8, vec![vec![1.0; 3]]);
        }
        let result = CalibrationResult {
            templates,
            note_count: NOTE_COUNT - 1,
            sample_count: NOTE_COUNT - 1,
            samples_per_note: 1,
            feature_len: 3,
            sample_rate: 48_000,
            fft_size: 2048,
            window_ms: 30.0,
            delay_ms: 8.0,
        };

        assert!(validate_and_save(result, std::path::Path::new("unused.json")).is_err());
    }
}
