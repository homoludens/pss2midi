use gpui::{
    div, prelude::*, px, rgb, size, App, Context, FontWeight, Render, Window, WindowBounds,
    WindowOptions,
};
use pss2midi::ui::state::{AppState, Page};

use crate::theme;

const APP_WINDOW_TITLE: &str = "pss2midi — Yamaha PSS-F30 Audio to MIDI";
pub(super) const INITIAL_WINDOW_SIZE: (f32, f32) = (1000.0, 680.0);
pub(super) const MINIMUM_WINDOW_SIZE: (f32, f32) = (850.0, 600.0);

#[derive(Default)]
struct Pss2MidiApp {
    state: AppState,
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
                    .child(self.page_content()),
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

    fn page_content(&self) -> impl IntoElement {
        let (eyebrow, title, description, panel_title, panel_description, marker) =
            match self.state.page {
                Page::Live => (
                    "MONITORING",
                    "Live",
                    "A workspace for instrument detection and MIDI activity.",
                    "Live monitor",
                    "Detection status, audio levels, and recent events will appear here.",
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
            .child(
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
                            .child(panel_title),
                    )
                    .child(
                        div()
                            .text_size(px(theme::FONT_BODY))
                            .text_color(rgb(theme::TEXT_SECONDARY))
                            .child(panel_description),
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
                    ),
            )
    }
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
