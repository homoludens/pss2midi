use anyhow::{ensure, Result};

use crate::engine::{
    config::{AudioConfig, SpectralConfig},
    features::{FeatureExtractor, SampleCapture},
    note::{rms_db, MAX_MIDI, MIN_MIDI, NOTE_COUNT},
    onset::OnsetDetector,
    templates::TemplateFile,
};

const CLIPPED_PEAK: f32 = 0.98;
pub const DEFAULT_SAMPLES_PER_NOTE: usize = 5;

#[derive(Clone, Debug, PartialEq)]
pub enum CalibrationSampleQuality {
    AwaitingOnset,
    Accepted,
    Rejected(CalibrationRejectionReason),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalibrationRejectionReason {
    TooQuiet,
    Clipped,
    Invalid,
}

/// Sample-level progress. `sample_index` is one-based and identifies the
/// current attempt; rejected captures retain the same index and accepted
/// captures report their result before progress moves to the next attempt.
#[derive(Clone, Debug, PartialEq)]
pub struct CalibrationProgress {
    pub requested_note: u8,
    pub sample_index: usize,
    pub samples_per_note: usize,
    pub accepted_samples_for_note: usize,
    pub accepted_samples: usize,
    pub required_samples: usize,
    pub completed_notes: usize,
    pub rms_dbfs: Option<f32>,
    /// Absolute peak amplitude in full-scale linear units (0.0–1.0 normally).
    pub peak: Option<f32>,
    pub quality: CalibrationSampleQuality,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalibrationNoteCompletion {
    pub midi_note: u8,
    pub completed_notes: usize,
    pub total_notes: usize,
    pub accepted_samples: usize,
}

#[derive(Debug)]
pub struct CalibrationResult {
    pub templates: TemplateFile,
    pub note_count: usize,
    pub sample_count: usize,
    pub samples_per_note: usize,
    pub feature_len: usize,
    pub sample_rate: u32,
    pub fft_size: usize,
    pub window_ms: f32,
    pub delay_ms: f32,
}

#[derive(Debug)]
pub enum CalibrationUpdate {
    Progress(CalibrationProgress),
    NoteCompleted(CalibrationNoteCompletion),
    Completed(CalibrationResult),
}

/// GPUI-independent calibration state machine. Audio frames are identified by
/// their absolute sample index so onset delay and spectral-window extraction
/// use the same sample-accurate capture helper as the detector.
pub struct CalibrationSession {
    onset: OnsetDetector,
    extractor: FeatureExtractor,
    capture: Option<SampleCapture>,
    delay_samples: u64,
    silence_db: f32,
    samples_per_note: usize,
    current_note: u8,
    current_examples: Vec<Vec<f32>>,
    templates: Option<TemplateFile>,
    accepted_samples: usize,
    completed_notes: usize,
    complete: bool,
}

impl CalibrationSession {
    pub fn new(
        audio: &AudioConfig,
        sample_rate: u32,
        hop: usize,
        spectral: &SpectralConfig,
        samples_per_note: usize,
    ) -> Result<Self> {
        ensure!(
            samples_per_note > 0,
            "samples per note must be greater than zero"
        );
        ensure!(sample_rate > 0, "sample rate must be greater than zero");
        ensure!(hop > 0, "audio hop must be greater than zero");
        ensure!(
            audio.silence_db.is_finite(),
            "silence threshold must be finite"
        );
        ensure!(
            spectral.spectral_delay_ms.is_finite() && spectral.spectral_delay_ms >= 0.0,
            "spectral delay must be a finite non-negative number"
        );

        let onset = OnsetDetector::new(audio, audio.onset_buffer, 0.30, 60.0, sample_rate, hop)?;
        let extractor =
            FeatureExtractor::new(sample_rate, spectral.spectral_window_ms, spectral.fft_size)?;
        let delay_samples =
            (sample_rate as f32 * spectral.spectral_delay_ms / 1000.0).round() as u64;
        let templates = TemplateFile::empty(
            sample_rate,
            spectral.fft_size,
            spectral.spectral_window_ms,
            spectral.spectral_delay_ms,
        );

        Ok(Self {
            onset,
            extractor,
            capture: None,
            delay_samples,
            silence_db: audio.silence_db,
            samples_per_note,
            current_note: MIN_MIDI as u8,
            current_examples: Vec::with_capacity(samples_per_note),
            templates: Some(templates),
            accepted_samples: 0,
            completed_notes: 0,
            complete: false,
        })
    }

    pub fn initial_progress(&self) -> CalibrationProgress {
        self.progress(
            self.current_note,
            1,
            None,
            None,
            CalibrationSampleQuality::AwaitingOnset,
        )
    }

    /// Discard an in-flight window and keep the same requested note/sample.
    pub fn retry_current_sample(&mut self) -> CalibrationProgress {
        self.capture = None;
        self.current_progress()
    }

    pub fn current_progress(&self) -> CalibrationProgress {
        self.progress(
            self.current_note,
            self.current_examples.len() + 1,
            None,
            None,
            CalibrationSampleQuality::AwaitingOnset,
        )
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Consume a sequential capture frame. Only onset and calibration metadata
    /// leave the session; audio samples remain private to the engine.
    pub fn process_frame(
        &mut self,
        frame_start: u64,
        frame: &[f32],
    ) -> Result<Vec<CalibrationUpdate>> {
        ensure!(!self.complete, "calibration session is already complete");
        if self.capture.is_none() && self.onset.detect(frame)? {
            self.capture = Some(SampleCapture::new(
                frame_start + self.delay_samples,
                self.extractor.window_samples(),
            ));
        }

        if let Some(capture) = self.capture.as_mut() {
            capture.push_frame(frame_start, frame);
            if capture.is_complete() {
                let samples = capture.samples().to_vec();
                self.capture = None;
                return self.process_captured_window(&samples);
            }
        }
        Ok(Vec::new())
    }

    fn process_captured_window(&mut self, samples: &[f32]) -> Result<Vec<CalibrationUpdate>> {
        ensure!(!self.complete, "calibration session is already complete");
        let note = self.current_note;
        let sample_index = self.current_examples.len() + 1;
        let all_finite = samples.iter().all(|sample| sample.is_finite());
        let levels_available = !samples.is_empty() && all_finite;
        let rms_dbfs = levels_available.then(|| rms_db(samples));
        let peak = levels_available.then(|| {
            samples
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
        });

        let mut feature = None;
        let quality = if samples.len() != self.extractor.window_samples() || !all_finite {
            CalibrationSampleQuality::Rejected(CalibrationRejectionReason::Invalid)
        } else if rms_dbfs.is_some_and(|rms| rms < self.silence_db) {
            CalibrationSampleQuality::Rejected(CalibrationRejectionReason::TooQuiet)
        } else if peak.is_some_and(|peak| peak >= CLIPPED_PEAK) {
            CalibrationSampleQuality::Rejected(CalibrationRejectionReason::Clipped)
        } else {
            match self.extractor.extract(samples) {
                Ok(extracted) => {
                    feature = Some(extracted);
                    CalibrationSampleQuality::Accepted
                }
                Err(_) => CalibrationSampleQuality::Rejected(CalibrationRejectionReason::Invalid),
            }
        };

        let accepted = matches!(&quality, CalibrationSampleQuality::Accepted);
        if let Some(feature) = feature {
            self.current_examples.push(feature);
            self.accepted_samples += 1;
        }

        let mut updates = vec![CalibrationUpdate::Progress(self.progress(
            note,
            sample_index,
            rms_dbfs,
            peak,
            quality,
        ))];

        if !accepted || self.current_examples.len() < self.samples_per_note {
            updates.push(CalibrationUpdate::Progress(self.current_progress()));
            return Ok(updates);
        }

        self.templates
            .as_mut()
            .expect("incomplete calibration retains its template data")
            .notes
            .insert(note, std::mem::take(&mut self.current_examples));
        self.completed_notes += 1;
        updates.push(CalibrationUpdate::NoteCompleted(
            CalibrationNoteCompletion {
                midi_note: note,
                completed_notes: self.completed_notes,
                total_notes: NOTE_COUNT,
                accepted_samples: self.accepted_samples,
            },
        ));

        if note as i32 == MAX_MIDI {
            self.complete = true;
            let templates = self
                .templates
                .take()
                .expect("completed calibration owns template data");
            let delay_ms = templates.delay_ms;
            updates.push(CalibrationUpdate::Completed(CalibrationResult {
                templates,
                note_count: self.completed_notes,
                sample_count: self.accepted_samples,
                samples_per_note: self.samples_per_note,
                feature_len: self.extractor.feature_len(),
                sample_rate: self.extractor.sample_rate(),
                fft_size: self.extractor.fft_size(),
                window_ms: self.extractor.window_ms(),
                delay_ms,
            }));
        } else {
            self.current_note += 1;
            self.current_examples = Vec::with_capacity(self.samples_per_note);
            updates.push(CalibrationUpdate::Progress(self.current_progress()));
        }

        Ok(updates)
    }

    fn progress(
        &self,
        requested_note: u8,
        sample_index: usize,
        rms_dbfs: Option<f32>,
        peak: Option<f32>,
        quality: CalibrationSampleQuality,
    ) -> CalibrationProgress {
        CalibrationProgress {
            requested_note,
            sample_index,
            samples_per_note: self.samples_per_note,
            accepted_samples_for_note: self.current_examples.len(),
            accepted_samples: self.accepted_samples,
            required_samples: NOTE_COUNT * self.samples_per_note,
            completed_notes: self.completed_notes,
            rms_dbfs,
            peak,
            quality,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::persistence::TemplatePersistenceTask;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn audio_config() -> AudioConfig {
        AudioConfig::default()
    }

    fn spectral_config() -> SpectralConfig {
        SpectralConfig::default()
    }

    fn session(samples_per_note: usize) -> CalibrationSession {
        CalibrationSession::new(
            &audio_config(),
            48_000,
            128,
            &spectral_config(),
            samples_per_note,
        )
        .unwrap()
    }

    fn tone(session: &CalibrationSession, amplitude: f32) -> Vec<f32> {
        (0..session.extractor.window_samples())
            .map(|index| {
                amplitude * (2.0 * std::f32::consts::PI * 440.0 * index as f32 / 48_000.0).sin()
            })
            .collect()
    }

    fn latest_progress(updates: &[CalibrationUpdate]) -> &CalibrationProgress {
        updates
            .iter()
            .find_map(|update| match update {
                CalibrationUpdate::Progress(progress)
                    if !matches!(&progress.quality, CalibrationSampleQuality::AwaitingOnset) =>
                {
                    Some(progress)
                }
                _ => None,
            })
            .expect("capture should report sample quality")
    }

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "pss2midi-calibration-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rejected_quiet_and_clipped_captures_keep_the_same_sample_request() {
        let mut session = session(2);
        let quiet = vec![0.0; session.extractor.window_samples()];
        let quiet_updates = session.process_captured_window(&quiet).unwrap();
        let quiet_progress = latest_progress(&quiet_updates);
        assert_eq!(quiet_progress.requested_note, 36);
        assert_eq!(quiet_progress.sample_index, 1);
        assert_eq!(quiet_progress.accepted_samples_for_note, 0);
        assert_eq!(quiet_progress.accepted_samples, 0);
        assert_eq!(
            quiet_progress.quality,
            CalibrationSampleQuality::Rejected(CalibrationRejectionReason::TooQuiet)
        );

        let clipped = vec![0.99; session.extractor.window_samples()];
        let clipped_updates = session.process_captured_window(&clipped).unwrap();
        let clipped_progress = latest_progress(&clipped_updates);
        assert_eq!(clipped_progress.requested_note, 36);
        assert_eq!(clipped_progress.sample_index, 1);
        assert_eq!(clipped_progress.accepted_samples_for_note, 0);
        assert_eq!(clipped_progress.accepted_samples, 0);
        assert_eq!(
            clipped_progress.quality,
            CalibrationSampleQuality::Rejected(CalibrationRejectionReason::Clipped)
        );

        let invalid = vec![f32::NAN; session.extractor.window_samples()];
        let invalid_updates = session.process_captured_window(&invalid).unwrap();
        let invalid_progress = latest_progress(&invalid_updates);
        assert_eq!(invalid_progress.requested_note, 36);
        assert_eq!(invalid_progress.sample_index, 1);
        assert_eq!(invalid_progress.accepted_samples_for_note, 0);
        assert_eq!(invalid_progress.accepted_samples, 0);
        assert_eq!(invalid_progress.rms_dbfs, None);
        assert_eq!(invalid_progress.peak, None);
        assert_eq!(
            invalid_progress.quality,
            CalibrationSampleQuality::Rejected(CalibrationRejectionReason::Invalid)
        );

        let retry = session.retry_current_sample();
        assert_eq!(retry.requested_note, 36);
        assert_eq!(retry.sample_index, 1);
        assert_eq!(retry.accepted_samples, 0);

        let valid = tone(&session, 0.2);
        let accepted_updates = session.process_captured_window(&valid).unwrap();
        let accepted = latest_progress(&accepted_updates);
        assert_eq!(accepted.requested_note, 36);
        assert_eq!(accepted.sample_index, 1);
        assert_eq!(accepted.accepted_samples_for_note, 1);
        assert_eq!(accepted.accepted_samples, 1);
        assert_eq!(accepted.quality, CalibrationSampleQuality::Accepted);
        assert_eq!(session.current_progress().sample_index, 2);
    }

    #[test]
    fn default_session_accounts_for_all_37_notes_and_185_accepted_samples() {
        let mut session = session(DEFAULT_SAMPLES_PER_NOTE);
        let sample = tone(&session, 0.2);
        let mut completed_notes = Vec::new();
        let mut result = None;
        let mut accepted_progress_events = 0;

        for _ in 0..NOTE_COUNT * DEFAULT_SAMPLES_PER_NOTE {
            let updates = session.process_captured_window(&sample).unwrap();
            for update in updates {
                match update {
                    CalibrationUpdate::Progress(progress)
                        if progress.quality == CalibrationSampleQuality::Accepted =>
                    {
                        accepted_progress_events += 1;
                    }
                    CalibrationUpdate::NoteCompleted(note) => {
                        completed_notes.push(note.midi_note);
                    }
                    CalibrationUpdate::Completed(completed) => result = Some(completed),
                    CalibrationUpdate::Progress(_) => {}
                }
            }
        }

        let result = result.expect("all notes should complete");
        assert!(session.is_complete());
        assert_eq!(accepted_progress_events, 185);
        assert_eq!(
            completed_notes,
            (MIN_MIDI..=MAX_MIDI)
                .map(|note| note as u8)
                .collect::<Vec<_>>()
        );
        assert_eq!(result.note_count, 37);
        assert_eq!(result.sample_count, 185);
        assert_eq!(result.samples_per_note, DEFAULT_SAMPLES_PER_NOTE);
        assert_eq!(result.templates.notes.len(), 37);
        assert!(result
            .templates
            .notes
            .values()
            .all(|examples| examples.len() == DEFAULT_SAMPLES_PER_NOTE));
    }

    #[test]
    fn completed_templates_save_and_load_on_a_separate_persistence_thread() {
        let mut session = session(DEFAULT_SAMPLES_PER_NOTE);
        let sample = tone(&session, 0.2);
        let mut result = None;
        for _ in 0..NOTE_COUNT * DEFAULT_SAMPLES_PER_NOTE {
            for update in session.process_captured_window(&sample).unwrap() {
                if let CalibrationUpdate::Completed(completed) = update {
                    result = Some(completed);
                }
            }
        }
        let result = result.expect("calibration should produce template data");
        let directory = TestDirectory::new();
        let path = directory.0.join("templates.json");
        let persistence = TemplatePersistenceTask::spawn(result, path.clone()).unwrap();
        assert_ne!(persistence.thread_id(), std::thread::current().id());
        assert_eq!(persistence.wait().unwrap(), path);

        let loaded = TemplateFile::load(&path).unwrap();
        assert_eq!(loaded.notes.len(), 37);
        assert_eq!(loaded.notes.values().map(Vec::len).sum::<usize>(), 185);
        loaded
            .validate(48_000, 2048, 30.0, 8.0, session.extractor.feature_len())
            .unwrap();
    }
}
