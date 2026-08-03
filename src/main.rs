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

    // Starts true (not false-then-flipped): other logic (e.g. shortcuts.rs's
    // escape-grab demand check) reads this Cell from the moment the app
    // starts, before the deferred idle callback below has had a chance to
    // run — if it started false, that logic would briefly believe our UI
    // was open (click-through "off") and race to grab shortcuts it doesn't
    // need yet.
    let click_through = Rc::new(Cell::new(true));
    let window_for_toggle = window.clone();
    let toggle_for_click = toggle.clone();
    let click_through_for_click = click_through.clone();
    let events_for_click = backend.events.clone();
    toggle.connect_clicked(move |_btn| {
        toggle_click_through(
            &window_for_toggle,
            &toggle_for_click,
            &click_through_for_click,
            &events_for_click,
        );
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
    // click_through's tracked state already defaults to true (see above);
    // this just applies that to the actual surface once layout has
    // happened. Deferred to idle: compute_bounds (used to carve out the
    // toggle button's input-region exception) needs the button to have
    // already been laid out, which hasn't happened yet immediately after
    // present().
    {
        let window = window.clone();
        let toggle = toggle.clone();
        glib::idle_add_local_once(move || {
            apply_click_through_to_surface(true, &window, &toggle);
        });
    }

    let active_window: Rc<RefCell<Option<kwin_tracker::WindowEvent>>> = Rc::new(RefCell::new(None));
    let (sender, receiver) = async_channel::unbounded::<TrackerEvent>();
    let conn_rx = kwin_tracker::spawn(sender);
    {
        let active_window = active_window.clone();
        let window = window.clone();
        let toggle_for_monitor = toggle.clone();
        let click_through_for_monitor = click_through.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = receiver.recv().await {
                match event {
                    TrackerEvent::Activated(w) => {
                        println!(
                            "[kwin_tracker] activated: class={:?} caption={:?} pid={} {}x{} @ ({}, {})",
                            w.class_name, w.caption, w.pid, w.width, w.height, w.x, w.y
                        );
                        // Layer-shell surfaces with no explicit monitor set
                        // just go wherever the compositor's default output
                        // is — which isn't necessarily where the game
                        // actually is on a multi-monitor setup. Follow PoE.
                        if w.is_path_of_exile() {
                            move_overlay_to_window_monitor(
                                &window,
                                &w,
                                &toggle_for_monitor,
                                &click_through_for_monitor,
                            );
                        }
                        active_window.replace(Some(w));
                    }
                    TrackerEvent::GeometryChanged(w) => println!(
                        "[kwin_tracker] geometry: class={:?} caption={:?} {}x{} @ ({}, {})",
                        w.class_name, w.caption, w.width, w.height, w.x, w.y
                    ),
                }
            }
        });
    }

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
        let active_window = active_window.clone();
        let focus_game_rx = backend.focus_game_rx.clone();
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

            shortcuts::spawn(
                window,
                toggle,
                click_through,
                kwin_connection,
                remote_input,
                events,
                active_window,
                focus_game_rx,
            );
        });
    }
}

/// `can-target` only affects GTK's own internal hit-testing, not the input
/// region the compositor is told about — that requires poking the surface
/// directly, or clicks keep landing on our window no matter what.
fn toggle_click_through(
    window: &ApplicationWindow,
    toggle: &Button,
    click_through: &Rc<Cell<bool>>,
    events: &server::EventBus,
) {
    set_click_through(!click_through.get(), window, toggle, click_through, events);
}

/// Like `toggle_click_through`, but sets an absolute state rather than
/// flipping — needed by anything that decides the *desired* state itself
/// (e.g. `widget_area_tracker`'s mouse-distance logic) rather than reacting
/// to a keypress.
pub(crate) fn set_click_through(
    now_click_through: bool,
    window: &ApplicationWindow,
    toggle: &Button,
    click_through: &Rc<Cell<bool>>,
    events: &server::EventBus,
) {
    if click_through.get() == now_click_through {
        return;
    }
    click_through.set(now_click_through);
    apply_click_through_to_surface(now_click_through, window, toggle);

    // The renderer only auto-hides "hide-on-blur" popups (e.g. the
    // price-check window) in reaction to this event
    // (renderer/src/web/overlay/OverlayWindow.vue) — without it, anything
    // shown via item-text just stays on screen forever. click-through ON
    // maps to "overlay not active" (game usable, so any lingering popup
    // should go away); click-through OFF maps to "overlay active".
    events.broadcast(
        "MAIN->OVERLAY::focus-change",
        serde_json::json!({
            "game": now_click_through,
            "overlay": !now_click_through,
            "usingHotkey": true,
        }),
    );
}

/// Applies `now_click_through` to the surface's actual Wayland input region,
/// unconditionally — split out of `set_click_through` so callers that need
/// to force a re-apply (the surface was remapped and may have silently
/// dropped its input region) can do so without `set_click_through`'s
/// early-return-if-state-unchanged guard getting in the way.
fn apply_click_through_to_surface(now_click_through: bool, window: &ApplicationWindow, toggle: &Button) {
    let Some(surface) = window.surface() else { return };
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

/// A layer-shell surface with no explicit monitor set just lands on
/// whatever output the compositor considers default — not necessarily
/// where the game actually is on a multi-monitor setup. gtk4-layer-shell
/// supports changing this on an already-mapped surface, but its own docs
/// warn it "gets remapped so the change can take effect" — that remap was
/// silently resetting the Wayland input region set for click-through,
/// leaving the overlay fully click-blocking after every monitor switch even
/// though our tracked state still said click-through was on. Re-applying
/// the input region right after fixes it; deferred to idle so the remap
/// (which isn't necessarily synchronous with `set_monitor()` returning) has
/// actually landed first.
fn move_overlay_to_window_monitor(
    window: &ApplicationWindow,
    target: &kwin_tracker::WindowEvent,
    toggle: &Button,
    click_through: &Rc<Cell<bool>>,
) {
    let display = WidgetExt::display(window);
    let center_x = target.x + target.width / 2.0;
    let center_y = target.y + target.height / 2.0;

    let monitors = display.monitors();
    for i in 0..monitors.n_items() {
        let Some(obj) = monitors.item(i) else { continue };
        let Ok(monitor) = obj.downcast::<gtk4::gdk::Monitor>() else { continue };
        let geo = monitor.geometry();
        let (mx, my, mw, mh) = (
            geo.x() as f64,
            geo.y() as f64,
            geo.width() as f64,
            geo.height() as f64,
        );
        if center_x >= mx && center_x < mx + mw && center_y >= my && center_y < my + mh {
            if window.monitor().as_ref() != Some(&monitor) {
                println!("[main] moving overlay to PoE's monitor");
                window.set_monitor(Some(&monitor));

                let window = window.clone();
                let toggle = toggle.clone();
                let click_through = click_through.clone();
                glib::idle_add_local_once(move || {
                    apply_click_through_to_surface(click_through.get(), &window, &toggle);
                });
            }
            return;
        }
    }
}
