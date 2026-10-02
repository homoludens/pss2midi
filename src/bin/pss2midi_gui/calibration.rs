//! Pure Calibration-page projection and command mapping.

use std::collections::BTreeSet;

use pss2midi::{
    engine::{
        calibration::{CalibrationProgress, CalibrationSampleQuality, DEFAULT_SAMPLES_PER_NOTE},
        EngineCommand,
    },
    ui::state::{
        AudioLevels, CalibrationCompletion, CalibrationState, CalibrationStatus,
        CALIBRATION_NOTE_COUNT,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CalibrationAction {
    Start,
    Retry,
    Cancel,
}

pub(super) fn command_for_action(action: CalibrationAction) -> EngineCommand {
    match action {
        CalibrationAction::Start => EngineCommand::BeginCalibration {
            samples_per_note: DEFAULT_SAMPLES_PER_NOTE,
        },
        CalibrationAction::Retry => EngineCommand::RetryCalibrationSample,
        CalibrationAction::Cancel => EngineCommand::CancelCalibration,
    }
}

/// Display values projected only from reducer-owned calibration and audio-level state.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CalibrationViewModel {
    pub(super) status: CalibrationStatus,
    pub(super) progress: Option<CalibrationProgress>,
    pub(super) last_sample_result: Option<CalibrationProgress>,
    pub(super) completed_notes: BTreeSet<u8>,
    pub(super) completion: Option<CalibrationCompletion>,
    pub(super) error_message: Option<String>,
    pub(super) live_levels: Option<AudioLevels>,
    pub(super) accepted_samples_for_note: usize,
    pub(super) accepted_samples: usize,
    pub(super) required_samples: usize,
    pub(super) samples_per_note: usize,
}

impl CalibrationViewModel {
    pub(super) fn from_calibration(
        calibration: &CalibrationState,
        live_levels: Option<AudioLevels>,
    ) -> Self {
        let progress = calibration.progress.clone();
        let completion = calibration.completion.clone();
        let samples_per_note = progress
            .as_ref()
            .map(|progress| progress.samples_per_note)
            .unwrap_or(DEFAULT_SAMPLES_PER_NOTE);

        Self {
            status: calibration.status,
            accepted_samples_for_note: progress
                .as_ref()
                .map(|progress| progress.accepted_samples_for_note)
                .unwrap_or_default(),
            accepted_samples: progress
                .as_ref()
                .map(|progress| progress.accepted_samples)
                .or_else(|| {
                    completion
                        .as_ref()
                        .map(|completion| completion.sample_count)
                })
                .unwrap_or_default(),
            required_samples: progress
                .as_ref()
                .map(|progress| progress.required_samples)
                .unwrap_or(CALIBRATION_NOTE_COUNT * DEFAULT_SAMPLES_PER_NOTE),
            samples_per_note,
            progress,
            last_sample_result: calibration.last_sample_result.clone(),
            completed_notes: calibration.completed_notes.clone(),
            completion,
            error_message: calibration.error_message.clone(),
            live_levels,
        }
    }

    pub(super) fn is_running(&self) -> bool {
        self.status == CalibrationStatus::Running
    }

    pub(super) fn can_start(&self) -> bool {
        matches!(
            self.status,
            CalibrationStatus::Idle | CalibrationStatus::Cancelled | CalibrationStatus::Error
        )
    }

    pub(super) fn can_retry_or_cancel(&self) -> bool {
        self.is_running() && self.progress.is_some()
    }

    pub(super) fn current_note(&self) -> Option<u8> {
        (self.status == CalibrationStatus::Running)
            .then(|| {
                self.progress
                    .as_ref()
                    .map(|progress| progress.requested_note)
            })
            .flatten()
    }
}

pub(super) fn sample_quality_label(quality: &CalibrationSampleQuality) -> &'static str {
    match quality {
        CalibrationSampleQuality::AwaitingOnset => "Waiting for onset",
        CalibrationSampleQuality::Accepted => "Accepted",
        CalibrationSampleQuality::Rejected(reason) => match reason {
            pss2midi::engine::calibration::CalibrationRejectionReason::TooQuiet => "Too quiet",
            pss2midi::engine::calibration::CalibrationRejectionReason::Clipped => "Clipped",
            pss2midi::engine::calibration::CalibrationRejectionReason::Invalid => "Invalid",
        },
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use pss2midi::{
        engine::{
            calibration::{
                CalibrationProgress, CalibrationRejectionReason, CalibrationSampleQuality,
            },
            EngineCommand, EngineEvent,
        },
        ui::state::{AppState, CalibrationStatus},
    };

    use super::{command_for_action, CalibrationAction, CalibrationViewModel};

    fn progress(
        sample_index: usize,
        accepted_samples_for_note: usize,
        accepted_samples: usize,
        quality: CalibrationSampleQuality,
    ) -> CalibrationProgress {
        CalibrationProgress {
            requested_note: 36,
            sample_index,
            samples_per_note: 5,
            accepted_samples_for_note,
            accepted_samples,
            required_samples: 185,
            completed_notes: 0,
            rms_dbfs: Some(-22.0),
            peak: Some(0.42),
            quality,
        }
    }

    #[test]
    fn command_actions_use_default_samples_and_existing_calibration_commands() {
        assert_eq!(
            command_for_action(CalibrationAction::Start),
            EngineCommand::BeginCalibration {
                samples_per_note: 5
            }
        );
        assert_eq!(
            command_for_action(CalibrationAction::Retry),
            EngineCommand::RetryCalibrationSample
        );
        assert_eq!(
            command_for_action(CalibrationAction::Cancel),
            EngineCommand::CancelCalibration
        );
    }

    #[test]
    fn calibration_view_model_retains_accepted_rejected_retry_and_cancel_states() {
        let mut state = AppState::default();
        let start = Instant::now();
        state.reduce_event(
            EngineEvent::CalibrationProgress(progress(
                1,
                0,
                0,
                CalibrationSampleQuality::AwaitingOnset,
            )),
            start,
        );

        state.reduce_event(
            EngineEvent::CalibrationProgress(progress(1, 1, 1, CalibrationSampleQuality::Accepted)),
            start + Duration::from_millis(1),
        );
        state.reduce_event(
            EngineEvent::CalibrationProgress(progress(
                2,
                1,
                1,
                CalibrationSampleQuality::AwaitingOnset,
            )),
            start + Duration::from_millis(2),
        );
        let accepted = CalibrationViewModel::from_calibration(&state.calibration, None);
        assert_eq!(
            &accepted.last_sample_result.as_ref().unwrap().quality,
            &CalibrationSampleQuality::Accepted
        );
        assert_eq!(accepted.progress.as_ref().unwrap().sample_index, 2);
        assert_eq!(accepted.accepted_samples_for_note, 1);
        assert!(accepted.can_retry_or_cancel());

        state.reduce_event(
            EngineEvent::CalibrationProgress(progress(
                2,
                1,
                1,
                CalibrationSampleQuality::Rejected(CalibrationRejectionReason::TooQuiet),
            )),
            start + Duration::from_millis(3),
        );
        state.reduce_event(
            EngineEvent::CalibrationProgress(progress(
                2,
                1,
                1,
                CalibrationSampleQuality::AwaitingOnset,
            )),
            start + Duration::from_millis(4),
        );
        let retried = CalibrationViewModel::from_calibration(&state.calibration, None);
        assert_eq!(retried.status, CalibrationStatus::Running);
        assert_eq!(retried.progress.as_ref().unwrap().sample_index, 2);
        assert_eq!(retried.accepted_samples_for_note, 1);
        assert_eq!(
            retried.last_sample_result.as_ref().unwrap().quality,
            CalibrationSampleQuality::Rejected(CalibrationRejectionReason::TooQuiet)
        );
        assert!(retried.can_retry_or_cancel());
        assert_eq!(
            command_for_action(CalibrationAction::Retry),
            EngineCommand::RetryCalibrationSample
        );

        state.reduce_event(
            EngineEvent::CalibrationCancelled,
            start + Duration::from_millis(5),
        );
        let cancelled = CalibrationViewModel::from_calibration(&state.calibration, None);
        assert_eq!(cancelled.status, CalibrationStatus::Cancelled);
        assert!(!cancelled.can_retry_or_cancel());
        assert!(cancelled.can_start());
        assert_eq!(cancelled.current_note(), None);
        assert_eq!(
            cancelled.last_sample_result.as_ref().unwrap().quality,
            CalibrationSampleQuality::Rejected(CalibrationRejectionReason::TooQuiet)
        );
    }
}
