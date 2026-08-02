mod config_store;
mod kwin_tracker;
mod logger;
mod proxy;
mod remote_input;
mod server;
mod shortcuts;
mod uploads;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::cairo::{RectangleInt, Region};
use gtk4::prelude::*;
use gtk4::{glib, Application, ApplicationWindow, Button, Overlay};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use kwin_tracker::TrackerEvent;
use webkit6::prelude::*;
use webkit6::WebView;

pub(crate) const APP_ID: &str = "dev.spike.apt_wayland_overlay";

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
    // Confirmed via testing: a layer-shell surface that never holds keyboard
    // focus also can't reliably read gdk::Clipboard (Wayland gates clipboard
    // visibility by focus for privacy). Since the overlay is meant to never
    // steal focus from the game, clipboard reads for the price-check flow go
    // through the portal's Clipboard interface instead (remote_input.rs),
    // which works regardless of local focus — so this can stay None.
    window.set_keyboard_mode(KeyboardMode::None);

    let (port, backend) = server::spawn().expect("failed to start local server");
    println!("[main] server listening on 127.0.0.1:{port}");

    let webview = WebView::new();
    webview.set_background_color(&gtk4::gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
    // The renderer only enables overlay-mode behavior (vs. plain-browser
    // mode) when navigator.userAgent contains "Electron"
    // (renderer/src/web/background/IPC.ts) — we're WebKitGTK, not Electron,
    // so without this it would silently fall back to browser mode.
    if let Some(settings) = webkit6::prelude::WebViewExt::settings(&webview) {
        let ua = settings.user_agent().map(|s| s.to_string()).unwrap_or_default();
        settings.set_user_agent(Some(&format!("{ua} Electron/32.0.0")));
    }
    webview.load_uri(&format!("http://127.0.0.1:{port}/"));

    let toggle = Button::with_label("click-through: OFF");
    toggle.set_halign(gtk4::Align::End);
    toggle.set_valign(gtk4::Align::Start);
    toggle.set_margin_top(12);
    toggle.set_margin_end(12);

    let click_through = Rc::new(Cell::new(false));
    let window_for_toggle = window.clone();
    let toggle_for_click = toggle.clone();
    let click_through_for_click = click_through.clone();
    toggle.connect_clicked(move |_btn| {
        toggle_click_through(&window_for_toggle, &toggle_for_click, &click_through_for_click);
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

    let (sender, receiver) = async_channel::unbounded::<TrackerEvent>();
    let conn_rx = kwin_tracker::spawn(sender);
    glib::spawn_future_local(async move {
        while let Ok(event) = receiver.recv().await {
            match event {
                TrackerEvent::Activated(w) => println!(
                    "[kwin_tracker] activated: class={:?} caption={:?} pid={} {}x{} @ ({}, {})",
                    w.class_name, w.caption, w.pid, w.width, w.height, w.x, w.y
                ),
                TrackerEvent::GeometryChanged(w) => println!(
                    "[kwin_tracker] geometry: class={:?} caption={:?} {}x{} @ ({}, {})",
                    w.class_name, w.caption, w.width, w.height, w.x, w.y
                ),
            }
        }
    });

    let kwin_connection: Rc<RefCell<Option<zbus::Connection>>> = Rc::new(RefCell::new(None));
    {
        let kwin_connection = kwin_connection.clone();
        glib::spawn_future_local(async move {
            if let Ok(connection) = conn_rx.recv().await {
                kwin_connection.replace(Some(connection));
            }
        });
    }

    let remote_input: Rc<RefCell<Option<remote_input::RemoteInput>>> = Rc::new(RefCell::new(None));
    {
        let window = window.clone();
        let toggle = toggle.clone();
        let click_through = click_through.clone();
        let kwin_connection = kwin_connection.clone();
        let remote_input = remote_input.clone();
        let events = backend.events.clone();
        glib::spawn_future_local(async move {
            // Launched from a terminal (no systemd app-scope), so the portal
            // can't derive our app id on its own — register it explicitly
            // first, once, or every app-id-gated portal call (GlobalShortcuts,
            // RemoteDesktop) fails with "An app id is required". The portal
            // errors if this is called twice, so it happens exactly once here
            // rather than inside shortcuts.rs or remote_input.rs.
            if let Err(err) = ashpd::register_host_app(APP_ID.try_into().unwrap()).await {
                eprintln!("[main] failed to register host app: {err}");
                return;
            }

            match remote_input::RemoteInput::setup().await {
                Ok(ri) => {
                    remote_input.replace(Some(ri));
                }
                Err(err) => eprintln!("[main] remote_input setup failed: {err}"),
            }

            shortcuts::spawn(window, toggle, click_through, kwin_connection, remote_input, events);
        });
    }
}

/// `can-target` only affects GTK's own internal hit-testing, not the input
/// region the compositor is told about — that requires poking the surface
/// directly, or clicks keep landing on our window no matter what.
fn toggle_click_through(window: &ApplicationWindow, toggle: &Button, click_through: &Rc<Cell<bool>>) {
    let Some(surface) = window.surface() else { return };
    let now_click_through = !click_through.get();
    click_through.set(now_click_through);

    if now_click_through {
        // Empty input region except for the toggle button itself, so it's
        // still reachable to flip this back off.
        let region = Region::create();
        if let Some(bounds) = toggle.compute_bounds(window) {
            let _ = region.union_rectangle(&RectangleInt::new(
                bounds.x() as i32,
                bounds.y() as i32,
                bounds.width() as i32,
                bounds.height() as i32,
            ));
        }
        surface.set_input_region(Some(&region));
        toggle.set_label("click-through: ON");
    } else {
        surface.set_input_region(None);
        toggle.set_label("click-through: OFF");
    }
}
