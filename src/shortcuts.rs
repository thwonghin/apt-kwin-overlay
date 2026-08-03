use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use ashpd::desktop::CreateSessionOptions;
use ashpd::WindowIdentifier;
use futures_util::StreamExt;
use gtk4::{glib, ApplicationWindow};
use serde_json::json;

use crate::kwin_tracker::WindowEvent;
use crate::remote_input::RemoteInput;
use crate::server::EventBus;
use crate::{set_click_through, toggle_click_through};

const TOGGLE_OVERLAY_ID: &str = "toggle-overlay";
const PRICE_CHECK_ID: &str = "price-check";
const PRICE_CHECK_LOCKED_ID: &str = "price-check-locked";

pub fn spawn(
    window: ApplicationWindow,
    click_through: Rc<Cell<bool>>,
    cursor_pos: Rc<Cell<(f64, f64)>>,
    remote_input: Rc<RefCell<Option<RemoteInput>>>,
    events: Arc<EventBus>,
    active_window: Rc<RefCell<Option<WindowEvent>>>,
    focus_game_rx: async_channel::Receiver<()>,
) {
    let price_check_open = Rc::new(Cell::new(false));
    let price_check_locked = Rc::new(Cell::new(false));

    {
        let window = window.clone();
        let click_through = click_through.clone();
        let events = events.clone();
        let price_check_open = price_check_open.clone();
        let price_check_locked = price_check_locked.clone();
        glib::spawn_future_local(async move {
            // The renderer's own title-bar "x" button sends this instead of
            // just hiding itself locally (main/src/windowing/OverlayWindow.ts's
            // assertGameActive handler for the same event) — it expects us to
            // actually restore click-through and close whatever's open.
            while focus_game_rx.recv().await.is_ok() {
                close_all_ui(&window, &click_through, &events, &price_check_open, &price_check_locked);
            }
        });
    }

    glib::spawn_future_local(async move {
        if let Err(err) = run(
            &window,
            &click_through,
            &cursor_pos,
            &remote_input,
            &events,
            &price_check_open,
            &price_check_locked,
            &active_window,
        )
        .await
        {
            eprintln!("[shortcuts] error: {err}");
        }
    });
}

/// Force click-through back on and close any open overlay widgets, as an
/// always-available safety hatch regardless of what's currently showing.
/// Matches real APT's `assertGameActive` (main/src/windowing/OverlayWindow.ts).
fn close_all_ui(
    window: &ApplicationWindow,
    click_through: &Rc<Cell<bool>>,
    events: &Arc<EventBus>,
    price_check_open: &Rc<Cell<bool>>,
    price_check_locked: &Rc<Cell<bool>>,
) {
    if price_check_open.get() {
        events.broadcast("MAIN->OVERLAY::hide-exclusive-widget", serde_json::Value::Null);
        price_check_open.set(false);
    }
    price_check_locked.set(false);
    set_click_through(true, window, click_through, events);
}

async fn run(
    window: &ApplicationWindow,
    click_through: &Rc<Cell<bool>>,
    cursor_pos: &Rc<Cell<(f64, f64)>>,
    remote_input: &Rc<RefCell<Option<RemoteInput>>>,
    events: &Arc<EventBus>,
    price_check_open: &Rc<Cell<bool>>,
    price_check_locked: &Rc<Cell<bool>>,
    active_window: &Rc<RefCell<Option<WindowEvent>>>,
) -> ashpd::Result<()> {
    let proxy = GlobalShortcuts::new().await?;
    let session = proxy.create_session(CreateSessionOptions::default()).await?;

    let window_id = WindowIdentifier::from_native(window).await;
    let shortcuts = [
        NewShortcut::new(TOGGLE_OVERLAY_ID, "Toggle click-through")
            .preferred_trigger(Some("SHIFT+space")),
        NewShortcut::new(PRICE_CHECK_ID, "Price-check hovered item")
            .preferred_trigger(Some("CTRL+d")),
        NewShortcut::new(PRICE_CHECK_LOCKED_ID, "Price-check hovered item (locked open)")
            .preferred_trigger(Some("CTRL+ALT+d")),
    ];

    let request = proxy
        .bind_shortcuts(
            &session,
            &shortcuts,
            window_id.as_ref(),
            BindShortcutsOptions::default(),
        )
        .await?;
    let bound = request.response()?;
    for s in bound.shortcuts() {
        println!(
            "[shortcuts] bound {} -> {}",
            s.id(),
            s.trigger_description()
        );
    }

    // Debounce: the portal has been observed emitting multiple Activated
    // signals for what looked like a single physical keypress (several
    // rapid modifier transitions logged for one press during testing) —
    // without this, that means duplicate Ctrl+C injections and duplicate
    // proxy/network requests per press.
    const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);
    let mut last_activation: Option<std::time::Instant> = None;

    let mut activated = proxy.receive_activated().await?;
    while let Some(event) = activated.next().await {
        let now = std::time::Instant::now();
        if last_activation.is_some_and(|t| now.duration_since(t) < DEBOUNCE) {
            continue;
        }
        last_activation = Some(now);

        match event.shortcut_id() {
            TOGGLE_OVERLAY_ID => toggle_click_through(window, click_through, events),
            PRICE_CHECK_ID => {
                price_check(
                    window,
                    click_through,
                    cursor_pos,
                    remote_input,
                    events,
                    price_check_open,
                    price_check_locked,
                    active_window,
                    false,
                )
                .await
            }
            PRICE_CHECK_LOCKED_ID => {
                price_check(
                    window,
                    click_through,
                    cursor_pos,
                    remote_input,
                    events,
                    price_check_open,
                    price_check_locked,
                    active_window,
                    true,
                )
                .await
            }
            _ => {}
        }
    }

    Ok(())
}

async fn price_check(
    window: &ApplicationWindow,
    click_through: &Rc<Cell<bool>>,
    cursor_pos: &Rc<Cell<(f64, f64)>>,
    remote_input: &Rc<RefCell<Option<RemoteInput>>>,
    events: &Arc<EventBus>,
    price_check_open: &Rc<Cell<bool>>,
    price_check_locked: &Rc<Cell<bool>>,
    active_window: &Rc<RefCell<Option<WindowEvent>>>,
    locked: bool,
) {
    println!(
        "[shortcuts] price_check called, locked={locked}, price_check_open={}",
        price_check_open.get()
    );
    // Pressing the same hotkey again while the popup's open closes it --
    // hide-exclusive-widget targets exactly this widget (unlike
    // focus-change, which affects every hide-on-blur/hide-on-focus widget).
    if price_check_open.get() {
        events.broadcast("MAIN->OVERLAY::hide-exclusive-widget", serde_json::Value::Null);
        price_check_open.set(false);
        if price_check_locked.get() {
            // Opening the locked popup switched click-through off so it
            // could be interacted with; closing it should give control of
            // the game back rather than leaving the user stuck interactive.
            set_click_through(true, window, click_through, events);
            price_check_locked.set(false);
        }
        return;
    }

    // The global shortcut fires regardless of which window has focus; without
    // this check, pressing it over any other app (browser, terminal, ...)
    // would Ctrl+C-and-read whatever text is selected there. Only forward the
    // hotkey to the game.
    let is_poe = active_window
        .borrow()
        .as_ref()
        .is_some_and(WindowEvent::is_path_of_exile);
    if !is_poe {
        println!("[shortcuts] price-check ignored: active window is not Path of Exile");
        return;
    }

    let (x, y) = cursor_pos.get();

    let remote_ref = remote_input.borrow();
    let Some(remote) = remote_ref.as_ref() else {
        eprintln!("[shortcuts] price-check: remote input not ready yet");
        return;
    };

    remote.press_ctrl_c();

    // Injected keys go out over the EIS socket fire-and-forget — press_ctrl_c
    // returning just means the events were queued/flushed, not that the
    // target app has processed Ctrl+C and updated its clipboard yet. Without
    // this, the read below races ahead and returns stale content.
    glib::timeout_future(std::time::Duration::from_millis(150)).await;

    // gdk::Clipboard only reliably reflects content while our own window has
    // had keyboard focus (Wayland gates clipboard visibility by focus) — use
    // the portal's clipboard instead, which works regardless of local focus.
    let clipboard = match remote.read_clipboard_text().await {
        Ok(Some(text)) => text,
        Ok(None) => {
            println!("[shortcuts] price-check clipboard: empty");
            return;
        }
        Err(err) => {
            eprintln!("[shortcuts] price-check clipboard read failed: {err}");
            return;
        }
    };
    println!(
        "[shortcuts] price-check clipboard read ({} chars), sending item-text at ({x}, {y})",
        clipboard.len()
    );

    // "price-check" is the target the renderer's PriceCheckWindow.vue
    // actually listens for. focusOverlay matches the real app's two default
    // hotkeys (renderer/src/web/price-check/PriceCheckWindow.vue): plain
    // Ctrl+D sends focusOverlay:false and auto-closes on mouse-move-away;
    // Ctrl+Alt+D ("hotkeyLocked") sends focusOverlay:true, which skips that
    // (real APT's own hover-to-interact replacement for the auto-close
    // path — ours is a simpler Rust-side poll, see spawn_auto_close).
    events.send_last_active(
        "MAIN->CLIENT::item-text",
        json!({
            "target": "price-check",
            "clipboard": clipboard,
            "position": { "x": x, "y": y },
            "focusOverlay": locked,
        }),
    );
    price_check_open.set(true);
    if locked {
        // The locked popup is meant to be interacted with (read, click,
        // copy) without the game stealing clicks underneath it.
        set_click_through(false, window, click_through, events);
        price_check_locked.set(true);
    } else {
        spawn_auto_close(x, y, cursor_pos.clone(), events.clone(), price_check_open.clone());
    }
}

/// Ported from main/src/windowing/WidgetAreaTracker.ts: closes the popup
/// once the cursor has moved far enough from where the item was originally
/// hovered. `cursor_pos` is updated continuously by kwin_tracker.rs's
/// persistent script (workspace.cursorPosChanged) — this just reads it, no
/// D-Bus round trip per tick. An earlier version called a one-shot KWin
/// cursor query every tick instead, which meant loading/running/unloading a
/// disposable KWin script 25x/sec while a popup was open — cheap-looking in
/// isolation, but enough sustained D-Bus + file I/O traffic to make the
/// whole compositor feel laggy, not just this popup.
fn spawn_auto_close(
    from_x: f64,
    from_y: f64,
    cursor_pos: Rc<Cell<(f64, f64)>>,
    events: Arc<EventBus>,
    price_check_open: Rc<Cell<bool>>,
) {
    // The real default (2.5 * font size, ~50px) is paired with a
    // hold-Ctrl-to-pin escape hatch we don't have; a bit more generous here
    // so small mouse drift while reading doesn't close it prematurely, but
    // not so generous it feels laggy to dismiss.
    const CLOSE_THRESHOLD: f64 = 80.0;
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(40);

    glib::spawn_future_local(async move {
        loop {
            glib::timeout_future(POLL_INTERVAL).await;

            if !price_check_open.get() {
                return; // closed some other way (e.g. pressing the hotkey again)
            }

            let (x, y) = cursor_pos.get();
            let distance = ((x - from_x).powi(2) + (y - from_y).powi(2)).sqrt();
            if distance > CLOSE_THRESHOLD {
                events.broadcast("MAIN->OVERLAY::hide-exclusive-widget", serde_json::Value::Null);
                price_check_open.set(false);
                return;
            }
        }
    });
}
