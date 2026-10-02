use std::time::Duration;

use gpui::{
    div, prelude::*, px, rgb, size, App, Context, FontWeight, Render, Window, WindowBounds,
    WindowOptions,
};
use pss2midi::{
    engine::note::note_name,
    ui::state::{AppState, Page},
};

use crate::{
    components::{
        DetectorCard, EventLog, LevelMeter, ScoreMeter, SpectralMatchRow, StatusBadge, StatusTone,
    },
    live::{DetectorAgreement, LiveViewModel},
    piano::PianoKeyboard,
    theme,
};

const APP_WINDOW_TITLE: &str = "pss2midi — Yamaha PSS-F30 Audio to MIDI";
pub(super) const INITIAL_WINDOW_SIZE: (f32, f32) = (1000.0, 680.0);
pub(super) const MINIMUM_WINDOW_SIZE: (f32, f32) = (850.0, 600.0);

struct Pss2MidiApp {
    state: AppState,
    show_log: bool,
}

impl Default for Pss2MidiApp {
    fn default() -> Self {
        Self {
            state: AppState::default(),
            show_log: true,
        }
    }
}

impl Render for Pss2MidiApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::WINDOW))
            .text_color(rgb(theme::TEXT_PRIMARY))
            .child(top_bar())
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar(cx))
                    .child(self.page_content(cx)),
            )
    }
}

impl Pss2MidiApp {
    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(theme::SIDEBAR_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_SM))
            .p(px(theme::SPACE_MD))
            .bg(rgb(theme::SIDEBAR))
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .child(
                div()
                    .px(px(theme::SPACE_MD))
                    .pt(px(theme::SPACE_MD))
                    .pb(px(theme::SPACE_XS))
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("WORKSPACE"),
            )
            .child(self.navigation_item(Page::Live, "Live", "Detection overview", cx))
            .child(self.navigation_item(
                Page::Calibration,
                "Calibration",
                "Build note profiles",
                cx,
            ))
            .child(self.navigation_item(Page::Settings, "Settings", "Application preferences", cx))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE_XS))
                    .px(px(theme::SPACE_MD))
                    .py(px(theme::SPACE_MD))
                    .border_t_1()
                    .border_color(rgb(theme::BORDER))
                    .child(
                        div()
                            .text_size(px(theme::FONT_SMALL))
                            .text_color(rgb(theme::TEXT_SECONDARY))
                            .child("Yamaha PSS-F30"),
                    )
                    .child(
                        div()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child("Audio to MIDI desktop"),
                    ),
            )
    }

    fn navigation_item(
        &self,
        page: Page,
        title: &'static str,
        description: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.state.page == page;
        let page_id = match page {
            Page::Live => "navigation-live",
            Page::Calibration => "navigation-calibration",
            Page::Settings => "navigation-settings",
        };

        div()
            .id(page_id)
            .w_full()
            .flex()
            .items_center()
            .gap(px(theme::SPACE_SM))
            .px(px(theme::SPACE_SM))
            .py(px(theme::SPACE_SM))
            .rounded_md()
            .bg(rgb(if selected {
                theme::ACCENT_TINT
            } else {
                theme::SIDEBAR
            }))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.select_page(page);
                cx.notify();
            }))
            .child(
                div()
                    .w(px(3.0))
                    .h(px(34.0))
                    .rounded_full()
                    .bg(rgb(if selected {
                        theme::ACCENT
                    } else {
                        theme::SIDEBAR
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE_XS))
                    .child(
                        div()
                            .text_size(px(theme::FONT_BODY))
                            .text_color(rgb(if selected {
                                theme::ACCENT
                            } else {
                                theme::TEXT_PRIMARY
                            }))
                            .font_weight(if selected {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child(description),
                    ),
            )
    }

    fn page_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (eyebrow, title, description, panel_title, panel_description, marker) =
            match self.state.page {
                Page::Live => (
                    "MONITORING",
                    "Live",
                    "Live detection, input levels, and detector comparison.",
                    "",
                    "",
                    "01",
                ),
                Page::Calibration => (
                    "INSTRUMENT SETUP",
                    "Calibration",
                    "Prepare spectral note profiles for the Yamaha PSS-F30.",
                    "Guided calibration",
                    "The calibration workflow and per-note progress will appear here.",
                    "02",
                ),
                Page::Settings => (
                    "PREFERENCES",
                    "Settings",
                    "Configure audio, detector, and spectral preferences.",
                    "Application settings",
                    "Audio and detection preferences will be available here.",
                    "03",
                ),
            };

        let content = div().flex_1().min_h_0();
        let content = match self.state.page {
            Page::Live => content.child(self.live_dashboard(cx)),
            Page::Calibration | Page::Settings => {
                content.child(placeholder_panel(panel_title, panel_description, marker))
            }
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap(px(theme::SPACE_LG))
            .p(px(theme::PAGE_PADDING))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE_XS))
                    .child(
                        div()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(rgb(theme::ACCENT))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(eyebrow),
                    )
                    .child(
                        div()
                            .text_size(px(theme::FONT_TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(theme::FONT_SMALL))
                            .text_color(rgb(theme::TEXT_SECONDARY))
                            .child(description),
                    ),
            )
            .child(content)
    }

    fn live_dashboard(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = LiveViewModel::from_state(&self.state);

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .id("live-dashboard-scroll")
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE_MD))
                    .pb(px(theme::SPACE_MD))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_stretch()
                            .gap(px(theme::SPACE_SM))
                            .child(dominant_note_panel(&view))
                            .child(input_levels_panel(&view)),
                    )
                    .child(comparison_panel(&view))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_stretch()
                            .gap(px(theme::SPACE_SM))
                            .child(div().flex_1().min_w_0().child(DetectorCard::yin(
                                view.yin_result.as_ref(),
                                view.yin_is_output,
                            )))
                            .child(div().flex_1().min_w_0().child(DetectorCard::spectral(
                                view.spectral_result.as_ref(),
                                view.spectral_is_output,
                            ))),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_stretch()
                            .gap(px(theme::SPACE_SM))
                            .child(spectral_scores_panel(&view))
                            .child(spectral_matches_panel(&view)),
                    )
                    .child(keyboard_panel(&view))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(theme::SPACE_SM))
                            .child(
                                div()
                                    .text_size(px(theme::FONT_SMALL))
                                    .text_color(rgb(theme::TEXT_PRIMARY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Recent events"),
                            )
                            .child(
                                div()
                                    .id("live-show-log-toggle")
                                    .flex()
                                    .items_center()
                                    .gap(px(theme::SPACE_SM))
                                    .px(px(theme::SPACE_SM))
                                    .py(px(theme::SPACE_XS))
                                    .rounded_md()
                                    .bg(rgb(theme::PANEL_INSET))
                                    .border_1()
                                    .border_color(rgb(theme::BORDER))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_log = !this.show_log;
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .text_size(px(theme::FONT_CAPTION))
                                            .text_color(rgb(theme::TEXT_SECONDARY))
                                            .child("Show log"),
                                    )
                                    .child(
                                        StatusBadge::new(
                                            if self.show_log { "ON" } else { "OFF" },
                                            if self.show_log {
                                                StatusTone::Accent
                                            } else {
                                                StatusTone::Neutral
                                            },
                                        )
                                        .render(),
                                    ),
                            ),
                    )
                    .child(EventLog::new(&self.state.recent_events, self.show_log).render()),
            )
    }
}

fn placeholder_panel(
    title: &'static str,
    description: &'static str,
    marker: &'static str,
) -> impl IntoElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(theme::SPACE_MD))
        .p(px(theme::SPACE_XL))
        .rounded_lg()
        .bg(rgb(theme::PANEL))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .child(
            div()
                .size(px(52.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_lg()
                .bg(rgb(theme::PANEL_INSET))
                .text_size(px(theme::FONT_BODY))
                .text_color(rgb(theme::ACCENT))
                .font_weight(FontWeight::SEMIBOLD)
                .child(marker),
        )
        .child(
            div()
                .text_size(px(theme::FONT_TITLE))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .text_size(px(theme::FONT_BODY))
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(description),
        )
        .child(
            div()
                .mt(px(theme::SPACE_SM))
                .px(px(theme::SPACE_MD))
                .py(px(theme::SPACE_SM))
                .rounded_md()
                .bg(rgb(theme::PANEL_INSET))
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_MUTED))
                .child("PAGE PLACEHOLDER"),
        )
}

fn dominant_note_panel(view: &LiveViewModel) -> impl IntoElement {
    let (headline, note_number, detail) = match &view.dominant_note {
        Some(note) => (
            note.name.clone(),
            format!("MIDI {}", note.midi_note),
            "Current dominant detection".to_owned(),
        ),
        None => (
            "Idle".to_owned(),
            "Unavailable".to_owned(),
            "Waiting for a selected engine note".to_owned(),
        ),
    };
    let latency = view
        .latency
        .map(format_latency)
        .unwrap_or_else(|| "Unavailable".to_owned());

    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_SM))
        .p(px(theme::SPACE_MD))
        .rounded_md()
        .bg(rgb(theme::PANEL))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(theme::SPACE_SM))
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("DOMINANT NOTE"),
                )
                .child(
                    StatusBadge::new(
                        if view.dominant_note.is_some() {
                            "DETECTED"
                        } else {
                            "IDLE"
                        },
                        if view.dominant_note.is_some() {
                            StatusTone::Success
                        } else {
                            StatusTone::Neutral
                        },
                    )
                    .render(),
                ),
        )
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(theme::SPACE_SM))
                .child(
                    div()
                        .text_size(px(27.0))
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(headline),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_SMALL))
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(note_number),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(theme::SPACE_SM))
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(detail),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(format!("Latency · {latency}")),
                ),
        )
}

fn input_levels_panel(view: &LiveViewModel) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .p(px(theme::SPACE_MD))
        .rounded_md()
        .bg(rgb(theme::PANEL))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_MUTED))
                .font_weight(FontWeight::SEMIBOLD)
                .child("INPUT LEVEL"),
        )
        .child(LevelMeter::new("RMS", view.levels.map(|levels| levels.rms_dbfs)).render())
        .child(LevelMeter::new("Peak", view.levels.map(|levels| levels.peak_dbfs)).render())
}

fn comparison_panel(view: &LiveViewModel) -> impl IntoElement {
    let (label, tone, message) = match view.agreement {
        DetectorAgreement::Waiting => (
            "WAITING",
            StatusTone::Neutral,
            "Agreement unavailable · waiting for both detector notes".to_owned(),
        ),
        DetectorAgreement::Agree { note } => (
            "AGREE",
            StatusTone::Success,
            format!("YIN and Spectral agree on {}", note_name(note)),
        ),
        DetectorAgreement::Disagree {
            yin_note,
            spectral_note,
        } => (
            "DISAGREEMENT",
            StatusTone::Warning,
            format!(
                "YIN {} · Spectral {}",
                note_name(yin_note),
                note_name(spectral_note)
            ),
        ),
    };
    let output = if view.yin_is_output {
        "YIN is authoritative for output"
    } else {
        "Spectral is authoritative for output"
    };

    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(theme::SPACE_MD))
        .px(px(theme::SPACE_MD))
        .py(px(theme::SPACE_SM))
        .rounded_md()
        .bg(rgb(theme::PANEL_INSET))
        .border_1()
        .border_color(rgb(if tone == StatusTone::Warning {
            theme::WARNING
        } else {
            theme::BORDER
        }))
        .child(
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(theme::SPACE_SM))
                .child(StatusBadge::new(label, tone).render())
                .child(
                    div()
                        .text_size(px(theme::FONT_SMALL))
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .child(message),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(output),
        )
}

fn spectral_scores_panel(view: &LiveViewModel) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .p(px(theme::SPACE_MD))
        .rounded_md()
        .bg(rgb(theme::PANEL))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .child(
            div()
                .text_size(px(theme::FONT_SMALL))
                .text_color(rgb(theme::TEXT_PRIMARY))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Spectral confidence & margin"),
        )
        .child(
            ScoreMeter::new(
                "Confidence",
                view.spectral_result
                    .as_ref()
                    .map(|result| result.confidence),
            )
            .render(),
        )
        .child(
            ScoreMeter::new(
                "Margin",
                view.spectral_result.as_ref().map(|result| result.margin),
            )
            .render(),
        )
}

fn spectral_matches_panel(view: &LiveViewModel) -> impl IntoElement {
    let mut panel = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_SM))
        .p(px(theme::SPACE_MD))
        .rounded_md()
        .bg(rgb(theme::PANEL))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .child(
            div()
                .text_size(px(theme::FONT_SMALL))
                .text_color(rgb(theme::TEXT_PRIMARY))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Top spectral matches"),
        );

    if view.ranked_matches.is_empty() {
        panel = panel.child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_MUTED))
                .child("No ranked matches available"),
        );
    } else {
        panel = panel.children(view.ranked_matches.iter().cloned().enumerate().map(
            |(index, candidate)| {
                SpectralMatchRow::new(index + 1, candidate, view.keyboard_selected_note).render()
            },
        ));
    }

    panel
}

fn keyboard_panel(view: &LiveViewModel) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_SM))
        .p(px(theme::SPACE_MD))
        .rounded_md()
        .bg(rgb(theme::PANEL))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(theme::SPACE_SM))
                .child(
                    div()
                        .text_size(px(theme::FONT_SMALL))
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Keyboard activity · MIDI 36–72"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE_XS))
                        .child(StatusBadge::new("Selected", StatusTone::Accent).render())
                        .child(StatusBadge::new("YIN", StatusTone::Warning).render()),
                ),
        )
        .child(
            PianoKeyboard::new()
                .selected_note(view.keyboard_selected_note)
                .yin_note(view.keyboard_yin_note)
                .render(),
        )
}

fn format_latency(latency: Duration) -> String {
    format!("{:.2} ms", latency.as_secs_f64() * 1_000.0)
}

fn top_bar() -> impl IntoElement {
    div()
        .h(px(theme::TOP_BAR_HEIGHT))
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .px(px(theme::SPACE_XL))
        .bg(rgb(theme::TOP_BAR))
        .border_b_1()
        .border_color(rgb(theme::BORDER))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(theme::SPACE_MD))
                .child(
                    div()
                        .size(px(38.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .bg(rgb(theme::ACCENT_TINT))
                        .text_size(px(theme::FONT_BODY))
                        .text_color(rgb(theme::ACCENT))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("P"),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE_XS))
                        .child(
                            div()
                                .text_size(px(theme::FONT_TITLE))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("pss2midi"),
                        )
                        .child(
                            div()
                                .text_size(px(theme::FONT_SMALL))
                                .text_color(rgb(theme::TEXT_SECONDARY))
                                .child("Yamaha PSS-F30 Audio to MIDI"),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(theme::SPACE_SM))
                .px(px(theme::SPACE_MD))
                .py(px(theme::SPACE_SM))
                .rounded_md()
                .bg(rgb(theme::PANEL_INSET))
                .child(div().size(px(7.0)).rounded_full().bg(rgb(theme::ACCENT)))
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("DESKTOP APP"),
                ),
        )
}

fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(
            size(px(INITIAL_WINDOW_SIZE.0), px(INITIAL_WINDOW_SIZE.1)),
            cx,
        )),
        window_min_size: Some(size(px(MINIMUM_WINDOW_SIZE.0), px(MINIMUM_WINDOW_SIZE.1))),
        ..WindowOptions::default()
    }
}

pub(super) fn launch(cx: &mut App) {
    let options = window_options(cx);
    cx.open_window(options, |window, cx| {
        window.set_window_title(APP_WINDOW_TITLE);
        cx.new(|_| Pss2MidiApp::default())
    })
    .expect("failed to open GPUI window");
}

#[cfg(test)]
mod tests {
    use super::{INITIAL_WINDOW_SIZE, MINIMUM_WINDOW_SIZE};

    #[test]
    fn window_size_constants_match_shell_target() {
        assert_eq!(INITIAL_WINDOW_SIZE, (1000.0, 680.0));
        assert_eq!(MINIMUM_WINDOW_SIZE, (850.0, 600.0));
    }
}
