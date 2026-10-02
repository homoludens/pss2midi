// These reusable widgets are introduced before their page integration tasks.
#[allow(dead_code)]
#[path = "pss2midi_gui/components.rs"]
mod components;
#[path = "pss2midi_gui/live.rs"]
mod live;
#[allow(dead_code)]
#[path = "pss2midi_gui/piano.rs"]
mod piano;
#[path = "pss2midi_gui/shell.rs"]
mod shell;
#[allow(dead_code)]
#[path = "pss2midi_gui/theme.rs"]
mod theme;

fn main() {
    gpui_platform::application().run(shell::launch);
}
