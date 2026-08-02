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

    let webview_for_toggle = webview.clone();
    toggle.connect_clicked(move |btn| {
        // can_target == true means the webview receives clicks (click-through is OFF).
        let new_can_target = !webview_for_toggle.can_target();
        webview_for_toggle.set_can_target(new_can_target);
        btn.set_label(if new_can_target {
            "click-through: OFF"
        } else {
            "click-through: ON"
        });
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
