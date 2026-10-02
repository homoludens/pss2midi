use std::{
    ops::Range,
    time::{Duration, Instant},
};

use gpui::{
    div, prelude::*, px, rgb, size, App, Context, CursorStyle, FocusHandle, FontWeight,
    KeyDownEvent, MouseButton, Render, Task, Window, WindowBounds, WindowOptions,
};
use pss2midi::{
    engine::{
        app_config::AppConfig,
        calibration::{CalibrationRejectionReason, CalibrationSampleQuality},
        config::DetectorMode,
        load_config_or_default,
        note::note_name,
        save_config, EngineCommand, EngineEvent, EngineState, PssEngine, TemplateStatus,
    },
    ui::state::{
        engine_command_for_action, startup_engine_commands, AppState, CalibrationStatus, ErrorKind,
        LiveControlAction, MidiOutputStatus, Page,
    },
};

use crate::{
    calibration::{
        command_for_action, sample_quality_label, CalibrationAction, CalibrationViewModel,
    },
    components::{
        DetectorCard, EventLog, LevelMeter, ScoreMeter, SpectralMatchRow, StatusBadge, StatusTone,
    },
    live::{audio_device_choices, AudioDeviceChoice, DetectorAgreement, LiveViewModel},
    piano::PianoKeyboard,
    settings::{
        apply_text_edit, SettingField, SettingsViewModel, ADVANCED_SETTINGS_EXPANDED_BY_DEFAULT,
    },
    theme,
};

const APP_WINDOW_TITLE: &str = "pss2midi — Yamaha PSS-F30 Audio to MIDI";
pub(super) const INITIAL_WINDOW_SIZE: (f32, f32) = (1000.0, 680.0);
pub(super) const MINIMUM_WINDOW_SIZE: (f32, f32) = (850.0, 600.0);

struct Pss2MidiApp {
    state: AppState,
    show_log: bool,
    audio_device_menu_open: bool,
    advanced_settings_expanded: bool,
    settings_text_edit: Option<SettingsTextEdit>,
    settings_focus_handle: FocusHandle,
    settings_save_sender: async_channel::Sender<ConfigSaveRequest>,
    settings_save_revision: u64,
    settings_save_task: Option<Task<()>>,
    settings_error: Option<String>,
    settings_saved: bool,
    event_task: Option<Task<()>>,
    engine: Option<PssEngine>,
}

struct ConfigSaveRequest {
    revision: u64,
    config: AppConfig,
}

struct SettingsTextEdit {
    field: SettingField,
    text: String,
    cursor: usize,
    selection: Option<Range<usize>>,
}

impl SettingsTextEdit {
    fn new(field: SettingField, text: String) -> Self {
        let cursor = text.len();
        Self {
            field,
            text,
            cursor,
            selection: None,
        }
    }

    fn replace_selection(&mut self, replacement: &str) -> bool {
        let range = self.selection.take().unwrap_or(self.cursor..self.cursor);
        if range.start > range.end || range.end > self.text.len() {
            return false;
        }
        self.text.replace_range(range.clone(), replacement);
        self.cursor = range.start + replacement.len();
        self.selection = None;
        true
    }
}

impl Pss2MidiApp {
    fn new(cx: &mut Context<Self>) -> Self {
        let loaded_config = load_config_or_default();
        let config = loaded_config.config;
        let config_error = loaded_config.error.map(|error| error.to_string());
        let (settings_save_sender, settings_save_receiver) = async_channel::unbounded();
        let mut app = Self {
            state: AppState::new(config.clone()),
            show_log: true,
            audio_device_menu_open: false,
            advanced_settings_expanded: ADVANCED_SETTINGS_EXPANDED_BY_DEFAULT,
            settings_text_edit: None,
            settings_focus_handle: cx.focus_handle(),
            settings_save_sender,
            settings_save_revision: 0,
            settings_save_task: None,
            settings_error: None,
            settings_saved: false,
            event_task: None,
            engine: None,
        };
        if let Some(message) = config_error {
            app.state
                .reduce_event(EngineEvent::Error { message }, Instant::now());
        }

        app.settings_save_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(mut request) = settings_save_receiver.recv().await {
                while let Ok(newer_request) = settings_save_receiver.try_recv() {
                    request = newer_request;
                }

                let revision = request.revision;
                let result = cx
                    .background_spawn(async move {
                        save_config(&request.config).map_err(|error| error.to_string())
                    })
                    .await;

                if this
                    .update(cx, |this, cx| {
                        if this.settings_save_revision == revision {
                            match result {
                                Ok(()) => {
                                    this.settings_error = None;
                                    this.settings_saved = true;
                                }
                                Err(error) => {
                                    this.settings_error = Some(error);
                                    this.settings_saved = false;
                                }
                            }
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));

        match PssEngine::new_with_config(config.clone()) {
            Ok(engine) => {
                let event_rx = engine.event_receiver();
                app.engine = Some(engine);
                app.event_task = Some(cx.spawn(async move |this, cx| {
                    while let Ok(event) = event_rx.recv().await {
                        if this
                            .update(cx, |this, cx| {
                                let previous_config = this.state.config.clone();
                                this.state.reduce_event(event, Instant::now());
                                if this.state.config != previous_config {
                                    this.queue_settings_save();
                                }
                                cx.notify();
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
                for command in startup_engine_commands(&config) {
                    if app
                        .engine
                        .as_ref()
                        .is_some_and(|engine| engine.send(command).is_err())
                    {
                        app.state.reduce_event(
                            EngineEvent::Error {
                                message: "The engine worker rejected a startup command".to_owned(),
                            },
                            Instant::now(),
                        );
                        break;
                    }
                }
            }
            Err(error) => app.state.reduce_event(
                EngineEvent::Error {
                    message: format!("Could not start the engine worker: {error}"),
                },
                Instant::now(),
            ),
        }

        app
    }

    fn dispatch_control_action(&mut self, action: LiveControlAction, cx: &mut Context<Self>) {
        let Some(command) = engine_command_for_action(action, self.state.engine_state) else {
            return;
        };
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        if engine.send(command).is_err() {
            self.state.reduce_event(
                EngineEvent::Error {
                    message: "The engine worker is no longer accepting commands".to_owned(),
                },
                Instant::now(),
            );
            cx.notify();
        }
    }

    fn dispatch_calibration_action(&mut self, action: CalibrationAction, cx: &mut Context<Self>) {
        let command = command_for_action(action);
        let error = match self.engine.as_ref() {
            Some(engine) => engine.send(command).err().map(|_| {
                "The calibration engine worker is no longer accepting commands".to_owned()
            }),
            None => Some("The calibration engine is unavailable".to_owned()),
        };
        if let Some(message) = error {
            self.state
                .reduce_event(EngineEvent::CalibrationError { message }, Instant::now());
            cx.notify();
        }
    }

    fn begin_settings_edit(
        &mut self,
        field: SettingField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = field.value(&self.state.config);
        self.settings_text_edit = Some(SettingsTextEdit::new(field, text));
        self.settings_focus_handle.focus(window, cx);
        cx.notify();
    }

    fn handle_settings_key(
        &mut self,
        field: SettingField,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .settings_text_edit
            .as_ref()
            .is_some_and(|edit| edit.field == field)
        {
            return;
        }

        let key = event.keystroke.key.to_ascii_lowercase();
        let modified = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        if modified && key == "a" {
            if let Some(edit) = self.settings_text_edit.as_mut() {
                edit.selection = Some(0..edit.text.len());
                edit.cursor = edit.text.len();
            }
            cx.notify();
            return;
        }
        if key == "enter" {
            self.settings_text_edit = None;
            cx.notify();
            return;
        }
        if key == "escape" {
            self.settings_text_edit = None;
            self.settings_error = None;
            cx.notify();
            return;
        }

        let mut changed = false;
        if let Some(edit) = self.settings_text_edit.as_mut() {
            match key.as_str() {
                "backspace" => {
                    if edit.selection.is_none() && edit.cursor > 0 {
                        let previous = previous_char_boundary(&edit.text, edit.cursor);
                        edit.selection = Some(previous..edit.cursor);
                    }
                    changed = edit.replace_selection("");
                }
                "delete" => {
                    if edit.selection.is_none() && edit.cursor < edit.text.len() {
                        let next = next_char_boundary(&edit.text, edit.cursor);
                        edit.selection = Some(edit.cursor..next);
                    }
                    changed = edit.replace_selection("");
                }
                "left" => {
                    if let Some(selection) = edit.selection.take() {
                        edit.cursor = selection.start;
                    } else {
                        edit.cursor = previous_char_boundary(&edit.text, edit.cursor);
                    }
                }
                "right" => {
                    if let Some(selection) = edit.selection.take() {
                        edit.cursor = selection.end;
                    } else {
                        edit.cursor = next_char_boundary(&edit.text, edit.cursor);
                    }
                }
                "home" => {
                    edit.selection = None;
                    edit.cursor = 0;
                }
                "end" => {
                    edit.selection = None;
                    edit.cursor = edit.text.len();
                }
                "v" if modified => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        changed =
                            edit.replace_selection(&text.replace('\n', " ").replace('\r', " "));
                    }
                }
                _ => {
                    if !modified && !event.keystroke.modifiers.alt {
                        if let Some(text) = event.keystroke.key_char.as_deref() {
                            if !text.chars().any(char::is_control) {
                                changed = edit.replace_selection(text);
                            }
                        }
                    }
                }
            }
        }

        if changed {
            let text = self
                .settings_text_edit
                .as_ref()
                .map(|edit| edit.text.clone())
                .unwrap_or_default();
            self.apply_settings_text(field, &text);
        }
        let _ = window;
        cx.notify();
    }

    fn apply_settings_text(&mut self, field: SettingField, text: &str) {
        match apply_text_edit(&self.state.config, field, text) {
            Ok(config) => {
                self.settings_error = None;
                if config != self.state.config {
                    let template_path =
                        (field == SettingField::TemplatePath).then(|| config.template_path.clone());
                    self.state.config = config;
                    self.settings_saved = false;
                    self.queue_settings_save();
                    if let (Some(engine), Some(path)) = (self.engine.as_ref(), template_path) {
                        if engine
                            .send(EngineCommand::SetTemplatePath { path })
                            .is_err()
                        {
                            self.settings_error = Some(
                                "The engine worker is no longer accepting template settings"
                                    .to_owned(),
                            );
                        }
                    }
                }
            }
            Err(error) => {
                self.settings_save_revision = self.settings_save_revision.wrapping_add(1);
                self.settings_error = Some(error);
                self.settings_saved = false;
            }
        }
    }

    fn queue_settings_save(&mut self) {
        self.settings_save_revision = self.settings_save_revision.wrapping_add(1);
        self.settings_saved = false;
        if self
            .settings_save_sender
            .try_send(ConfigSaveRequest {
                revision: self.settings_save_revision,
                config: self.state.config.clone(),
            })
            .is_err()
        {
            self.settings_error = Some("Settings could not be queued for saving".to_owned());
        }
    }
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor.min(text.len())]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text.get(cursor..)
        .and_then(|remaining| remaining.char_indices().nth(1))
        .map(|(offset, _)| cursor + offset)
        .unwrap_or(text.len())
}

impl Render for Pss2MidiApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::WINDOW))
            .text_color(rgb(theme::TEXT_PRIMARY))
            .child(top_bar(&self.state, cx))
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

    fn page_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (eyebrow, title, description) = match self.state.page {
            Page::Live => (
                "MONITORING",
                "Live",
                "Live detection, input levels, and detector comparison.",
            ),
            Page::Calibration => (
                "INSTRUMENT SETUP",
                "Calibration",
                "Prepare spectral note profiles for the Yamaha PSS-F30.",
            ),
            Page::Settings => (
                "PREFERENCES",
                "Settings",
                "Configure audio, detector, spectral, and advanced preferences.",
            ),
        };

        let content = div().flex_1().min_h_0();
        let content = match self.state.page {
            Page::Live => content.child(self.live_dashboard(cx)),
            Page::Calibration => content.child(self.calibration_dashboard(cx)),
            Page::Settings => content.child(self.settings_dashboard(cx)),
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
                    .child(live_controls(
                        &self.state,
                        self.audio_device_menu_open,
                        None,
                        cx,
                    ))
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

    fn calibration_dashboard(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = CalibrationViewModel::from_calibration(
            &self.state.calibration,
            self.state.audio_levels,
        );
        let mut page = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .id("calibration-dashboard-scroll")
            .overflow_y_scroll();
        let mut content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_MD))
            .pb(px(theme::SPACE_MD));

        if let Some(message) = &view.error_message {
            content = content.child(calibration_error_banner(message));
        }

        content = content
            .child(calibration_progress_panel(&view, cx))
            .child(calibration_keyboard_panel(&view));
        page = page.child(content);
        page
    }

    fn settings_dashboard(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = SettingsViewModel::from_state(&self.state, self.advanced_settings_expanded);
        let mut content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_MD))
            .pb(px(theme::SPACE_MD));

        if let Some(message) = &self.settings_error {
            content = content.child(settings_error_banner(message));
        } else if self.settings_saved {
            content = content.child(settings_saved_banner());
        }
        let mut audio_panel = settings_panel(
            "Audio and detector",
            "Device and mode reflect the latest engine state; sample rate is saved for startup settings.",
        )
        .child(live_controls(
            &self.state,
            self.audio_device_menu_open,
            Some(&view),
            cx,
        ));
        audio_panel = audio_panel.child(settings_row(
            "Sample rate",
            "Requested capture rate · takes effect when settings are connected to engine startup.",
            self.settings_text_field(
                SettingField::SampleRate,
                SettingField::SampleRate.value(&view.config),
                cx,
            ),
        ));
        content = content.child(audio_panel);

        let template_panel = settings_panel(
            "Spectral templates",
            "Choose the editable template file used by future calibration runs.",
        )
        .child(settings_row(
            "Template path",
            "Template samples remain separate from the application settings file.",
            self.settings_text_field(
                SettingField::TemplatePath,
                SettingField::TemplatePath.value(&view.config),
                cx,
            ),
        ))
        .child(template_status_summary(&self.state.template_status))
        .child(
            div()
                .id("settings-recalibrate")
                .flex_none()
                .px(px(theme::SPACE_MD))
                .py(px(theme::SPACE_SM))
                .rounded_md()
                .bg(rgb(theme::ACCENT_TINT))
                .border_1()
                .border_color(rgb(theme::ACCENT))
                .text_size(px(theme::FONT_SMALL))
                .text_color(rgb(theme::ACCENT))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.select_page(Page::Calibration);
                    cx.notify();
                }))
                .child("Recalibrate"),
        );
        content = content.child(template_panel);

        let mut spectral_panel = settings_panel(
            "Spectral detection",
            "Tune when a spectral observation is taken and how matches are accepted.",
        );
        spectral_panel = spectral_panel
            .child(settings_row(
                "Delay",
                "Milliseconds after onset before analyzing a note.",
                self.settings_text_field(
                    SettingField::SpectralDelay,
                    SettingField::SpectralDelay.value(&view.config),
                    cx,
                ),
            ))
            .child(settings_row(
                "Window",
                "Audio window duration in milliseconds.",
                self.settings_text_field(
                    SettingField::SpectralWindow,
                    SettingField::SpectralWindow.value(&view.config),
                    cx,
                ),
            ))
            .child(settings_row(
                "Minimum score",
                "Lowest template similarity accepted as a match (0–1).",
                self.settings_text_field(
                    SettingField::SpectralMinimumScore,
                    SettingField::SpectralMinimumScore.value(&view.config),
                    cx,
                ),
            ))
            .child(settings_row(
                "Minimum margin",
                "Required lead over the next-ranked template (0–1).",
                self.settings_text_field(
                    SettingField::SpectralMinimumMargin,
                    SettingField::SpectralMinimumMargin.value(&view.config),
                    cx,
                ),
            ));
        content = content.child(spectral_panel);

        let midi_panel = settings_panel(
            "MIDI output",
            "Connection name and status are reported by the engine.",
        )
        .child(midi_output_badge(&view.midi_output_status));
        content = content.child(midi_panel);

        let advanced_header = div()
            .id("settings-advanced-toggle")
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(theme::SPACE_SM))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.advanced_settings_expanded = !this.advanced_settings_expanded;
                cx.notify();
            }))
            .child(
                div()
                    .text_size(px(theme::FONT_SMALL))
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Advanced"),
            )
            .child(
                div()
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .child(if view.advanced_expanded {
                        "Expanded · click to collapse"
                    } else {
                        "Collapsed · click to expand"
                    }),
            );
        let mut advanced_panel = settings_panel(
            "Advanced controls",
            "FFT, onset, release/retrigger, YIN voting, and spectral analysis tuning.",
        )
        .child(advanced_header);
        if view.advanced_expanded {
            for (label, description, field) in [
                (
                    "FFT size",
                    "Spectral transform size · power of two.",
                    SettingField::FftSize,
                ),
                (
                    "Hop size",
                    "Samples processed between detector frames.",
                    SettingField::HopSize,
                ),
                (
                    "Onset buffer",
                    "Frame size used by onset detection.",
                    SettingField::OnsetBufferSize,
                ),
                (
                    "Silence threshold",
                    "Input level threshold in dBFS.",
                    SettingField::SilenceDb,
                ),
                (
                    "Release",
                    "Silence duration before MIDI note-off, in milliseconds.",
                    SettingField::ReleaseMs,
                ),
                (
                    "Retrigger",
                    "Minimum time between repeated attacks, in milliseconds.",
                    SettingField::RetriggerMs,
                ),
                (
                    "Onset threshold",
                    "aubio onset peak threshold.",
                    SettingField::OnsetThreshold,
                ),
                (
                    "YIN pitch buffer",
                    "aubio pitch analysis window size.",
                    SettingField::PitchBufferSize,
                ),
                (
                    "YIN attack ignore",
                    "Ignore pitch estimates after onset, in milliseconds.",
                    SettingField::AttackIgnoreMs,
                ),
                (
                    "YIN decision window",
                    "Collect pitch votes for this duration, in milliseconds.",
                    SettingField::DecisionWindowMs,
                ),
                (
                    "YIN decision extension",
                    "Additional time for ambiguous pitch votes, in milliseconds.",
                    SettingField::DecisionExtendMs,
                ),
                (
                    "YIN vote ratio",
                    "Fraction of valid pitch votes required for a winner.",
                    SettingField::VoteRatio,
                ),
                (
                    "YIN stable frames",
                    "Stable frames required for initial pitch acquisition.",
                    SettingField::InitialStableFrames,
                ),
            ] {
                advanced_panel = advanced_panel.child(settings_row(
                    label,
                    description,
                    self.settings_text_field(field, field.value(&view.config), cx),
                ));
            }
        }
        content = content.child(advanced_panel);

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .id("settings-dashboard-scroll")
            .overflow_y_scroll()
            .child(content)
    }

    fn settings_text_field(
        &self,
        field: SettingField,
        value: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active_edit = self
            .settings_text_edit
            .as_ref()
            .filter(|edit| edit.field == field);
        let is_active = active_edit.is_some();
        let display_value = active_edit.map_or(value.clone(), |edit| {
            let mut display = edit.text.clone();
            display.insert(edit.cursor, '▏');
            display
        });
        let focus_handle = self.settings_focus_handle.clone();

        div()
            .id(format!("settings-input-{}", field.id()))
            .track_focus(&focus_handle)
            .w(px(280.0))
            .max_w(px(360.0))
            .flex_none()
            .px(px(theme::SPACE_SM))
            .py(px(theme::SPACE_SM))
            .rounded_md()
            .bg(rgb(theme::PANEL_INSET))
            .border_1()
            .border_color(rgb(if is_active {
                theme::ACCENT
            } else {
                theme::BORDER
            }))
            .text_size(px(theme::FONT_SMALL))
            .text_color(rgb(if is_active {
                theme::TEXT_PRIMARY
            } else {
                theme::TEXT_SECONDARY
            }))
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.begin_settings_edit(field, window, cx);
                }),
            )
            .on_key_down(cx.listener(move |this, event, window, cx| {
                this.handle_settings_key(field, event, window, cx);
            }))
            .child(display_value)
    }
}

fn settings_panel(title: &'static str, description: &'static str) -> gpui::Div {
    div()
        .w_full()
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
                .child(title),
        )
        .child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(description),
        )
}

fn settings_row(
    label: &'static str,
    description: &'static str,
    control: impl IntoElement,
) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(theme::SPACE_MD))
        .px(px(theme::SPACE_SM))
        .py(px(theme::SPACE_XS))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_XS))
                .child(
                    div()
                        .text_size(px(theme::FONT_SMALL))
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child(description),
                ),
        )
        .child(control)
}

fn settings_error_banner(message: &str) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(theme::SPACE_SM))
        .px(px(theme::SPACE_MD))
        .py(px(theme::SPACE_SM))
        .rounded_md()
        .bg(rgb(theme::ERROR_TINT))
        .border_1()
        .border_color(rgb(theme::ERROR))
        .child(StatusBadge::new("SETTINGS ERROR", StatusTone::Error).render())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(theme::FONT_SMALL))
                .text_color(rgb(theme::ERROR))
                .child(message.to_owned()),
        )
}

fn settings_saved_banner() -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(theme::SPACE_SM))
        .px(px(theme::SPACE_MD))
        .py(px(theme::SPACE_SM))
        .rounded_md()
        .bg(rgb(theme::SUCCESS_TINT))
        .border_1()
        .border_color(rgb(theme::SUCCESS))
        .child(StatusBadge::new("SAVED", StatusTone::Success).render())
        .child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child("Settings saved to ~/.config/pss2midi/config.json"),
        )
}

fn calibration_error_banner(message: &str) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(theme::SPACE_SM))
        .px(px(theme::SPACE_MD))
        .py(px(theme::SPACE_SM))
        .rounded_md()
        .bg(rgb(theme::ERROR_TINT))
        .border_1()
        .border_color(rgb(theme::ERROR))
        .child(StatusBadge::new("CALIBRATION ERROR", StatusTone::Error).render())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(theme::FONT_SMALL))
                .text_color(rgb(theme::ERROR))
                .child(message.to_owned()),
        )
}

fn calibration_progress_panel(
    view: &CalibrationViewModel,
    cx: &mut Context<Pss2MidiApp>,
) -> impl IntoElement {
    let (status_label, status_tone) = match view.status {
        CalibrationStatus::Idle => ("READY", StatusTone::Neutral),
        CalibrationStatus::Running => ("IN PROGRESS", StatusTone::Accent),
        CalibrationStatus::Completed => ("COMPLETE", StatusTone::Success),
        CalibrationStatus::Cancelled => ("CANCELLED", StatusTone::Warning),
        CalibrationStatus::Error => ("ERROR", StatusTone::Error),
    };
    let mut panel = div()
        .w_full()
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
                        .child("Guided spectral calibration"),
                )
                .child(StatusBadge::new(status_label, status_tone).render()),
        );

    if view.status == CalibrationStatus::Completed {
        if let Some(completion) = &view.completion {
            panel = panel.child(calibration_completion_content(completion));
        }
        return panel;
    }

    let mut note_panel = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_SM))
        .p(px(theme::SPACE_MD))
        .rounded_md()
        .bg(rgb(theme::PANEL_INSET))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_MUTED))
                .font_weight(FontWeight::SEMIBOLD)
                .child("REQUESTED NOTE"),
        );

    if let Some(progress) = &view.progress {
        note_panel = note_panel
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
                            .child(note_name(progress.requested_note)),
                    )
                    .child(
                        div()
                            .text_size(px(theme::FONT_SMALL))
                            .text_color(rgb(theme::TEXT_SECONDARY))
                            .child(format!("MIDI {}", progress.requested_note)),
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
                            .text_size(px(theme::FONT_SMALL))
                            .text_color(rgb(theme::TEXT_PRIMARY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!(
                                "Sample {} of {}",
                                progress.sample_index, progress.samples_per_note
                            )),
                    )
                    .child(
                        StatusBadge::new(
                            format!(
                                "{} / {} accepted",
                                progress.accepted_samples_for_note, progress.samples_per_note
                            ),
                            if progress.accepted_samples_for_note == progress.samples_per_note {
                                StatusTone::Success
                            } else {
                                StatusTone::Neutral
                            },
                        )
                        .render(),
                    ),
            )
            .child(accepted_sample_dots(
                progress.accepted_samples_for_note,
                progress.samples_per_note,
            ));
    } else {
        note_panel = note_panel.child(
            div()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_XS))
                .child(
                    div()
                        .text_size(px(theme::FONT_BODY))
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("C2 · MIDI 36"),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child("Start calibration to request the first sample."),
                ),
        );
    }

    let summary = div()
        .w_full()
        .flex()
        .items_stretch()
        .gap(px(theme::SPACE_SM))
        .child(note_panel)
        .child(calibration_quality_panel(view));

    let progress_fraction = if view.required_samples == 0 {
        0.0
    } else {
        (view.accepted_samples as f32 / view.required_samples as f32).clamp(0.0, 1.0)
    };
    let overall_progress = div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(theme::SPACE_MD))
        .px(px(theme::SPACE_MD))
        .py(px(theme::SPACE_SM))
        .rounded_md()
        .bg(rgb(theme::PANEL_INSET))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .child(
            div()
                .flex_none()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_MUTED))
                .font_weight(FontWeight::SEMIBOLD)
                .child("OVERALL PROGRESS"),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(theme::FONT_SMALL))
                .text_color(rgb(theme::TEXT_PRIMARY))
                .child(format!(
                    "{} / 37 notes · {} / {} samples",
                    view.completed_notes.len(),
                    view.accepted_samples,
                    view.required_samples
                )),
        )
        .child(
            div()
                .flex_1()
                .relative()
                .h(px(7.0))
                .overflow_hidden()
                .rounded_full()
                .bg(rgb(theme::WINDOW))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .w(gpui::relative(progress_fraction))
                        .rounded_full()
                        .bg(rgb(theme::SUCCESS)),
                ),
        );
    panel = panel.child(summary).child(overall_progress);

    let mut actions = div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(theme::SPACE_SM));
    if view.status == CalibrationStatus::Cancelled {
        actions = actions
            .child(
                div()
                    .text_size(px(theme::FONT_SMALL))
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .child("Calibration canceled. Completed notes remain marked."),
            )
            .child(calibration_action_button(
                "calibration-start-again",
                "Start Calibration",
                CalibrationAction::Start,
                true,
                cx,
            ));
    } else if view.can_start() {
        actions = actions.child(calibration_action_button(
            "calibration-start",
            "Start Calibration",
            CalibrationAction::Start,
            true,
            cx,
        ));
        actions = actions.child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_MUTED))
                .child("37 notes · 5 accepted samples per note"),
        );
    } else if view.can_retry_or_cancel() {
        actions = actions
            .child(calibration_action_button(
                "calibration-retry",
                "Retry sample",
                CalibrationAction::Retry,
                true,
                cx,
            ))
            .child(calibration_action_button(
                "calibration-cancel",
                "Cancel",
                CalibrationAction::Cancel,
                false,
                cx,
            ));
    }
    if view.status != CalibrationStatus::Completed {
        panel = panel.child(actions);
    }

    panel
}

fn accepted_sample_dots(accepted: usize, total: usize) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(theme::SPACE_SM))
        .children((0..total).map(|index| {
            div()
                .id(format!("calibration-sample-dot-{index}"))
                .size(px(10.0))
                .rounded_full()
                .bg(rgb(if index < accepted {
                    theme::SUCCESS
                } else {
                    theme::BORDER
                }))
        }))
}

fn calibration_quality_panel(view: &CalibrationViewModel) -> impl IntoElement {
    let mut panel = div()
        .w(px(238.0))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_SM))
        .p(px(theme::SPACE_MD))
        .rounded_md()
        .bg(rgb(theme::PANEL_INSET))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_MUTED))
                .font_weight(FontWeight::SEMIBOLD)
                .child("SAMPLE QUALITY"),
        );

    if let Some(result) = &view.last_sample_result {
        let tone = match &result.quality {
            CalibrationSampleQuality::Accepted => StatusTone::Success,
            CalibrationSampleQuality::Rejected(_) => StatusTone::Warning,
            CalibrationSampleQuality::AwaitingOnset => StatusTone::Neutral,
        };
        panel = panel
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SPACE_XS))
                    .child(StatusBadge::new(sample_quality_label(&result.quality), tone).render())
                    .child(
                        div()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(rgb(theme::TEXT_SECONDARY))
                            .child(format!(
                                "{} · {}/{}",
                                note_name(result.requested_note),
                                result.sample_index,
                                result.samples_per_note
                            )),
                    ),
            )
            .child(calibration_outcome_description(view, result))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE_XS))
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .child(format!(
                        "Sample RMS · {}",
                        result
                            .rms_dbfs
                            .filter(|value| value.is_finite())
                            .map(|value| format!("{value:.1} dBFS"))
                            .unwrap_or_else(|| "Unavailable".to_owned())
                    ))
                    .child(format!("Sample peak · {}", format_sample_peak(result.peak))),
            );
    } else {
        let prompt = if view.is_running() {
            "Play the requested key to capture the first sample."
        } else {
            "Sample quality will appear after the first capture."
        };
        panel = panel.child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(prompt),
        );
    }

    panel
        .child(
            LevelMeter::new(
                "Live input RMS",
                view.live_levels.map(|levels| levels.rms_dbfs),
            )
            .render(),
        )
        .child(
            LevelMeter::new(
                "Live input peak",
                view.live_levels.map(|levels| levels.peak_dbfs),
            )
            .render(),
        )
}

fn calibration_outcome_description(
    view: &CalibrationViewModel,
    result: &pss2midi::engine::calibration::CalibrationProgress,
) -> impl IntoElement {
    let current = view.progress.as_ref();
    let waiting_for_onset = current.is_some_and(|progress| {
        matches!(&progress.quality, CalibrationSampleQuality::AwaitingOnset)
    });
    let outcome = match &result.quality {
        CalibrationSampleQuality::Accepted => format!(
            "{} sample {} accepted.",
            note_name(result.requested_note),
            result.sample_index
        ),
        CalibrationSampleQuality::Rejected(CalibrationRejectionReason::TooQuiet) => format!(
            "{} sample {} was too quiet; it does not advance progress.",
            note_name(result.requested_note),
            result.sample_index
        ),
        CalibrationSampleQuality::Rejected(CalibrationRejectionReason::Clipped) => format!(
            "{} sample {} was clipped; it does not advance progress.",
            note_name(result.requested_note),
            result.sample_index
        ),
        CalibrationSampleQuality::Rejected(CalibrationRejectionReason::Invalid) => format!(
            "{} sample {} was invalid; it does not advance progress.",
            note_name(result.requested_note),
            result.sample_index
        ),
        CalibrationSampleQuality::AwaitingOnset => "Waiting for an onset.".to_owned(),
    };
    let message = if waiting_for_onset {
        if let Some(progress) = current {
            if result.requested_note == progress.requested_note
                && result.sample_index == progress.sample_index
            {
                format!("{outcome} Waiting for another onset for this sample.")
            } else {
                format!(
                    "{outcome} Waiting for {} sample {}.",
                    note_name(progress.requested_note),
                    progress.sample_index
                )
            }
        } else {
            outcome
        }
    } else {
        outcome
    };

    div()
        .text_size(px(theme::FONT_CAPTION))
        .text_color(rgb(theme::TEXT_SECONDARY))
        .child(message)
}

fn format_sample_peak(peak: Option<f32>) -> String {
    peak.filter(|value| value.is_finite())
        .map(|value| {
            let dbfs = if value > 0.0 {
                format!("{:.1} dBFS", 20.0 * value.log10())
            } else {
                "−∞ dBFS".to_owned()
            };
            format!("{value:.3} · {dbfs}")
        })
        .unwrap_or_else(|| "Unavailable".to_owned())
}

fn calibration_completion_content(
    completion: &pss2midi::ui::state::CalibrationCompletion,
) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(theme::SPACE_MD))
        .p(px(theme::SPACE_MD))
        .rounded_md()
        .bg(rgb(theme::SUCCESS_TINT))
        .border_1()
        .border_color(rgb(theme::SUCCESS))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_SM))
                .child(
                    div()
                        .text_size(px(theme::FONT_BODY))
                        .text_color(rgb(theme::SUCCESS))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Calibration complete"),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_SMALL))
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .child(format!(
                            "Saved {} notes and {} accepted samples.",
                            completion.note_count, completion.sample_count
                        )),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(format!(
                            "Template file · {}",
                            completion.template_path.display()
                        )),
                ),
        )
        // Task 3.2 connects this visible CTA to template reload and playback.
        .child(
            div()
                .id("calibration-start-playing")
                .flex_none()
                .px(px(theme::SPACE_MD))
                .py(px(theme::SPACE_SM))
                .rounded_md()
                .bg(rgb(theme::ACCENT_TINT))
                .border_1()
                .border_color(rgb(theme::ACCENT))
                .text_size(px(theme::FONT_SMALL))
                .text_color(rgb(theme::ACCENT))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Start Playing"),
        )
}

fn calibration_action_button(
    id: &'static str,
    label: &'static str,
    action: CalibrationAction,
    primary: bool,
    cx: &mut Context<Pss2MidiApp>,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .px(px(theme::SPACE_MD))
        .py(px(theme::SPACE_SM))
        .rounded_md()
        .bg(rgb(if primary {
            theme::ACCENT_TINT
        } else {
            theme::PANEL_INSET
        }))
        .border_1()
        .border_color(rgb(if primary {
            theme::ACCENT
        } else {
            theme::BORDER
        }))
        .text_size(px(theme::FONT_SMALL))
        .text_color(rgb(if primary {
            theme::ACCENT
        } else {
            theme::TEXT_SECONDARY
        }))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.dispatch_calibration_action(action, cx);
        }))
        .child(label)
}

fn calibration_keyboard_panel(view: &CalibrationViewModel) -> impl IntoElement {
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
                        .child("Calibration keyboard · MIDI 36–72"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE_XS))
                        .child(StatusBadge::new("Current", StatusTone::Accent).render())
                        .child(StatusBadge::new("Completed", StatusTone::Success).render())
                        .child(StatusBadge::new("Remaining", StatusTone::Neutral).render()),
                ),
        )
        .child(
            PianoKeyboard::new()
                .current_note(view.current_note())
                .completed_notes(view.completed_notes.iter().copied())
                .render(),
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

fn live_controls(
    state: &AppState,
    audio_device_menu_open: bool,
    settings_view: Option<&SettingsViewModel>,
    cx: &mut Context<Pss2MidiApp>,
) -> impl IntoElement {
    let selected_device = settings_view
        .map(|view| view.selected_audio_device.as_str())
        .unwrap_or(&state.selected_audio_device);
    let choices = settings_view.map_or_else(
        || audio_device_choices(&state.audio_devices, selected_device),
        |view| view.audio_choices.clone(),
    );
    let detector_mode = settings_view.map_or(state.config.detector_mode, |view| view.detector_mode);
    let selected_label = choices
        .iter()
        .find(|choice| choice.selected)
        .map(|choice| choice.label.clone())
        .unwrap_or_else(|| selected_device.to_owned());
    let (audio_status, audio_tone) = if state.audio_device_open {
        ("OPEN", StatusTone::Success)
    } else if state.error_banner.as_ref().is_some_and(|error| {
        matches!(
            error.kind,
            ErrorKind::AudioDevice | ErrorKind::AudioDeviceEnumeration
        )
    }) {
        ("ERROR", StatusTone::Error)
    } else {
        ("NOT OPEN", StatusTone::Neutral)
    };

    let mut device_selector = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_XS))
        .child(
            div()
                .id("audio-device-selector-toggle")
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(theme::SPACE_SM))
                .px(px(theme::SPACE_SM))
                .py(px(theme::SPACE_SM))
                .rounded_md()
                .bg(rgb(theme::PANEL_INSET))
                .border_1()
                .border_color(rgb(theme::BORDER))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.audio_device_menu_open = !this.audio_device_menu_open;
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(theme::FONT_SMALL))
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .child(format!("{selected_label} · {}", selected_device)),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(if audio_device_menu_open { "▲" } else { "▼" }),
                ),
        );

    if audio_device_menu_open {
        let mut menu = div()
            .id("audio-device-selector-menu")
            .w_full()
            .h(px(150.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_XS))
            .p(px(theme::SPACE_XS))
            .rounded_md()
            .bg(rgb(theme::PANEL_INSET))
            .border_1()
            .border_color(rgb(theme::BORDER));
        if state.audio_devices.is_empty() {
            menu = menu.child(
                div()
                    .px(px(theme::SPACE_XS))
                    .py(px(theme::SPACE_XS))
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child("No ALSA inputs listed · Refresh to retry; PipeWire default remains selectable"),
            );
        }
        menu = menu.children(choices.iter().map(|choice| audio_device_choice(choice, cx)));
        device_selector = device_selector.child(menu);
    }

    let mut panel = div()
        .w_full()
        .flex()
        .items_stretch()
        .gap(px(theme::SPACE_SM))
        .child(
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
                        .child(control_heading("AUDIO INPUT"))
                        .child(StatusBadge::new(audio_status, audio_tone).render()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(theme::SPACE_SM))
                        .child(device_selector)
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .gap(px(theme::SPACE_XS))
                                .child(action_button(
                                    "audio-refresh-devices",
                                    "Refresh",
                                    LiveControlAction::EnumerateAudioDevices,
                                    false,
                                    cx,
                                ))
                                .child(action_button(
                                    "audio-retry-device",
                                    "Retry input",
                                    LiveControlAction::RetryAudioDevice,
                                    false,
                                    cx,
                                )),
                        ),
                ),
        )
        .child(
            div()
                .w(px(245.0))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_SM))
                .p(px(theme::SPACE_MD))
                .rounded_md()
                .bg(rgb(theme::PANEL))
                .border_1()
                .border_color(rgb(theme::BORDER))
                .child(control_heading("DETECTOR MODE"))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(theme::SPACE_XS))
                        .child(detector_mode_choice(DetectorMode::Yin, detector_mode, cx))
                        .child(detector_mode_choice(
                            DetectorMode::Spectral,
                            detector_mode,
                            cx,
                        ))
                        .child(detector_mode_choice(
                            DetectorMode::Compare,
                            detector_mode,
                            cx,
                        )),
                )
                .child(template_status_summary(&state.template_status)),
        );

    if let Some(error) = &state.error_banner {
        panel = panel.child(error_banner_panel(error, cx));
    }

    panel
}

fn control_heading(label: &'static str) -> impl IntoElement {
    div()
        .text_size(px(theme::FONT_CAPTION))
        .text_color(rgb(theme::TEXT_MUTED))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

fn action_button(
    id: &'static str,
    label: &'static str,
    action: LiveControlAction,
    primary: bool,
    cx: &mut Context<Pss2MidiApp>,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .px(px(theme::SPACE_SM))
        .py(px(theme::SPACE_XS))
        .rounded_md()
        .bg(rgb(if primary {
            theme::ACCENT_TINT
        } else {
            theme::PANEL_INSET
        }))
        .border_1()
        .border_color(rgb(if primary {
            theme::ACCENT
        } else {
            theme::BORDER
        }))
        .text_size(px(theme::FONT_CAPTION))
        .text_color(rgb(if primary {
            theme::ACCENT
        } else {
            theme::TEXT_SECONDARY
        }))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.dispatch_control_action(action.clone(), cx);
        }))
        .child(label)
}

fn detector_mode_choice(
    mode: DetectorMode,
    selected_mode: DetectorMode,
    cx: &mut Context<Pss2MidiApp>,
) -> impl IntoElement {
    let (id, label) = match mode {
        DetectorMode::Yin => ("detector-mode-yin", "YIN"),
        DetectorMode::Spectral => ("detector-mode-spectral", "Spectral"),
        DetectorMode::Compare => ("detector-mode-compare", "Compare"),
    };
    let selected = mode == selected_mode;
    div()
        .id(id)
        .px(px(theme::SPACE_SM))
        .py(px(theme::SPACE_SM))
        .rounded_md()
        .bg(rgb(if selected {
            theme::ACCENT_TINT
        } else {
            theme::PANEL_INSET
        }))
        .border_1()
        .border_color(rgb(if selected {
            theme::ACCENT
        } else {
            theme::BORDER
        }))
        .text_size(px(theme::FONT_CAPTION))
        .text_color(rgb(if selected {
            theme::ACCENT
        } else {
            theme::TEXT_SECONDARY
        }))
        .font_weight(if selected {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.dispatch_control_action(LiveControlAction::SetDetectorMode(mode), cx);
        }))
        .child(label)
}

fn audio_device_choice(
    choice: &AudioDeviceChoice,
    cx: &mut Context<Pss2MidiApp>,
) -> impl IntoElement {
    let selected = choice.selected;
    let choice_id = choice.id.clone();
    let action = choice.selection_action();
    div()
        .id(choice_id.clone())
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(theme::SPACE_SM))
        .px(px(theme::SPACE_SM))
        .py(px(theme::SPACE_XS))
        .rounded_md()
        .bg(rgb(if selected {
            theme::ACCENT_TINT
        } else {
            theme::PANEL_INSET
        }))
        .border_1()
        .border_color(rgb(if selected {
            theme::ACCENT
        } else {
            theme::BORDER
        }))
        .text_size(px(theme::FONT_CAPTION))
        .text_color(rgb(if selected {
            theme::ACCENT
        } else {
            theme::TEXT_SECONDARY
        }))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.audio_device_menu_open = false;
            this.dispatch_control_action(action.clone(), cx);
            cx.notify();
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(if selected {
                    theme::ACCENT
                } else {
                    theme::TEXT_SECONDARY
                }))
                .child(format!("{} · {}", choice.label, choice.id)),
        )
        .when(selected, |row| {
            row.child(StatusBadge::new("SELECTED", StatusTone::Accent).render())
        })
}

fn error_banner_panel(
    error: &pss2midi::ui::state::ErrorBanner,
    cx: &mut Context<Pss2MidiApp>,
) -> impl IntoElement {
    let can_retry_audio = matches!(
        error.kind,
        ErrorKind::AudioDevice | ErrorKind::AudioDeviceEnumeration
    );
    let mut panel = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(theme::SPACE_SM))
        .px(px(theme::SPACE_MD))
        .py(px(theme::SPACE_SM))
        .rounded_md()
        .bg(rgb(theme::ERROR_TINT))
        .border_1()
        .border_color(rgb(theme::ERROR))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(theme::FONT_SMALL))
                .text_color(rgb(theme::ERROR))
                .child(error.message.clone()),
        );
    if can_retry_audio {
        panel = panel.child(action_button(
            "audio-error-retry",
            "Retry",
            LiveControlAction::RetryAudioDevice,
            false,
            cx,
        ));
    }
    panel
}

fn midi_output_badge(status: &MidiOutputStatus) -> impl IntoElement {
    let (name, label, tone) = match status {
        MidiOutputStatus::NotInitialized => {
            ("PSS-F30 Audio MIDI", "NOT STARTED", StatusTone::Neutral)
        }
        MidiOutputStatus::Available { name } => (name.as_str(), "CONNECTED", StatusTone::Success),
        MidiOutputStatus::Closed => ("PSS-F30 Audio MIDI", "STOPPED", StatusTone::Neutral),
        MidiOutputStatus::Error { .. } => ("PSS-F30 Audio MIDI", "ERROR", StatusTone::Error),
    };
    div()
        .flex()
        .items_center()
        .gap(px(theme::SPACE_XS))
        .px(px(theme::SPACE_SM))
        .py(px(theme::SPACE_XS))
        .rounded_md()
        .bg(rgb(theme::PANEL_INSET))
        .child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(name.to_owned()),
        )
        .child(StatusBadge::new(label, tone).render())
}

fn template_status_summary(status: &TemplateStatus) -> impl IntoElement {
    let (label, tone, detail) = match status {
        TemplateStatus::NotCalibrated { path } => (
            "NOT CALIBRATED",
            StatusTone::Warning,
            format!("Spectral detection needs calibration · {}", path.display()),
        ),
        TemplateStatus::Loaded { path } => (
            "CALIBRATED",
            StatusTone::Success,
            format!("Loaded templates · {}", path.display()),
        ),
        TemplateStatus::Error { path, message } => (
            "ERROR",
            StatusTone::Error,
            format!("{} · {message}", path.display()),
        ),
    };
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(theme::SPACE_SM))
        .px(px(theme::SPACE_SM))
        .py(px(theme::SPACE_XS))
        .rounded_md()
        .bg(rgb(theme::PANEL_INSET))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(detail),
        )
        .child(StatusBadge::new(label, tone).render())
}

fn top_bar(state: &AppState, cx: &mut Context<Pss2MidiApp>) -> impl IntoElement {
    let (control_label, control_tone) = match state.engine_state {
        EngineState::Stopped => ("Start", StatusTone::Success),
        EngineState::Running => ("Stop", StatusTone::Warning),
        EngineState::ShuttingDown => ("Closing…", StatusTone::Neutral),
    };
    let mut control = div()
        .id("engine-start-stop")
        .flex_none()
        .px(px(theme::SPACE_MD))
        .py(px(theme::SPACE_SM))
        .rounded_md()
        .bg(rgb(match control_tone {
            StatusTone::Success => theme::SUCCESS_TINT,
            StatusTone::Warning => theme::WARNING_TINT,
            _ => theme::PANEL_INSET,
        }))
        .border_1()
        .border_color(rgb(match control_tone {
            StatusTone::Success => theme::SUCCESS,
            StatusTone::Warning => theme::WARNING,
            _ => theme::BORDER,
        }))
        .text_size(px(theme::FONT_SMALL))
        .text_color(rgb(match control_tone {
            StatusTone::Success => theme::SUCCESS,
            StatusTone::Warning => theme::WARNING,
            _ => theme::TEXT_MUTED,
        }))
        .font_weight(FontWeight::SEMIBOLD);
    if state.engine_state != EngineState::ShuttingDown {
        control = control
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.dispatch_control_action(LiveControlAction::ToggleEngine, cx);
            }));
    }
    control = control.child(control_label);

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
                .child(midi_output_badge(&state.midi_output_status))
                .child(control),
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
        cx.new(Pss2MidiApp::new)
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
