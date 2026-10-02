//! Display-only 37-key piano and its pure chromatic layout model.

use std::collections::BTreeSet;

use gpui::{div, prelude::*, px, relative, rgb, FontWeight};

use crate::theme;

pub(crate) const FIRST_MIDI_NOTE: u8 = 36;
pub(crate) const LAST_MIDI_NOTE: u8 = 72;
const WHITE_KEY_COUNT: usize = 22;
const BLACK_KEY_WIDTH_IN_WHITE_KEYS: f32 = 0.62;
const KEYBED_HEIGHT: f32 = 116.0;
const BLACK_KEY_HEIGHT: f32 = 74.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PianoKeyKind {
    White { index: usize },
    Black { after_white_index: usize },
}

/// Normalized key geometry; `left` and `width` are fractions of the full keybed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PianoKeyLayout {
    pub(crate) midi_note: u8,
    pub(crate) kind: PianoKeyKind,
    pub(crate) left: f32,
    pub(crate) width: f32,
    pub(crate) octave_label: Option<&'static str>,
}

/// Return keys in chromatic order with black-key centers aligned to white-key boundaries.
pub(crate) fn piano_key_layout() -> Vec<PianoKeyLayout> {
    (FIRST_MIDI_NOTE..=LAST_MIDI_NOTE)
        .map(|midi_note| {
            let octave_label = match midi_note {
                36 => Some("C2"),
                48 => Some("C3"),
                60 => Some("C4"),
                72 => Some("C5"),
                _ => None,
            };

            if is_black_key(midi_note) {
                let white_keys_before = (FIRST_MIDI_NOTE..midi_note)
                    .filter(|note| !is_black_key(*note))
                    .count();
                let width = BLACK_KEY_WIDTH_IN_WHITE_KEYS / WHITE_KEY_COUNT as f32;
                let left = (white_keys_before as f32 - BLACK_KEY_WIDTH_IN_WHITE_KEYS / 2.0)
                    / WHITE_KEY_COUNT as f32;
                PianoKeyLayout {
                    midi_note,
                    kind: PianoKeyKind::Black {
                        after_white_index: white_keys_before - 1,
                    },
                    left,
                    width,
                    octave_label,
                }
            } else {
                let index = (FIRST_MIDI_NOTE..midi_note)
                    .filter(|note| !is_black_key(*note))
                    .count();
                PianoKeyLayout {
                    midi_note,
                    kind: PianoKeyKind::White { index },
                    left: index as f32 / WHITE_KEY_COUNT as f32,
                    width: 1.0 / WHITE_KEY_COUNT as f32,
                    octave_label,
                }
            }
        })
        .collect()
}

fn is_black_key(midi_note: u8) -> bool {
    matches!(midi_note % 12, 1 | 3 | 6 | 8 | 10)
}

/// Reusable display-only keyboard. The current and selected note share a blue highlight.
#[derive(Clone, Debug, Default)]
pub(crate) struct PianoKeyboard {
    selected_note: Option<u8>,
    current_note: Option<u8>,
    yin_note: Option<u8>,
    completed_notes: BTreeSet<u8>,
}

impl PianoKeyboard {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn selected_note(mut self, note: Option<u8>) -> Self {
        self.selected_note = note;
        self
    }

    pub(crate) fn current_note(mut self, note: Option<u8>) -> Self {
        self.current_note = note;
        self
    }

    pub(crate) fn yin_note(mut self, note: Option<u8>) -> Self {
        self.yin_note = note;
        self
    }

    pub(crate) fn completed_notes(mut self, notes: impl IntoIterator<Item = u8>) -> Self {
        self.completed_notes = notes.into_iter().collect();
        self
    }

    pub(crate) fn render(&self) -> impl IntoElement {
        let layout = piano_key_layout();
        let highlighted_note = self.current_note.or(self.selected_note);
        let white_keys = layout
            .iter()
            .filter(|key| matches!(key.kind, PianoKeyKind::White { .. }))
            .map(|key| {
                white_key(
                    *key,
                    highlighted_note == Some(key.midi_note),
                    self.yin_note == Some(key.midi_note) && highlighted_note != Some(key.midi_note),
                    self.completed_notes.contains(&key.midi_note),
                )
            });
        let black_keys = layout
            .iter()
            .filter(|key| matches!(key.kind, PianoKeyKind::Black { .. }))
            .map(|key| {
                black_key(
                    *key,
                    highlighted_note == Some(key.midi_note),
                    self.yin_note == Some(key.midi_note) && highlighted_note != Some(key.midi_note),
                    self.completed_notes.contains(&key.midi_note),
                )
            });
        let octave_labels = layout
            .iter()
            .filter(|key| key.octave_label.is_some())
            .map(|key| octave_label(*key));

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_XS))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(KEYBED_HEIGHT))
                    .overflow_hidden()
                    .rounded_sm()
                    .bg(rgb(theme::PIANO_BLACK_KEY))
                    .child(div().w_full().h_full().flex().children(white_keys))
                    .children(black_keys),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(18.0))
                    .children(octave_labels),
            )
    }
}

fn white_key(
    key: PianoKeyLayout,
    highlighted: bool,
    yin_marker: bool,
    completed: bool,
) -> impl IntoElement {
    let fill = if highlighted {
        theme::ACCENT
    } else {
        theme::PIANO_WHITE_KEY
    };

    div()
        .id(format!("piano-white-{}", key.midi_note))
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .border_r_1()
        .border_b_1()
        .border_color(rgb(theme::BORDER))
        .bg(rgb(fill))
        .child(marker(
            yin_marker,
            theme::WARNING,
            fill,
            MarkerPosition::TopLeft,
        ))
        .child(marker(
            completed,
            theme::SUCCESS,
            fill,
            MarkerPosition::BottomRight,
        ))
}

fn black_key(
    key: PianoKeyLayout,
    highlighted: bool,
    yin_marker: bool,
    completed: bool,
) -> impl IntoElement {
    let fill = if highlighted {
        theme::ACCENT
    } else {
        theme::PIANO_BLACK_KEY
    };

    div()
        .id(format!("piano-black-{}", key.midi_note))
        .absolute()
        .top(px(0.0))
        .left(relative(key.left))
        .w(relative(key.width))
        .h(px(BLACK_KEY_HEIGHT))
        .rounded_b_sm()
        .border_1()
        .border_color(rgb(theme::BORDER))
        .bg(rgb(fill))
        .child(marker(
            yin_marker,
            theme::WARNING,
            fill,
            MarkerPosition::TopCenter,
        ))
        .child(marker(
            completed,
            theme::SUCCESS,
            fill,
            MarkerPosition::BottomCenter,
        ))
}

#[derive(Clone, Copy)]
enum MarkerPosition {
    TopLeft,
    BottomRight,
    TopCenter,
    BottomCenter,
}

fn marker(visible: bool, color: u32, key_fill: u32, position: MarkerPosition) -> impl IntoElement {
    let mut marker = div()
        .size(px(6.0))
        .absolute()
        .rounded_full()
        .bg(rgb(if visible { color } else { key_fill }));

    marker = match position {
        MarkerPosition::TopLeft => marker.top(px(5.0)).left(px(5.0)),
        MarkerPosition::BottomRight => marker.bottom(px(5.0)).right(px(5.0)),
        MarkerPosition::TopCenter => marker.top(px(5.0)).left(relative(0.5)).ml(px(-3.0)),
        MarkerPosition::BottomCenter => marker.bottom(px(5.0)).left(relative(0.5)).ml(px(-3.0)),
    };

    marker
}

fn octave_label(key: PianoKeyLayout) -> impl IntoElement {
    div()
        .absolute()
        .top(px(0.0))
        .left(relative(key.left))
        .w(relative(key.width))
        .flex()
        .justify_center()
        .text_size(px(theme::FONT_CAPTION))
        .text_color(rgb(theme::TEXT_SECONDARY))
        .font_weight(FontWeight::SEMIBOLD)
        .child(key.octave_label.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::{piano_key_layout, PianoKeyKind, FIRST_MIDI_NOTE, LAST_MIDI_NOTE};

    const EXPECTED_WHITE_NOTES: [u8; 22] = [
        36, 38, 40, 41, 43, 45, 47, 48, 50, 52, 53, 55, 57, 59, 60, 62, 64, 65, 67, 69, 71, 72,
    ];
    const EXPECTED_BLACK_NOTES: [u8; 15] =
        [37, 39, 42, 44, 46, 49, 51, 54, 56, 58, 61, 63, 66, 68, 70];
    const EXPECTED_BLACK_AFTER_WHITE: [usize; 15] =
        [0, 1, 3, 4, 5, 7, 8, 10, 11, 12, 14, 15, 17, 18, 19];

    #[test]
    fn mapping_covers_every_midi_note_in_chromatic_order() {
        let layout = piano_key_layout();
        assert_eq!(layout.len(), 37);
        assert_eq!(layout.first().unwrap().midi_note, FIRST_MIDI_NOTE);
        assert_eq!(layout.last().unwrap().midi_note, LAST_MIDI_NOTE);
        assert_eq!(
            layout.iter().map(|key| key.midi_note).collect::<Vec<_>>(),
            (FIRST_MIDI_NOTE..=LAST_MIDI_NOTE).collect::<Vec<_>>()
        );
    }

    #[test]
    fn white_and_black_keys_keep_chromatic_order_and_counts() {
        let layout = piano_key_layout();
        let white_notes = layout
            .iter()
            .filter_map(|key| match key.kind {
                PianoKeyKind::White { .. } => Some(key.midi_note),
                PianoKeyKind::Black { .. } => None,
            })
            .collect::<Vec<_>>();
        let black_notes = layout
            .iter()
            .filter_map(|key| match key.kind {
                PianoKeyKind::White { .. } => None,
                PianoKeyKind::Black { .. } => Some(key.midi_note),
            })
            .collect::<Vec<_>>();

        assert_eq!(white_notes, EXPECTED_WHITE_NOTES);
        assert_eq!(black_notes, EXPECTED_BLACK_NOTES);
        assert_eq!(white_notes.len(), 22);
        assert_eq!(black_notes.len(), 15);
    }

    #[test]
    fn white_slots_and_black_key_boundaries_are_positioned_exactly() {
        let layout = piano_key_layout();
        let white_width = 1.0 / 22.0;
        let black_width = 0.62 / 22.0;
        let mut black_after_white = Vec::new();

        for key in &layout {
            match key.kind {
                PianoKeyKind::White { index } => {
                    assert!((key.left - index as f32 * white_width).abs() < f32::EPSILON);
                    assert!((key.width - white_width).abs() < f32::EPSILON);
                }
                PianoKeyKind::Black { after_white_index } => {
                    let boundary = (after_white_index + 1) as f32 * white_width;
                    assert!((key.width - black_width).abs() < f32::EPSILON);
                    assert!((key.left + key.width / 2.0 - boundary).abs() < f32::EPSILON);
                    black_after_white.push(after_white_index);
                }
            }
        }

        assert_eq!(black_after_white, EXPECTED_BLACK_AFTER_WHITE);
    }

    #[test]
    fn octave_labels_mark_c2_through_c5() {
        let labels = piano_key_layout()
            .into_iter()
            .filter_map(|key| key.octave_label.map(|label| (key.midi_note, label)))
            .collect::<Vec<_>>();

        assert_eq!(labels, [(36, "C2"), (48, "C3"), (60, "C4"), (72, "C5")]);
    }
}
