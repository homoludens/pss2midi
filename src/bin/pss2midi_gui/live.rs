//! Pure projection of engine-backed app state for the Live dashboard.

use std::time::Duration;

use pss2midi::{
    engine::{
        config::DetectorMode,
        detector::{RankedMatch, SpectralResult, YinResult},
        note::note_name,
    },
    ui::state::{AppState, AudioLevels},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NoteReading {
    pub(super) midi_note: u8,
    pub(super) name: String,
}

impl NoteReading {
    fn new(midi_note: u8) -> Self {
        Self {
            midi_note,
            name: note_name(midi_note),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DetectorAgreement {
    Waiting,
    Agree { note: u8 },
    Disagree { yin_note: u8, spectral_note: u8 },
}

/// Display values derived from the reducer's latest engine-backed state.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LiveViewModel {
    pub(super) dominant_note: Option<NoteReading>,
    pub(super) latency: Option<Duration>,
    pub(super) levels: Option<AudioLevels>,
    pub(super) yin_result: Option<YinResult>,
    pub(super) spectral_result: Option<SpectralResult>,
    pub(super) ranked_matches: Vec<RankedMatch>,
    pub(super) agreement: DetectorAgreement,
    pub(super) keyboard_selected_note: Option<u8>,
    pub(super) keyboard_yin_note: Option<u8>,
    pub(super) yin_is_output: bool,
    pub(super) spectral_is_output: bool,
}

impl LiveViewModel {
    pub(super) fn from_state(state: &AppState) -> Self {
        let yin_result = state.yin_result.clone();
        let spectral_result = state.spectral_result.clone();
        let yin_note = yin_result.as_ref().and_then(|result| result.note);
        let spectral_note = spectral_result.as_ref().map(|result| result.note);
        let agreement = match (yin_note, spectral_note) {
            (Some(yin_note), Some(spectral_note)) if yin_note == spectral_note => {
                DetectorAgreement::Agree { note: yin_note }
            }
            (Some(yin_note), Some(spectral_note)) => DetectorAgreement::Disagree {
                yin_note,
                spectral_note,
            },
            _ => DetectorAgreement::Waiting,
        };
        let ranked_matches = spectral_result
            .as_ref()
            .map(|result| result.ranked_matches.iter().take(3).cloned().collect())
            .unwrap_or_default();
        let mode = state.config.detector_mode;

        Self {
            dominant_note: state.selected_note.map(NoteReading::new),
            latency: state.onset_to_note_latency,
            levels: state.audio_levels,
            yin_result,
            spectral_result,
            ranked_matches,
            agreement,
            keyboard_selected_note: state.selected_note.filter(|note| (36..=72).contains(note)),
            keyboard_yin_note: yin_note.filter(|note| (36..=72).contains(note)),
            yin_is_output: mode == DetectorMode::Yin,
            spectral_is_output: mode != DetectorMode::Yin,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant, SystemTime};

    use pss2midi::{
        engine::{
            app_config::AppConfig,
            config::DetectorMode,
            detector::{RankedMatch, SpectralResult, YinDecision, YinResult},
            templates::RankedMatch as TemplateRankedMatch,
            EngineEvent,
        },
        ui::state::{AppState, AudioLevels},
    };

    use super::{DetectorAgreement, LiveViewModel, NoteReading};

    #[test]
    fn empty_state_projects_unavailable_values_without_inventing_results() {
        let view = LiveViewModel::from_state(&AppState::default());

        assert_eq!(view.dominant_note, None);
        assert_eq!(view.latency, None);
        assert_eq!(view.levels, None);
        assert_eq!(view.yin_result, None);
        assert_eq!(view.spectral_result, None);
        assert!(view.ranked_matches.is_empty());
        assert_eq!(view.agreement, DetectorAgreement::Waiting);
        assert_eq!(view.keyboard_selected_note, None);
        assert_eq!(view.keyboard_yin_note, None);
    }

    #[test]
    fn engine_events_populate_and_update_live_dashboard_values() {
        let mut config = AppConfig::default();
        config.detector_mode = DetectorMode::Compare;
        let mut state = AppState::new(config);
        let now = Instant::now();
        let wall_clock = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let yin = YinResult {
            midi_pitch: Some(60.02),
            note: Some(60),
            cents: Some(2.0),
            decision: Some(YinDecision {
                candidate_note: Some(60),
                accepted: true,
                vote_count: 4,
                total_votes: 5,
                vote_ratio: Some(0.8),
            }),
        };
        let matches = vec![
            RankedMatch {
                note: 62,
                score: 0.91,
            },
            RankedMatch {
                note: 60,
                score: 0.73,
            },
            RankedMatch {
                note: 67,
                score: 0.52,
            },
            RankedMatch {
                note: 55,
                score: 0.31,
            },
        ];
        let spectral = SpectralResult {
            note: 62,
            confidence: 0.91,
            second_score: 0.73,
            margin: 0.18,
            accepted: true,
            selected_note: Some(62),
            ranked_matches: matches.clone(),
        };
        let latency = Duration::from_micros(34_500);

        state.reduce_event_at(
            EngineEvent::Detection {
                selected_note: Some(62),
                yin: Some(yin.clone()),
                spectral: Some(spectral.clone()),
                onset_to_note_latency: Some(latency),
            },
            now,
            wall_clock,
        );
        state.reduce_event_at(
            EngineEvent::AudioLevel {
                rms_dbfs: -18.2,
                peak_dbfs: -1.5,
            },
            now + Duration::from_millis(40),
            wall_clock,
        );

        let view = LiveViewModel::from_state(&state);
        assert_eq!(
            view.dominant_note,
            Some(NoteReading {
                midi_note: 62,
                name: "D4".to_owned(),
            })
        );
        assert_eq!(view.latency, Some(latency));
        assert_eq!(
            view.levels,
            Some(AudioLevels {
                rms_dbfs: -18.2,
                peak_dbfs: -1.5,
            })
        );
        assert_eq!(view.yin_result, Some(yin));
        assert_eq!(view.spectral_result, Some(spectral));
        assert_eq!(view.ranked_matches, matches[..3]);
        assert_eq!(
            view.agreement,
            DetectorAgreement::Disagree {
                yin_note: 60,
                spectral_note: 62,
            }
        );
        assert_eq!(view.keyboard_selected_note, Some(62));
        assert_eq!(view.keyboard_yin_note, Some(60));
        assert!(!view.yin_is_output);
        assert!(view.spectral_is_output);

        state.reduce_event_at(
            EngineEvent::Detection {
                selected_note: Some(60),
                yin: Some(YinResult {
                    midi_pitch: Some(60.0),
                    note: Some(60),
                    cents: Some(0.0),
                    decision: None,
                }),
                spectral: Some(SpectralResult {
                    note: 60,
                    confidence: 0.88,
                    second_score: 0.61,
                    margin: 0.27,
                    accepted: true,
                    selected_note: Some(60),
                    ranked_matches: vec![TemplateRankedMatch {
                        note: 60,
                        score: 0.88,
                    }],
                }),
                onset_to_note_latency: None,
            },
            now + Duration::from_millis(41),
            wall_clock,
        );
        let updated = LiveViewModel::from_state(&state);
        assert_eq!(updated.agreement, DetectorAgreement::Agree { note: 60 });
        assert_eq!(updated.keyboard_selected_note, Some(60));
        assert_eq!(updated.keyboard_yin_note, Some(60));
        assert_eq!(updated.latency, None);
    }
}
