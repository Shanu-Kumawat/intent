use gpui::{App, Application, Bounds, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size};

struct Shell;

impl Render for Shell {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x000000))
            .text_color(rgb(0xffffff))
            .child("INTENT")
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(960.0), px(640.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Shell),
        )
        .unwrap();
        cx.activate(true);
    });
}
