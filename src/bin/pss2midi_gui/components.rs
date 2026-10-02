//! Reusable native GPUI components for the desktop views.

use std::{collections::VecDeque, time::SystemTime};

use gpui::{div, prelude::*, px, relative, rgb, FontWeight};
use pss2midi::{
    engine::detector::{RankedMatch, SpectralResult, YinResult},
    ui::state::RecentEvent,
};

use crate::theme;

const METER_FLOOR_DBFS: f32 = -60.0;
const MAX_VISIBLE_EVENTS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StatusTone {
    Neutral,
    Accent,
    Success,
    Warning,
    Error,
}

/// Compact semantic status indicator shared by the app's views.
pub(crate) struct StatusBadge {
    label: String,
    tone: StatusTone,
}

impl StatusBadge {
    pub(crate) fn new(label: impl Into<String>, tone: StatusTone) -> Self {
        Self {
            label: label.into(),
            tone,
        }
    }

    pub(crate) fn render(&self) -> impl IntoElement {
        let (foreground, background) = match self.tone {
            StatusTone::Neutral => (theme::TEXT_SECONDARY, theme::PANEL_INSET),
            StatusTone::Accent => (theme::ACCENT, theme::ACCENT_TINT),
            StatusTone::Success => (theme::SUCCESS, theme::SUCCESS_TINT),
            StatusTone::Warning => (theme::WARNING, theme::WARNING_TINT),
            StatusTone::Error => (theme::ERROR, theme::ERROR_TINT),
        };

        div()
            .flex_none()
            .rounded_full()
            .bg(rgb(background))
            .px(px(theme::SPACE_SM))
            .py(px(theme::SPACE_XS))
            .text_size(px(theme::FONT_CAPTION))
            .text_color(rgb(foreground))
            .font_weight(FontWeight::SEMIBOLD)
            .child(self.label.clone())
    }
}

/// Displays an optional dBFS measurement and its level on a -60..0 dBFS scale.
pub(crate) struct LevelMeter {
    label: String,
    value_dbfs: Option<f32>,
}

impl LevelMeter {
    pub(crate) fn new(label: impl Into<String>, value_dbfs: Option<f32>) -> Self {
        Self {
            label: label.into(),
            value_dbfs,
        }
    }

    pub(crate) fn render(&self) -> impl IntoElement {
        let value = self.value_dbfs.filter(|value| value.is_finite());
        let fill = value
            .map(|value| ((value - METER_FLOOR_DBFS) / -METER_FLOOR_DBFS).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        let reading = value
            .map(|value| format!("{value:.1} dBFS"))
            .unwrap_or_else(|| "Unavailable".to_owned());

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_XS))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(rgb(theme::TEXT_SECONDARY))
                            .child(self.label.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(rgb(theme::TEXT_PRIMARY))
                            .child(reading),
                    ),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(7.0))
                    .overflow_hidden()
                    .rounded_full()
                    .bg(rgb(theme::PANEL_INSET))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(relative(fill))
                            .rounded_full()
                            .bg(rgb(theme::ACCENT)),
                    ),
            )
    }
}

/// Displays an optional normalized detector score with its exact reading.
pub(crate) struct ScoreMeter {
    label: String,
    value: Option<f32>,
}

impl ScoreMeter {
    pub(crate) fn new(label: impl Into<String>, value: Option<f32>) -> Self {
        Self {
            label: label.into(),
            value,
        }
    }

    pub(crate) fn render(&self) -> impl IntoElement {
        let value = self.value.filter(|value| value.is_finite());
        let fill = value.map(|value| value.clamp(0.0, 1.0)).unwrap_or(0.0);
        let reading = value
            .map(|value| format!("{value:.3} · {:.0}%", value * 100.0))
            .unwrap_or_else(|| "Unavailable".to_owned());

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_XS))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SPACE_SM))
                    .child(
                        div()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(rgb(theme::TEXT_SECONDARY))
                            .child(self.label.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(rgb(theme::TEXT_PRIMARY))
                            .child(reading),
                    ),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(7.0))
                    .overflow_hidden()
                    .rounded_full()
                    .bg(rgb(theme::PANEL_INSET))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(relative(fill))
                            .rounded_full()
                            .bg(rgb(theme::ACCENT)),
                    ),
            )
    }
}

/// Detector result card constructors keep YIN and spectral details consistent.
pub(crate) struct DetectorCard;

impl DetectorCard {
    pub(crate) fn yin(result: Option<&YinResult>, authoritative: bool) -> impl IntoElement {
        let (headline, detail, state_label, state_tone) = match result {
            None => (
                "No reading".to_owned(),
                "Waiting for a YIN result".to_owned(),
                "WAITING",
                StatusTone::Neutral,
            ),
            Some(result) => {
                let headline = result
                    .note
                    .map(|note| {
                        format!("{} · MIDI {note}", pss2midi::engine::note::note_name(note))
                    })
                    .unwrap_or_else(|| {
                        result
                            .midi_pitch
                            .map(|pitch| format!("Pitch {pitch:.1}"))
                            .unwrap_or_else(|| "No stable note".to_owned())
                    });
                let detail = result
                    .cents
                    .map(|cents| format!("{cents:+.0} cents from equal temperament"))
                    .or_else(|| {
                        result
                            .decision
                            .as_ref()
                            .and_then(|decision| decision.vote_ratio)
                            .map(|ratio| format!("Onset vote {:.0}%", ratio * 100.0))
                    })
                    .unwrap_or_else(|| "Pitch tracking".to_owned());
                let (label, tone) = match result.decision.as_ref() {
                    Some(decision) if decision.accepted => ("ACCEPTED", StatusTone::Success),
                    Some(_) => ("REJECTED", StatusTone::Warning),
                    None => ("TRACKING", StatusTone::Neutral),
                };
                (headline, detail, label, tone)
            }
        };

        detector_card(
            "YIN",
            headline,
            detail,
            state_label,
            state_tone,
            authoritative,
        )
    }

    pub(crate) fn spectral(
        result: Option<&SpectralResult>,
        authoritative: bool,
    ) -> impl IntoElement {
        let (headline, detail, state_label, state_tone) = match result {
            None => (
                "No classification".to_owned(),
                "Waiting for a spectral result".to_owned(),
                "WAITING",
                StatusTone::Neutral,
            ),
            Some(result) => {
                let headline = format!(
                    "{} · MIDI {}",
                    pss2midi::engine::note::note_name(result.note),
                    result.note
                );
                let detail = format!(
                    "Confidence {:.3} · margin {:.3}",
                    result.confidence, result.margin
                );
                if result.accepted {
                    (headline, detail, "ACCEPTED", StatusTone::Success)
                } else {
                    (headline, detail, "BELOW THRESHOLD", StatusTone::Warning)
                }
            }
        };

        detector_card(
            "SPECTRAL",
            headline,
            detail,
            state_label,
            state_tone,
            authoritative,
        )
    }
}

fn detector_card(
    title: &'static str,
    headline: String,
    detail: String,
    state_label: &'static str,
    state_tone: StatusTone,
    authoritative: bool,
) -> impl IntoElement {
    let mut status = div()
        .flex()
        .items_center()
        .gap(px(theme::SPACE_XS))
        .child(StatusBadge::new(state_label, state_tone).render());
    if authoritative {
        status = status.child(StatusBadge::new("OUTPUT", StatusTone::Accent).render());
    }

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_SM))
        .p(px(theme::SPACE_MD))
        .rounded_md()
        .bg(rgb(theme::PANEL))
        .border_1()
        .border_color(rgb(if authoritative {
            theme::ACCENT
        } else {
            theme::BORDER
        }))
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(status),
        )
        .child(
            div()
                .text_size(px(theme::FONT_BODY))
                .text_color(rgb(theme::TEXT_PRIMARY))
                .font_weight(FontWeight::SEMIBOLD)
                .child(headline),
        )
        .child(
            div()
                .text_size(px(theme::FONT_SMALL))
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(detail),
        )
}

/// One ranked spectral candidate with an optional selected-note emphasis.
pub(crate) struct SpectralMatchRow {
    rank: usize,
    candidate: RankedMatch,
    selected: bool,
}

impl SpectralMatchRow {
    pub(crate) fn new(rank: usize, candidate: RankedMatch, selected_note: Option<u8>) -> Self {
        Self {
            rank,
            candidate,
            selected: selected_note == Some(candidate.note),
        }
    }

    pub(crate) fn render(&self) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(theme::SPACE_SM))
            .px(px(theme::SPACE_SM))
            .py(px(theme::SPACE_XS))
            .rounded_sm()
            .bg(rgb(if self.selected {
                theme::ACCENT_TINT
            } else {
                theme::PANEL
            }))
            .child(
                div()
                    .w(px(22.0))
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(format!("{:02}", self.rank)),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(theme::FONT_SMALL))
                    .text_color(rgb(if self.selected {
                        theme::ACCENT
                    } else {
                        theme::TEXT_PRIMARY
                    }))
                    .font_weight(if self.selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .child(format!(
                        "{} · MIDI {}",
                        pss2midi::engine::note::note_name(self.candidate.note),
                        self.candidate.note
                    )),
            )
            .child(
                div()
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .child(format!("{:.3}", self.candidate.score)),
            )
    }
}

/// Compact, timestamped view of the latest semantic events.
pub(crate) struct EventLog<'a> {
    events: &'a VecDeque<RecentEvent>,
    visible: bool,
}

impl<'a> EventLog<'a> {
    pub(crate) fn new(events: &'a VecDeque<RecentEvent>, visible: bool) -> Self {
        Self { events, visible }
    }

    pub(crate) fn render(&self) -> impl IntoElement {
        let mut panel = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_SM))
            .p(px(theme::SPACE_MD))
            .rounded_md()
            .bg(rgb(theme::PANEL))
            .border_1()
            .border_color(rgb(theme::BORDER));

        if !self.visible {
            return panel.hidden();
        }

        panel = panel.child(
            div()
                .text_size(px(theme::FONT_SMALL))
                .text_color(rgb(theme::TEXT_PRIMARY))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Recent events"),
        );

        let mut recent = self
            .events
            .iter()
            .rev()
            .take(MAX_VISIBLE_EVENTS)
            .collect::<Vec<_>>();
        recent.reverse();

        if recent.is_empty() {
            panel = panel.child(
                div()
                    .text_size(px(theme::FONT_SMALL))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child("No recent events"),
            );
        } else {
            panel = panel.children(recent.into_iter().map(event_row));
        }

        panel
    }
}

fn event_row(event: &RecentEvent) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(px(theme::SPACE_SM))
        .child(
            div()
                .flex_none()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_MUTED))
                .child(format_event_time(event.wall_clock_at)),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(event.description()),
        )
}

fn format_event_time(time: SystemTime) -> String {
    let Ok(duration) = time.duration_since(SystemTime::UNIX_EPOCH) else {
        return "--:--:--".to_owned();
    };
    let seconds = duration.as_secs() % (24 * 60 * 60);
    let hours = seconds / 3600;
    let minutes = (seconds / 60) % 60;
    let seconds = seconds % 60;
    format!(
        "{hours:02}:{minutes:02}:{seconds:02}.{:03}",
        duration.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::format_event_time;

    #[test]
    fn event_time_includes_millisecond_precision() {
        let timestamp = UNIX_EPOCH
            + Duration::from_secs(10 * 3_600 + 42 * 60 + 14)
            + Duration::from_millis(231);

        assert_eq!(format_event_time(timestamp), "10:42:14.231");
    }
}
