use std::cell::Cell;
use std::rc::Rc;

use gtk4::cairo::{RectangleInt, Region};
use gtk4::prelude::*;
use gtk4::{glib, Application, ApplicationWindow, Button, Overlay};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use webkit6::prelude::*;
use webkit6::WebView;

const APP_ID: &str = "dev.spike.apt_wayland_overlay";
const APT_URL: &str = "http://127.0.0.1:8584/index.html";

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("apt-wayland-overlay spike")
        .build();

    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Bottom, true);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Right, true);
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::OnDemand);

    let webview = WebView::new();
    webview.set_background_color(&gtk4::gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
    webview.load_uri(APT_URL);

    let toggle = Button::with_label("click-through: OFF");
    toggle.set_halign(gtk4::Align::End);
    toggle.set_valign(gtk4::Align::Start);
    toggle.set_margin_top(12);
    toggle.set_margin_end(12);

    // `can-target` only affects GTK's own internal hit-testing, not the input
    // region the compositor is told about — that requires poking the surface
    // directly, or clicks keep landing on our window no matter what.
    let click_through = Rc::new(Cell::new(false));
    let window_for_toggle = window.clone();
    toggle.connect_clicked(move |btn| {
        let Some(surface) = window_for_toggle.surface() else { return };
        let now_click_through = !click_through.get();
        click_through.set(now_click_through);

        if now_click_through {
            // Empty input region except for the toggle button itself, so it's
            // still reachable to flip this back off.
            let region = Region::create();
            if let Some(bounds) = btn.compute_bounds(&window_for_toggle) {
                let _ = region.union_rectangle(&RectangleInt::new(
                    bounds.x() as i32,
                    bounds.y() as i32,
                    bounds.width() as i32,
                    bounds.height() as i32,
                ));
            }
            surface.set_input_region(Some(&region));
            btn.set_label("click-through: ON");
        } else {
            surface.set_input_region(None);
            btn.set_label("click-through: OFF");
        }
    });

    let overlay = Overlay::new();
    overlay.set_child(Some(&webview));
    overlay.add_overlay(&toggle);

    window.set_child(Some(&overlay));

    // Transparent CSS background so the layer surface itself doesn't paint opaque.
    let css = gtk4::CssProvider::new();
    css.load_from_string("window, overlay { background: transparent; }");
    gtk4::style_context_add_provider_for_display(
        &WidgetExt::display(&window),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    window.present();
}
