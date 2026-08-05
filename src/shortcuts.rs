use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use ashpd::WindowIdentifier;
use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::Session;
use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use gtk4::{ApplicationWindow, glib};
use serde_json::json;

use crate::host_config::{
    ActionKind, ShortcutAction, default_actions, extra_registerable_actions, shortcut_id,
};
use crate::kwin_tracker::WindowEvent;
use crate::remote_input::RemoteInput;
use crate::server::EventBus;
use crate::{set_click_through, toggle_click_through};

pub fn spawn(
    window: ApplicationWindow,
    click_through: Rc<Cell<bool>>,
    kwin_connection: Rc<RefCell<Option<zbus::Connection>>>,
    remote_input: Rc<RefCell<Option<RemoteInput>>>,
    events: Arc<EventBus>,
    active_window: Rc<RefCell<Option<WindowEvent>>>,
    focus_game_rx: async_channel::Receiver<()>,
    shortcut_config_rx: async_channel::Receiver<Vec<ShortcutAction>>,
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
                close_all_ui(
                    &window,
                    &click_through,
                    &events,
                    &price_check_open,
                    &price_check_locked,
                );
            }
        });
    }

    glib::spawn_future_local(async move {
        if let Err(err) = run(
            &window,
            &click_through,
            &kwin_connection,
            &remote_input,
            &events,
            &price_check_open,
            &price_check_locked,
            &active_window,
            shortcut_config_rx,
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
        events.broadcast(
            "MAIN->OVERLAY::hide-exclusive-widget",
            serde_json::Value::Null,
        );
        price_check_open.set(false);
    }
    price_check_locked.set(false);
    set_click_through(true, window, click_through, events);
}

/// Binds any actions not already bound this session and refreshes the
/// dispatch map from `actions`. Confirmed empirically against a live KDE
/// session (checking ~/.config/kglobalshortcutsrc after two separate
/// bind_shortcuts calls): the portal's own doc calls BindShortcuts additive,
/// but KDE's kglobalaccel backend actually replaces this app-id's *entire*
/// persisted grant set with whatever's supplied in the *latest* call — a
/// second call with only the newly-added shortcuts silently dropped every
/// previously-bound one. So `all_shortcuts` accumulates every NewShortcut
/// we've ever wanted bound (keyed by id, never shrinks) and the *full*
/// accumulated list is resupplied on every call that has anything new,
/// never just the delta. `dispatch` is wholesale-replaced each call from
/// the current config only, so actions dropped from it simply have no
/// entry left to look up (their old id, if pressed, is a harmless no-op).
/// Since `host_config::shortcut_id` is now trigger-independent (stable per
/// action, not per hotkey — see its doc comment), a "dropped" id here means
/// the action itself disappeared from config (e.g. a widget got disabled),
/// not that its hotkey merely changed — changing the hotkey no longer
/// touches `all_shortcuts`/`dispatch` at all, since the id doesn't change.
async fn apply_shortcuts(
    proxy: &GlobalShortcuts,
    session: &Session<GlobalShortcuts>,
    window_id: Option<&WindowIdentifier>,
    all_shortcuts: &Rc<RefCell<HashMap<String, NewShortcut>>>,
    dispatch: &Rc<RefCell<HashMap<String, ShortcutAction>>>,
    actions: Vec<ShortcutAction>,
) -> ashpd::Result<()> {
    let mut added_new = false;
    let mut fresh_dispatch = HashMap::new();

    for action in actions {
        let Some(id) = shortcut_id(&action) else {
            continue;
        };
        if !all_shortcuts.borrow().contains_key(&id) {
            let description = match &action.action {
                // Matches real APT's own action type name ('toggle-overlay')
                // and the row's own label in hotkeys.vue ("Overlay") rather
                // than leaking our internal click-through implementation
                // detail into KDE's user-facing Shortcuts list.
                ActionKind::ToggleOverlay => "Toggle overlay".to_string(),
                // Locked/unlocked price-check share the same target
                // ("price-check") — without the focus_overlay suffix both
                // would show up in KDE's own Shortcuts list under the
                // identical name "Copy item (price-check)", indistinguishable
                // from each other there.
                ActionKind::CopyItem {
                    target,
                    focus_overlay: true,
                } => format!("Copy item ({target}, locked)"),
                ActionKind::CopyItem {
                    target,
                    focus_overlay: false,
                } => format!("Copy item ({target})"),
                ActionKind::Unsupported => unreachable!(),
            };
            // Empty shortcut (extra_registerable_actions' unset-by-default
            // entries) -> empty trigger -> register with no preferred
            // trigger at all, so it still shows up in KDE's own Global
            // Shortcuts settings as a nameable, bindable-but-currently-unset
            // entry rather than silently not existing there.
            let trigger = crate::host_config::to_portal_trigger(&action.shortcut);
            let preferred_trigger = if trigger.is_empty() { None } else { Some(trigger.as_str()) };
            let new_shortcut = NewShortcut::new(id.clone(), description).preferred_trigger(preferred_trigger);
            all_shortcuts.borrow_mut().insert(id.clone(), new_shortcut);
            added_new = true;
        }
        fresh_dispatch.insert(id, action);
    }

    if added_new {
        let full_list: Vec<NewShortcut> = all_shortcuts.borrow().values().cloned().collect();
        let request = proxy
            .bind_shortcuts(
                session,
                &full_list,
                window_id,
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
    }

    *dispatch.borrow_mut() = fresh_dispatch;
    Ok(())
}

async fn run(
    window: &ApplicationWindow,
    click_through: &Rc<Cell<bool>>,
    kwin_connection: &Rc<RefCell<Option<zbus::Connection>>>,
    remote_input: &Rc<RefCell<Option<RemoteInput>>>,
    events: &Arc<EventBus>,
    price_check_open: &Rc<Cell<bool>>,
    price_check_locked: &Rc<Cell<bool>>,
    active_window: &Rc<RefCell<Option<WindowEvent>>>,
    shortcut_config_rx: async_channel::Receiver<Vec<ShortcutAction>>,
) -> ashpd::Result<()> {
    let proxy = GlobalShortcuts::new().await?;
    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await?;
    let window_id = WindowIdentifier::from_native(window).await;

    let all_shortcuts: Rc<RefCell<HashMap<String, NewShortcut>>> = Rc::new(RefCell::new(HashMap::new()));
    let dispatch: Rc<RefCell<HashMap<String, ShortcutAction>>> =
        Rc::new(RefCell::new(HashMap::new()));

    // Registers every copy-item target the original renderer UI could ever
    // configure, not just the 3 with a sensible default trigger — otherwise,
    // since our Hotkeys/Item Info tabs no longer expose a way to set them,
    // there'd be no way to ever discover or enable them via KDE's own
    // Global Shortcuts settings either.
    let mut initial_actions = default_actions();
    initial_actions.extend(extra_registerable_actions());
    apply_shortcuts(
        &proxy,
        &session,
        window_id.as_ref(),
        &all_shortcuts,
        &dispatch,
        initial_actions,
    )
    .await?;

    // Debounce: the portal has been observed emitting multiple Activated
    // signals for what looked like a single physical keypress (several
    // rapid modifier transitions logged for one press during testing) —
    // without this, that means duplicate Ctrl+C injections and duplicate
    // proxy/network requests per press.
    const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);
    let mut last_activation: Option<std::time::Instant> = None;

    let mut activated = proxy.receive_activated().await?;

    // Live-updates the same session's bindings whenever the renderer sends
    // a new CLIENT->MAIN::update-host-config — moved into its own task since
    // it needs to hold proxy/session for the rest of the app's lifetime,
    // same as the activation loop below.
    {
        let all_shortcuts = all_shortcuts.clone();
        let dispatch = dispatch.clone();
        glib::spawn_future_local(async move {
            while let Ok(actions) = shortcut_config_rx.recv().await {
                if let Err(err) = apply_shortcuts(
                    &proxy,
                    &session,
                    window_id.as_ref(),
                    &all_shortcuts,
                    &dispatch,
                    actions,
                )
                .await
                {
                    eprintln!("[shortcuts] failed to apply updated shortcuts: {err}");
                }
            }
        });
    }

    while let Some(event) = activated.next().await {
        let now = std::time::Instant::now();
        if last_activation.is_some_and(|t| now.duration_since(t) < DEBOUNCE) {
            continue;
        }
        last_activation = Some(now);

        let Some(action) = dispatch.borrow().get(event.shortcut_id()).cloned() else {
            continue;
        };
        match action.action {
            ActionKind::ToggleOverlay => {
                println!(
                    "[shortcuts] toggle-overlay activated, click_through was {}",
                    click_through.get()
                );
                toggle_click_through(window, click_through, events)
            }
            ActionKind::CopyItem {
                target,
                focus_overlay,
            } => {
                price_check(
                    window,
                    click_through,
                    kwin_connection,
                    remote_input,
                    events,
                    price_check_open,
                    price_check_locked,
                    active_window,
                    &target,
                    focus_overlay,
                )
                .await
            }
            ActionKind::Unsupported => {}
        }
    }

    Ok(())
}

/// Handles any `copy-item` action: presses Ctrl+C, reads the clipboard, and
/// broadcasts item-text with whatever `target` the bound action specifies
/// (real APT's own copy-item targets aren't just "price-check" — open-wiki,
/// open-craft-of-exile, open-poedb, search-similar, and item-check all go
/// through this same flow, per renderer/src/web/Config.ts's
/// getConfigForHost — the renderer decides what UI to show per target).
async fn price_check(
    window: &ApplicationWindow,
    click_through: &Rc<Cell<bool>>,
    kwin_connection: &Rc<RefCell<Option<zbus::Connection>>>,
    remote_input: &Rc<RefCell<Option<RemoteInput>>>,
    events: &Arc<EventBus>,
    price_check_open: &Rc<Cell<bool>>,
    price_check_locked: &Rc<Cell<bool>>,
    active_window: &Rc<RefCell<Option<WindowEvent>>>,
    target: &str,
    focus_overlay: bool,
) {
    println!(
        "[shortcuts] price_check called, target={target}, focus_overlay={focus_overlay}, price_check_open={}",
        price_check_open.get()
    );
    // Pressing the same hotkey again while the popup's open closes it --
    // hide-exclusive-widget targets exactly this widget (unlike
    // focus-change, which affects every hide-on-blur/hide-on-focus widget).
    if price_check_open.get() {
        events.broadcast(
            "MAIN->OVERLAY::hide-exclusive-widget",
            serde_json::Value::Null,
        );
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

    let cursor = {
        let connection = kwin_connection.borrow().clone();
        match connection {
            Some(connection) => match crate::kwin_tracker::query_cursor_pos(&connection).await {
                Ok(pos) => Some(pos),
                Err(err) => {
                    eprintln!("[shortcuts] price-check cursor query failed: {err}");
                    None
                }
            },
            None => {
                eprintln!("[shortcuts] price-check: kwin connection not ready yet");
                None
            }
        }
    };
    let (x, y) = cursor.unwrap_or((0.0, 0.0));

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

    // `target` is whatever the bound action's config specifies (e.g. the
    // renderer's PriceCheckWindow.vue listens for target:"price-check").
    // focusOverlay matches the real app's own hotkey semantics (renderer/
    // src/web/price-check/PriceCheckWindow.vue): plain Ctrl+D sends
    // focusOverlay:false and auto-closes on mouse-move-away; Ctrl+Alt+D
    // ("hotkeyLocked") sends focusOverlay:true, which skips that (real
    // APT's own hover-to-interact replacement for the auto-close path —
    // ours is a simpler Rust-side poll, see spawn_auto_close).
    events.send_last_active(
        "MAIN->CLIENT::item-text",
        json!({
            "target": target,
            "clipboard": clipboard,
            "position": { "x": x, "y": y },
            "focusOverlay": focus_overlay,
        }),
    );
    price_check_open.set(true);
    if focus_overlay {
        // The locked popup is meant to be interacted with (read, click,
        // copy) without the game stealing clicks underneath it.
        set_click_through(false, window, click_through, events);
        price_check_locked.set(true);
    } else {
        spawn_auto_close(
            x,
            y,
            kwin_connection.clone(),
            events.clone(),
            price_check_open.clone(),
        );
    }
}

/// Ported from main/src/windowing/WidgetAreaTracker.ts: closes the popup
/// once the cursor has moved far enough from where the item was originally
/// hovered. Polls via the same one-shot KWin cursor query used to capture
/// `from` in the first place, rather than the real implementation's
/// continuous uiohook mousemove stream (which we don't have on Wayland).
fn spawn_auto_close(
    from_x: f64,
    from_y: f64,
    kwin_connection: Rc<RefCell<Option<zbus::Connection>>>,
    events: Arc<EventBus>,
    price_check_open: Rc<Cell<bool>>,
) {
    // The real default (2.5 * font size, ~50px) is paired with a
    // hold-Ctrl-to-pin escape hatch we don't have; a bit more generous here
    // so small mouse drift while reading doesn't close it prematurely, but
    // not so generous it feels laggy to dismiss.
    //
    // POLL_INTERVAL was tried at 40ms for a snappier close, but each tick
    // costs a full KWin loadScript/run/unloadScript D-Bus round trip plus
    // temp-file I/O (query_cursor_pos is a one-shot mechanism, not meant for
    // hot-loop polling) -- 25 of those a second was enough to make the whole
    // compositor feel laggy, not just this popup. A push-based replacement
    // (workspace.cursorPosChanged) was tried and reverted: the signal exists
    // but doesn't actually fire on real mouse movement on this KWin version,
    // so cursor tracking silently stopped updating and the popup never
    // auto-closed at all. Back to 80ms, the original value, which never had
    // either problem.
    const CLOSE_THRESHOLD: f64 = 80.0;
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(80);

    glib::spawn_future_local(async move {
        loop {
            glib::timeout_future(POLL_INTERVAL).await;

            if !price_check_open.get() {
                return; // closed some other way (e.g. pressing the hotkey again)
            }

            let connection = kwin_connection.borrow().clone();
            let Some(connection) = connection else {
                continue;
            };
            let Ok((x, y)) = crate::kwin_tracker::query_cursor_pos(&connection).await else {
                continue;
            };

            let distance = ((x - from_x).powi(2) + (y - from_y).powi(2)).sqrt();
            if distance > CLOSE_THRESHOLD {
                events.broadcast(
                    "MAIN->OVERLAY::hide-exclusive-widget",
                    serde_json::Value::Null,
                );
                price_check_open.set(false);
                return;
            }
        }
    });
}
