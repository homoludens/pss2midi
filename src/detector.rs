use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{ensure, Result};
use aubio::{Pitch, PitchMode, PitchUnit};

use crate::{
    config::{default_template_path, DetectorMode, RunArgs},
    features::{FeatureExtractor, SampleCapture, SampleHistory},
    midi::Midi,
    note::{ms_to_hops, note_name, quantize_pitch, rms_db, MIN_MIDI, NOTE_COUNT},
    onset::OnsetDetector,
    templates::{validate_for_extractor, Classification, TemplateFile},
};

const FALLBACK_ATTACK_MARGIN_DB: f32 = 10.0;

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
}

struct PendingSpectral {
    onset_at: Instant,
    capture: SampleCapture,
    yin_ready: bool,
    yin_note: Option<u8>,
    classification: Option<Classification>,
}

#[derive(Default)]
struct CompareStats {
    events: u64,
    agree: u64,
    disagree: u64,
    spectral_rejected: u64,
    confusion: BTreeMap<(u8, u8), u64>,
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
    debug: bool,
    started_at: Instant,
    compare_stats: CompareStats,
}

impl Detector {
    pub fn new(args: &RunArgs, sample_rate: u32, hop: usize) -> Result<Self> {
        if args.detector != DetectorMode::Yin {
            ensure!(
                args.spectral.spectral_delay_ms.is_finite()
                    && args.spectral.spectral_delay_ms >= 0.0,
                "--spectral-delay-ms must be a finite non-negative number"
            );
            ensure!(
                args.spectral.spectral_min_score.is_finite()
                    && (0.0..=1.0).contains(&args.spectral.spectral_min_score),
                "--spectral-min-score must be between 0 and 1"
            );
            ensure!(
                args.spectral.spectral_min_margin.is_finite()
                    && (0.0..=1.0).contains(&args.spectral.spectral_min_margin),
                "--spectral-min-margin must be between 0 and 1"
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
            let default_path;
            let path = if let Some(path) = args.templates.as_deref() {
                path
            } else {
                default_path = default_template_path();
                default_path.as_path()
            };
            let templates = TemplateFile::load(path)?;
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
            debug: args.debug,
            started_at: Instant::now(),
            compare_stats: CompareStats::default(),
        })
    }

    fn debug(&self, message: impl AsRef<str>) {
        if self.debug {
            println!(
                "[{:10.2} ms] {}",
                self.started_at.elapsed().as_secs_f64() * 1000.0,
                message.as_ref()
            );
        }
    }

    fn clear_fallback(&mut self) {
        self.fallback_note = None;
        self.fallback_count = 0;
    }

    fn send_new_note(
        &mut self,
        note: u8,
        onset_time: Option<Instant>,
        midi: &mut Midi,
    ) -> Result<()> {
        let now = Instant::now();
        match self.current_note {
            None => {
                midi.note_on(note)?;
                self.current_note = Some(note);
                self.last_trigger = Some(now);
            }
            Some(current) if current == note => {
                let Some(onset_time) = onset_time else {
                    return Ok(());
                };
                let cooldown_ok = self
                    .last_trigger
                    .map(|last| now.duration_since(last) >= self.retrigger_time)
                    .unwrap_or(true);
                if !cooldown_ok {
                    return Ok(());
                }
                midi.note_off(note)?;
                midi.note_on(note)?;
                self.last_trigger = Some(now);
                self.debug(format!(
                    "same-note retrigger {} ; onset->MIDI {:.1} ms",
                    note_name(note),
                    now.duration_since(onset_time).as_secs_f64() * 1000.0
                ));
            }
            Some(current) => {
                midi.note_off(current)?;
                midi.note_on(note)?;
                self.current_note = Some(note);
                self.last_trigger = Some(now);
                if let Some(onset_time) = onset_time {
                    self.debug(format!(
                        "change {} -> {} ; onset->MIDI {:.1} ms",
                        note_name(current),
                        note_name(note),
                        now.duration_since(onset_time).as_secs_f64() * 1000.0
                    ));
                }
            }
        }
        Ok(())
    }

    fn release(&mut self, midi: &mut Midi) -> Result<()> {
        if let Some(note) = self.current_note.take() {
            midi.note_off(note)?;
        }
        self.pending_yin = None;
        if self.mode == DetectorMode::Compare {
            if let Some(pending) = self.pending_spectral.as_mut() {
                pending.yin_ready = true;
                pending.yin_note = None;
            }
        }
        if self.mode == DetectorMode::Yin {
            self.pending_spectral = None;
        }
        self.clear_fallback();
        Ok(())
    }

    pub fn process(&mut self, frame: &[f32], midi: &mut Midi) -> Result<()> {
        let frame_start = self.sample_index;
        self.sample_index += frame.len() as u64;
        if self.mode != DetectorMode::Yin {
            self.history.push_frame(frame_start, frame);
        }
        let now = Instant::now();
        let level = rms_db(frame);
        let pitch_value = if let Some(pitch) = self.pitch.as_mut() {
            pitch.do_result(frame)?
        } else {
            -1.0
        };
        let detected = quantize_pitch(pitch_value);
        let onset_detected = self.onset.detect(frame)?;

        if self.mode == DetectorMode::Yin {
            return self.process_yin(level, pitch_value, detected, onset_detected, now, midi);
        }

        let is_silent = level < self.silence_db;
        if is_silent {
            self.silence_count += 1;
            self.clear_fallback();
            if self.silence_count >= self.release_hops {
                self.release(midi)?;
            }
            self.debug(format!("level={level:6.1} dB silence"));
        } else {
            self.silence_count = 0;
        }

        let new_attack = onset_detected && !is_silent;
        if new_attack {
            let capture = SampleCapture::new(
                frame_start + self.spectral_delay_samples,
                self.spectral_window_samples,
            );
            self.pending_spectral = Some(PendingSpectral {
                onset_at: now,
                capture,
                yin_ready: false,
                yin_note: None,
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
            self.debug(format!("ONSET level={level:.1} dB pitch={pitch_value:.2}"));
        }

        if self.mode == DetectorMode::Compare {
            if let Some(yin_result) = self.advance_yin_decision(detected) {
                if let Some(pending) = self.pending_spectral.as_mut() {
                    pending.yin_ready = true;
                    pending.yin_note = yin_result;
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
                            .classify(
                                &features,
                                self.spectral_min_score,
                                self.spectral_min_margin,
                            )?,
                    );
                }
            }

            let compare_waiting_for_yin = self.mode == DetectorMode::Compare && !pending.yin_ready;
            if let Some(classification) = pending.classification {
                if compare_waiting_for_yin {
                    pending.classification = Some(classification);
                    self.pending_spectral = Some(pending);
                } else {
                    self.finish_spectral_decision(pending, classification, midi)?;
                }
            } else {
                self.pending_spectral = Some(pending);
            }
        }

        Ok(())
    }

    fn process_yin(
        &mut self,
        level: f32,
        midi_float: f32,
        detected: Option<(u8, f32)>,
        onset_detected: bool,
        now: Instant,
        midi: &mut Midi,
    ) -> Result<()> {
        if level < self.silence_db {
            self.silence_count += 1;
            self.clear_fallback();
            if self.silence_count >= self.release_hops {
                self.release(midi)?;
            }
            self.debug(format!("level={level:6.1} dB silence"));
            return Ok(());
        }
        self.silence_count = 0;

        if onset_detected {
            self.pending_yin = Some(PendingDecision::new(
                now,
                self.attack_ignore_hops,
                self.decision_hops,
                self.decision_extension_hops,
            ));
            self.clear_fallback();
            self.debug(format!("ONSET level={level:.1} dB pitch={midi_float:.2}"));
        }

        if let Some(mut pending) = self.pending_yin.take() {
            if pending.ignore_left > 0 {
                pending.ignore_left -= 1;
                self.pending_yin = Some(pending);
                self.debug(format!("attack-ignore pitch={midi_float:.2}"));
                return Ok(());
            }
            if let Some((note, cents)) = detected {
                pending.vote(note);
                self.debug(format!("vote {} ({cents:+.1}c)", note_name(note)));
            }
            pending.collect_left = pending.collect_left.saturating_sub(1);
            if pending.collect_left > 0 {
                self.pending_yin = Some(pending);
                return Ok(());
            }

            let decision = pending.winner();
            let onset_at = pending.onset_at;
            if let Some((winner, count, ratio)) = decision {
                self.debug(format!(
                    "decision {} votes={count} ratio={ratio:.2}",
                    note_name(winner)
                ));
                if count >= 2 && ratio >= self.vote_ratio {
                    self.send_new_note(winner, Some(onset_at), midi)?;
                    self.clear_fallback();
                    return Ok(());
                }
            }
            if pending.extensions_left > 0 {
                pending.extensions_left -= 1;
                pending.collect_left = pending.extension_hops;
                self.pending_yin = Some(pending);
                self.debug("ambiguous onset; extending decision window");
                return Ok(());
            }
            self.debug("onset decision rejected");
            return Ok(());
        }

        let Some((note, cents)) = detected else {
            self.clear_fallback();
            return Ok(());
        };
        if let Some(current) = self.current_note {
            self.clear_fallback();
            if current != note {
                self.debug(format!(
                    "ignoring pitch change without onset: {} -> {} ({cents:+.1}c)",
                    note_name(current),
                    note_name(note)
                ));
            }
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
        self.debug(format!(
            "initial fallback {} {}/{} ({cents:+.1}c)",
            note_name(note),
            self.fallback_count,
            self.initial_stable
        ));
        if self.fallback_count >= self.initial_stable {
            self.send_new_note(note, None, midi)?;
            self.clear_fallback();
        }
        Ok(())
    }

    /// Runs only as a diagnostic companion in compare mode.
    /// It shares the existing YIN onset vote rules and never drives MIDI.
    fn advance_yin_decision(&mut self, detected: Option<(u8, f32)>) -> Option<Option<u8>> {
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
        if let Some((note, count, ratio)) = pending.winner() {
            if count >= 2 && ratio >= self.vote_ratio {
                return Some(Some(note));
            }
        }
        if pending.extensions_left > 0 {
            pending.extensions_left -= 1;
            pending.collect_left = pending.extension_hops;
            self.pending_yin = Some(pending);
            return None;
        }
        Some(None)
    }

    fn finish_spectral_decision(
        &mut self,
        pending: PendingSpectral,
        classification: Classification,
        midi: &mut Midi,
    ) -> Result<()> {
        let spectral_note = classification.accepted.then_some(classification.note);
        let released_before_result =
            self.current_note.is_none() && self.silence_count >= self.release_hops;
        if self.mode == DetectorMode::Compare {
            self.compare_stats.events += 1;
            if !classification.accepted {
                self.compare_stats.spectral_rejected += 1;
            }
            if pending.yin_ready && pending.yin_note == spectral_note {
                self.compare_stats.agree += 1;
            } else {
                self.compare_stats.disagree += 1;
                if let (Some(yin), Some(spectral)) = (pending.yin_note, spectral_note) {
                    *self
                        .compare_stats
                        .confusion
                        .entry((yin, spectral))
                        .or_default() += 1;
                }
            }

            let yin_display = if pending.yin_ready {
                pending
                    .yin_note
                    .map(|note| format!("{}({note})", note_name(note)))
                    .unwrap_or_else(|| "rejected".to_owned())
            } else {
                "pending/none".to_owned()
            };
            let spectral_display = if classification.accepted {
                format!(
                    "{}({})",
                    note_name(classification.note),
                    classification.note
                )
            } else {
                "rejected".to_owned()
            };
            let result_display = if released_before_result {
                "suppressed: silence".to_owned()
            } else {
                spectral_note
                    .map(|note| format!("{}({note})", note_name(note)))
                    .unwrap_or_else(|| "none".to_owned())
            };
            println!(
                "[{:10.2} ms] compare: YIN={yin_display} SPECTRAL={spectral_display} score={:.3} second={:.3} margin={:.3} AGREE={} RESULT={result_display}",
                self.started_at.elapsed().as_secs_f64() * 1000.0,
                classification.score,
                classification.second_score,
                classification.margin,
                pending.yin_ready && pending.yin_note == spectral_note,
            );
        }

        self.debug(format!(
            "spectral: best={}({}) score={:.3} second={:.3} margin={:.3} {}",
            note_name(classification.note),
            classification.note,
            classification.score,
            classification.second_score,
            classification.margin,
            if classification.accepted {
                "accepted"
            } else {
                "rejected: below score or ambiguous"
            },
        ));
        if released_before_result {
            self.debug("discarding spectral result after sustained silence");
            return Ok(());
        }
        if let Some(note) = spectral_note {
            self.send_new_note(note, Some(pending.onset_at), midi)?;
        }
        Ok(())
    }

    pub fn shutdown(&mut self, midi: &mut Midi) -> Result<()> {
        self.release(midi)?;
        if self.mode == DetectorMode::Compare {
            println!(
                "Compare summary: events={} agree={} disagree={} spectral_rejected={}",
                self.compare_stats.events,
                self.compare_stats.agree,
                self.compare_stats.disagree,
                self.compare_stats.spectral_rejected,
            );
            for ((yin, spectral), count) in &self.compare_stats.confusion {
                println!(
                    "  YIN {} -> Spectral {}: {} times",
                    note_name(*yin),
                    note_name(*spectral),
                    count
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
