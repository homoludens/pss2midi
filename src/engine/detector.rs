use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use aubio::{Pitch, PitchMode, PitchUnit};

pub use crate::engine::templates::RankedMatch;

use crate::engine::{
    config::{DetectorMode, RunConfig},
    features::{FeatureExtractor, SampleCapture, SampleHistory},
    note::{ms_to_hops, quantize_pitch, rms_db, MIN_MIDI, NOTE_COUNT},
    onset::OnsetDetector,
    templates::{validate_for_extractor, Classification, TemplateFile},
};

const FALLBACK_ATTACK_MARGIN_DB: f32 = 10.0;

/// A note-on/off decision produced by DSP and applied by the owning caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteDecision {
    NoteOn { note: u8 },
    NoteOff { note: u8 },
}

/// Confidence in a completed YIN onset decision, based on actual pitch votes.
#[derive(Clone, Debug, PartialEq)]
pub struct YinDecision {
    pub candidate_note: Option<u8>,
    pub accepted: bool,
    pub vote_count: usize,
    pub total_votes: usize,
    pub vote_ratio: Option<f32>,
}

/// YIN's current pitch estimate and, when completed, its onset-vote decision.
#[derive(Clone, Debug, PartialEq)]
pub struct YinResult {
    /// Aubio's pitch estimate in MIDI units, absent when aubio reports no pitch.
    pub midi_pitch: Option<f32>,
    /// Quantized MIDI note when the estimate is within the supported range/tolerance.
    pub note: Option<u8>,
    pub cents: Option<f32>,
    pub decision: Option<YinDecision>,
}

impl YinResult {
    fn from_pitch(midi_pitch: f32, detected: Option<(u8, f32)>) -> Self {
        Self {
            midi_pitch: (midi_pitch.is_finite() && midi_pitch > 0.0).then_some(midi_pitch),
            note: detected.map(|(note, _)| note),
            cents: detected.map(|(_, cents)| cents),
            decision: None,
        }
    }
}

/// Spectral classification data from the detector's existing template scores.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectralResult {
    /// Top-ranked classifier candidate, including when it is rejected by thresholds.
    pub note: u8,
    pub confidence: f32,
    pub second_score: f32,
    pub margin: f32,
    pub accepted: bool,
    /// Accepted spectral note selected for output, absent when rejected/suppressed.
    pub selected_note: Option<u8>,
    pub ranked_matches: Vec<RankedMatch>,
}

impl SpectralResult {
    fn from_classification(classification: Classification) -> Self {
        let selected_note = classification.accepted.then_some(classification.note);
        Self {
            note: classification.note,
            confidence: classification.score,
            second_score: classification.second_score,
            margin: classification.margin,
            accepted: classification.accepted,
            selected_note,
            ranked_matches: classification.ranked_matches,
        }
    }
}

/// GPUI-independent values produced by one detector processing step.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectorOutcome {
    pub onset_detected: bool,
    pub selected_note: Option<u8>,
    /// Ordered MIDI actions; replacements contain NoteOff before NoteOn.
    pub note_decisions: Vec<NoteDecision>,
    pub yin: Option<YinResult>,
    pub spectral: Option<SpectralResult>,
    pub onset_to_note_latency: Option<Duration>,
    pub rms_dbfs: f32,
    pub peak: f32,
}

impl DetectorOutcome {
    fn new(rms_dbfs: f32, peak: f32) -> Self {
        Self {
            onset_detected: false,
            selected_note: None,
            note_decisions: Vec::new(),
            yin: None,
            spectral: None,
            onset_to_note_latency: None,
            rms_dbfs,
            peak,
        }
    }
}

struct PendingDecision {
    onset_at: Instant,
    ignore_left: usize,
    collect_left: usize,
    extension_hops: usize,
    extensions_left: usize,
    votes: [u16; NOTE_COUNT],
    total_votes: u16,
}

impl PendingDecision {
    fn new(
        onset_at: Instant,
        ignore_hops: usize,
        decision_hops: usize,
        extension_hops: usize,
    ) -> Self {
        Self {
            onset_at,
            ignore_left: ignore_hops,
            collect_left: decision_hops,
            extension_hops,
            extensions_left: 1,
            votes: [0; NOTE_COUNT],
            total_votes: 0,
        }
    }

    fn vote(&mut self, note: u8) {
        let index = note as i32 - MIN_MIDI;
        if index >= 0 && (index as usize) < NOTE_COUNT {
            self.votes[index as usize] += 1;
            self.total_votes += 1;
        }
    }

    fn winner(&self) -> Option<(u8, u16, f32)> {
        if self.total_votes == 0 {
            return None;
        }
        let (index, count) = self
            .votes
            .iter()
            .enumerate()
            .max_by_key(|(_, count)| *count)?;
        Some((
            (MIN_MIDI + index as i32) as u8,
            *count,
            *count as f32 / self.total_votes as f32,
        ))
    }

    fn result(&self, required_ratio: f32) -> YinDecision {
        let winner = self.winner();
        YinDecision {
            candidate_note: winner.map(|(note, _, _)| note),
            accepted: winner.is_some_and(|(_, count, ratio)| count >= 2 && ratio >= required_ratio),
            vote_count: winner.map_or(0, |(_, count, _)| count as usize),
            total_votes: self.total_votes as usize,
            vote_ratio: winner.map(|(_, _, ratio)| ratio),
        }
    }
}

struct PendingSpectral {
    onset_at: Instant,
    capture: SampleCapture,
    yin_ready: bool,
    classification: Option<Classification>,
}

pub struct Detector {
    mode: DetectorMode,
    pitch: Option<Pitch>,
    onset: OnsetDetector,
    extractor: Option<FeatureExtractor>,
    templates: Option<TemplateFile>,
    current_note: Option<u8>,
    pending_yin: Option<PendingDecision>,
    pending_spectral: Option<PendingSpectral>,
    fallback_note: Option<u8>,
    fallback_count: usize,
    silence_count: usize,
    attack_ignore_hops: usize,
    decision_hops: usize,
    decision_extension_hops: usize,
    release_hops: usize,
    initial_stable: usize,
    vote_ratio: f32,
    retrigger_time: Duration,
    last_trigger: Option<Instant>,
    silence_db: f32,
    spectral_delay_samples: u64,
    spectral_window_samples: usize,
    spectral_min_score: f32,
    spectral_min_margin: f32,
    history: SampleHistory,
    sample_index: u64,
}

impl Detector {
    pub fn new(args: &RunConfig, sample_rate: u32, hop: usize) -> Result<Self> {
        let templates = if args.detector == DetectorMode::Yin {
            None
        } else {
            Some(TemplateFile::load(&args.template_path)?)
        };
        Self::new_with_templates(args, sample_rate, hop, templates)
    }

    /// Construct a detector using templates already loaded by the worker.
    pub fn new_with_templates(
        args: &RunConfig,
        sample_rate: u32,
        hop: usize,
        loaded_templates: Option<TemplateFile>,
    ) -> Result<Self> {
        if args.detector != DetectorMode::Yin {
            ensure!(
                args.spectral.spectral_delay_ms.is_finite()
                    && args.spectral.spectral_delay_ms >= 0.0,
                "spectral delay must be a finite non-negative number"
            );
            ensure!(
                args.spectral.spectral_min_score.is_finite()
                    && (0.0..=1.0).contains(&args.spectral.spectral_min_score),
                "spectral minimum score must be between 0 and 1"
            );
            ensure!(
                args.spectral.spectral_min_margin.is_finite()
                    && (0.0..=1.0).contains(&args.spectral.spectral_min_margin),
                "spectral minimum margin must be between 0 and 1"
            );
        }
        let mode = args.detector;
        let pitch = if mode != DetectorMode::Spectral {
            Some(
                Pitch::new(PitchMode::Yin, args.pitch_buffer, hop, sample_rate)?
                    .with_unit(PitchUnit::Midi)
                    .with_silence(args.audio.silence_db),
            )
        } else {
            None
        };

        let onset = OnsetDetector::new(
            &args.audio,
            args.audio.onset_buffer,
            args.onset_threshold,
            args.retrigger_ms,
            sample_rate,
            hop,
        )?;

        let (extractor, templates) = if mode == DetectorMode::Yin {
            (None, None)
        } else {
            let extractor = FeatureExtractor::new(
                sample_rate,
                args.spectral.spectral_window_ms,
                args.spectral.fft_size,
            )?;
            let templates = loaded_templates.context("Spectral templates have not been loaded")?;
            validate_for_extractor(&templates, &extractor, args.spectral.spectral_delay_ms)?;
            (Some(extractor), Some(templates))
        };

        let spectral_window_samples = extractor
            .as_ref()
            .map_or(0, FeatureExtractor::window_samples);
        Ok(Self {
            mode,
            pitch,
            onset,
            extractor,
            templates,
            current_note: None,
            pending_yin: None,
            pending_spectral: None,
            fallback_note: None,
            fallback_count: 0,
            silence_count: 0,
            attack_ignore_hops: ms_to_hops(args.attack_ignore_ms, sample_rate, hop),
            decision_hops: ms_to_hops(args.decision_window_ms, sample_rate, hop),
            decision_extension_hops: ms_to_hops(args.decision_extend_ms, sample_rate, hop),
            release_hops: ms_to_hops(args.release_ms, sample_rate, hop),
            initial_stable: args.initial_stable,
            vote_ratio: args.vote_ratio,
            retrigger_time: Duration::from_secs_f32(args.retrigger_ms / 1000.0),
            last_trigger: None,
            silence_db: args.audio.silence_db,
            spectral_delay_samples: (sample_rate as f32 * args.spectral.spectral_delay_ms / 1000.0)
                .round() as u64,
            spectral_window_samples,
            spectral_min_score: args.spectral.spectral_min_score,
            spectral_min_margin: args.spectral.spectral_min_margin,
            history: SampleHistory::new((sample_rate as usize / 5).max(hop)),
            sample_index: 0,
        })
    }

    fn clear_fallback(&mut self) {
        self.fallback_note = None;
        self.fallback_count = 0;
    }

    fn send_new_note(
        &mut self,
        note: u8,
        onset_time: Option<Instant>,
    ) -> (Vec<NoteDecision>, Option<Duration>) {
        let now = Instant::now();
        let mut decisions = Vec::with_capacity(2);
        match self.current_note {
            None => {
                decisions.push(NoteDecision::NoteOn { note });
                self.current_note = Some(note);
                self.last_trigger = Some(now);
            }
            Some(current) if current == note => {
                if onset_time.is_none() {
                    return (decisions, None);
                }
                let cooldown_ok = self
                    .last_trigger
                    .map(|last| now.duration_since(last) >= self.retrigger_time)
                    .unwrap_or(true);
                if !cooldown_ok {
                    return (decisions, None);
                }
                decisions.push(NoteDecision::NoteOff { note });
                decisions.push(NoteDecision::NoteOn { note });
                self.last_trigger = Some(now);
            }
            Some(current) => {
                decisions.push(NoteDecision::NoteOff { note: current });
                decisions.push(NoteDecision::NoteOn { note });
                self.current_note = Some(note);
                self.last_trigger = Some(now);
            }
        }
        let emitted_note_on = decisions
            .iter()
            .any(|decision| matches!(decision, NoteDecision::NoteOn { .. }));
        let latency = emitted_note_on
            .then(|| onset_time.map(|onset_time| now.duration_since(onset_time)))
            .flatten();
        (decisions, latency)
    }

    fn release(&mut self) -> Vec<NoteDecision> {
        let decisions = self
            .current_note
            .take()
            .map(|note| vec![NoteDecision::NoteOff { note }])
            .unwrap_or_default();
        self.pending_yin = None;
        if self.mode == DetectorMode::Compare {
            if let Some(pending) = self.pending_spectral.as_mut() {
                pending.yin_ready = true;
            }
        }
        if self.mode == DetectorMode::Yin {
            self.pending_spectral = None;
        }
        self.clear_fallback();
        decisions
    }

    fn complete_outcome(&self, mut outcome: DetectorOutcome) -> DetectorOutcome {
        outcome.selected_note = self.current_note;
        outcome
    }

    pub fn process(&mut self, frame: &[f32]) -> Result<DetectorOutcome> {
        let frame_start = self.sample_index;
        self.sample_index += frame.len() as u64;
        if self.mode != DetectorMode::Yin {
            self.history.push_frame(frame_start, frame);
        }
        let now = Instant::now();
        let level = rms_db(frame);
        let peak = frame
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        let mut outcome = DetectorOutcome::new(level, peak);
        let pitch_value = if let Some(pitch) = self.pitch.as_mut() {
            pitch.do_result(frame)?
        } else {
            -1.0
        };
        let detected = quantize_pitch(pitch_value);
        if self.pitch.is_some() {
            outcome.yin = Some(YinResult::from_pitch(pitch_value, detected));
        }
        let onset_detected = self.onset.detect(frame)?;

        if self.mode == DetectorMode::Yin {
            self.process_yin(level, detected, onset_detected, now, &mut outcome)?;
            return Ok(self.complete_outcome(outcome));
        }

        let is_silent = level < self.silence_db;
        if is_silent {
            self.silence_count += 1;
            self.clear_fallback();
            if self.silence_count >= self.release_hops {
                outcome.note_decisions.extend(self.release());
            }
        } else {
            self.silence_count = 0;
        }

        let new_attack = onset_detected && !is_silent;
        if new_attack {
            outcome.onset_detected = true;
            let capture = SampleCapture::new(
                frame_start + self.spectral_delay_samples,
                self.spectral_window_samples,
            );
            self.pending_spectral = Some(PendingSpectral {
                onset_at: now,
                capture,
                yin_ready: false,
                classification: None,
            });
            self.pending_yin = (self.mode == DetectorMode::Compare).then(|| {
                PendingDecision::new(
                    now,
                    self.attack_ignore_hops,
                    self.decision_hops,
                    self.decision_extension_hops,
                )
            });
        }

        if self.mode == DetectorMode::Compare {
            if let Some(yin_result) = self.advance_yin_decision(detected) {
                if let Some(pending) = self.pending_spectral.as_mut() {
                    pending.yin_ready = true;
                }
                if let Some(yin) = outcome.yin.as_mut() {
                    yin.decision = Some(yin_result);
                }
            }
        }

        if let Some(mut pending) = self.pending_spectral.take() {
            if pending.classification.is_none() {
                if new_attack {
                    self.history.copy_available_to(&mut pending.capture);
                } else {
                    pending.capture.push_frame(frame_start, frame);
                }
                if pending.capture.is_complete() {
                    let features = self
                        .extractor
                        .as_mut()
                        .expect("spectral extractor is initialized")
                        .extract(pending.capture.samples())?;
                    pending.classification = Some(
                        self.templates
                            .as_ref()
                            .expect("spectral templates are initialized")
                            .classify_spectral(
                                &features,
                                self.spectral_min_score,
                                self.spectral_min_margin,
                            )?,
                    );
                }
            }

            let compare_waiting_for_yin = self.mode == DetectorMode::Compare && !pending.yin_ready;
            if let Some(classification) = pending.classification.take() {
                if compare_waiting_for_yin {
                    pending.classification = Some(classification);
                    self.pending_spectral = Some(pending);
                } else {
                    self.finish_spectral_decision(pending, classification, &mut outcome)?;
                }
            } else {
                self.pending_spectral = Some(pending);
            }
        }

        Ok(self.complete_outcome(outcome))
    }

    fn process_yin(
        &mut self,
        level: f32,
        detected: Option<(u8, f32)>,
        onset_detected: bool,
        now: Instant,
        outcome: &mut DetectorOutcome,
    ) -> Result<()> {
        if level < self.silence_db {
            self.silence_count += 1;
            self.clear_fallback();
            if self.silence_count >= self.release_hops {
                outcome.note_decisions.extend(self.release());
            }
            return Ok(());
        }
        self.silence_count = 0;

        if onset_detected {
            outcome.onset_detected = true;
            self.pending_yin = Some(PendingDecision::new(
                now,
                self.attack_ignore_hops,
                self.decision_hops,
                self.decision_extension_hops,
            ));
            self.clear_fallback();
        }

        if let Some(mut pending) = self.pending_yin.take() {
            if pending.ignore_left > 0 {
                pending.ignore_left -= 1;
                self.pending_yin = Some(pending);
                return Ok(());
            }
            if let Some((note, _cents)) = detected {
                pending.vote(note);
            }
            pending.collect_left = pending.collect_left.saturating_sub(1);
            if pending.collect_left > 0 {
                self.pending_yin = Some(pending);
                return Ok(());
            }

            let decision = pending.result(self.vote_ratio);
            let onset_at = pending.onset_at;
            if decision.accepted {
                let winner = decision
                    .candidate_note
                    .expect("an accepted YIN decision has a candidate note");
                if let Some(yin) = outcome.yin.as_mut() {
                    yin.decision = Some(decision);
                }
                let (actions, latency) = self.send_new_note(winner, Some(onset_at));
                outcome.note_decisions.extend(actions);
                outcome.onset_to_note_latency = latency;
                self.clear_fallback();
                return Ok(());
            }
            if pending.extensions_left > 0 {
                pending.extensions_left -= 1;
                pending.collect_left = pending.extension_hops;
                self.pending_yin = Some(pending);
                return Ok(());
            }
            if let Some(yin) = outcome.yin.as_mut() {
                yin.decision = Some(decision);
            }
            return Ok(());
        }

        let Some((note, _cents)) = detected else {
            self.clear_fallback();
            return Ok(());
        };
        if self.current_note.is_some() {
            self.clear_fallback();
            return Ok(());
        }
        if level < self.silence_db + FALLBACK_ATTACK_MARGIN_DB {
            self.clear_fallback();
            return Ok(());
        }
        if self.fallback_note == Some(note) {
            self.fallback_count += 1;
        } else {
            self.fallback_note = Some(note);
            self.fallback_count = 1;
        }
        if self.fallback_count >= self.initial_stable {
            let (actions, latency) = self.send_new_note(note, None);
            outcome.note_decisions.extend(actions);
            outcome.onset_to_note_latency = latency;
            self.clear_fallback();
        }
        Ok(())
    }

    /// Runs only as a diagnostic companion in compare mode.
    /// It shares the existing YIN onset vote rules and never drives MIDI.
    fn advance_yin_decision(&mut self, detected: Option<(u8, f32)>) -> Option<YinDecision> {
        let mut pending = self.pending_yin.take()?;
        if pending.ignore_left > 0 {
            pending.ignore_left -= 1;
            self.pending_yin = Some(pending);
            return None;
        }
        if let Some((note, _)) = detected {
            pending.vote(note);
        }
        pending.collect_left = pending.collect_left.saturating_sub(1);
        if pending.collect_left > 0 {
            self.pending_yin = Some(pending);
            return None;
        }
        let decision = pending.result(self.vote_ratio);
        if decision.accepted {
            return Some(decision);
        }
        if pending.extensions_left > 0 {
            pending.extensions_left -= 1;
            pending.collect_left = pending.extension_hops;
            self.pending_yin = Some(pending);
            return None;
        }
        Some(decision)
    }

    fn finish_spectral_decision(
        &mut self,
        pending: PendingSpectral,
        classification: Classification,
        outcome: &mut DetectorOutcome,
    ) -> Result<()> {
        let spectral_note = classification.accepted.then_some(classification.note);
        let released_before_result =
            self.current_note.is_none() && self.silence_count >= self.release_hops;
        if released_before_result {
            let mut spectral = SpectralResult::from_classification(classification);
            spectral.selected_note = None;
            outcome.spectral = Some(spectral);
            return Ok(());
        }
        if let Some(note) = spectral_note {
            let (actions, latency) = self.send_new_note(note, Some(pending.onset_at));
            outcome.note_decisions.extend(actions);
            outcome.onset_to_note_latency = latency;
        }
        outcome.spectral = Some(SpectralResult::from_classification(classification));
        Ok(())
    }

    pub fn shutdown(&mut self) -> Vec<NoteDecision> {
        self.release()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yin_detector() -> Detector {
        let args = RunConfig::default();
        Detector::new(&args, args.audio.sample_rate, args.audio.hop).unwrap()
    }

    fn decision(notes: &[u8]) -> Option<u8> {
        let mut pending = PendingDecision::new(Instant::now(), 0, notes.len(), 0);
        for &note in notes {
            pending.vote(note);
        }
        pending.winner().map(|(note, _, _)| note)
    }

    #[test]
    fn stable_c3() {
        assert_eq!(decision(&[48, 48, 48, 48]), Some(48));
    }

    #[test]
    fn attack_harmonics_choose_c3() {
        assert_eq!(decision(&[64, 40, 48, 48, 48, 48]), Some(48));
    }

    #[test]
    fn transient_does_not_dominate() {
        assert_eq!(decision(&[48, 48, 40, 72, 48, 48]), Some(48));
    }

    #[test]
    fn completed_yin_result_reports_vote_count_ratio_and_acceptance() {
        let mut pending = PendingDecision::new(Instant::now(), 0, 4, 0);
        for note in [48, 48, 49, 48] {
            pending.vote(note);
        }

        let result = pending.result(0.6);
        assert_eq!(result.candidate_note, Some(48));
        assert!(result.accepted);
        assert_eq!(result.vote_count, 3);
        assert_eq!(result.total_votes, 4);
        assert_eq!(result.vote_ratio, Some(0.75));

        assert!(!pending.result(0.8).accepted);
    }

    #[test]
    fn process_returns_typed_yin_and_measured_audio_levels() {
        let mut detector = yin_detector();
        let outcome = detector.process(&vec![0.0; 128]).unwrap();

        assert_eq!(outcome.rms_dbfs, -120.0);
        assert_eq!(outcome.peak, 0.0);
        assert!(!outcome.onset_detected);
        assert_eq!(outcome.selected_note, None);
        assert!(outcome.note_decisions.is_empty());
        assert_eq!(outcome.onset_to_note_latency, None);
        assert!(outcome.yin.is_some());
        assert!(outcome.spectral.is_none());
    }

    #[test]
    fn note_changes_return_ordered_actions_and_onset_latency() {
        let mut detector = yin_detector();
        let onset = Instant::now();

        let (first_actions, first_latency) = detector.send_new_note(48, Some(onset));
        assert_eq!(first_actions, vec![NoteDecision::NoteOn { note: 48 }]);
        assert!(first_latency.is_some());

        let (change_actions, change_latency) = detector.send_new_note(49, Some(Instant::now()));
        assert_eq!(
            change_actions,
            vec![
                NoteDecision::NoteOff { note: 48 },
                NoteDecision::NoteOn { note: 49 },
            ]
        );
        assert!(change_latency.is_some());
        assert_eq!(
            detector.shutdown(),
            vec![NoteDecision::NoteOff { note: 49 }]
        );
    }

    #[test]
    fn spectral_result_exposes_real_classification_values_and_matches() {
        let result = SpectralResult::from_classification(Classification {
            note: 48,
            score: 0.91,
            second_score: 0.82,
            margin: 0.09,
            accepted: true,
            ranked_matches: vec![
                RankedMatch {
                    note: 48,
                    score: 0.91,
                },
                RankedMatch {
                    note: 49,
                    score: 0.82,
                },
            ],
        });

        assert_eq!(result.note, 48);
        assert_eq!(result.confidence, 0.91);
        assert_eq!(result.second_score, 0.82);
        assert_eq!(result.margin, 0.09);
        assert!(result.accepted);
        assert_eq!(result.selected_note, Some(48));
        assert_eq!(result.ranked_matches.len(), 2);
    }
}
