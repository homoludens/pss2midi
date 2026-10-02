#[path = "pss2midi_gui/shell.rs"]
mod shell;
#[path = "pss2midi_gui/theme.rs"]
mod theme;

fn main() {
    gpui_platform::application().run(shell::launch);
}
