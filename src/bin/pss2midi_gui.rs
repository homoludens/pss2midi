use gpui::{div, prelude::*, App, Context, Window, WindowOptions};

struct Pss2MidiApp;

impl Render for Pss2MidiApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child("PSS2MIDI")
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| Pss2MidiApp))
            .expect("failed to open GPUI window");
    });
}
