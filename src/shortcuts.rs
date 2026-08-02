use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use ashpd::desktop::CreateSessionOptions;
use ashpd::WindowIdentifier;
use futures_util::StreamExt;
use gtk4::prelude::*;
use gtk4::{glib, ApplicationWindow, Button};

use crate::remote_input::RemoteInput;
use crate::toggle_click_through;

const TOGGLE_OVERLAY_ID: &str = "toggle-overlay";
const PRICE_CHECK_ID: &str = "price-check";

pub fn spawn(
    window: ApplicationWindow,
    toggle: Button,
    click_through: Rc<Cell<bool>>,
    kwin_connection: Rc<RefCell<Option<zbus::Connection>>>,
    remote_input: Rc<RefCell<Option<RemoteInput>>>,
) {
    glib::spawn_future_local(async move {
        if let Err(err) = run(
            &window,
            &toggle,
            &click_through,
            &kwin_connection,
            &remote_input,
        )
        .await
        {
            eprintln!("[shortcuts] error: {err}");
        }
    });
}

async fn run(
    window: &ApplicationWindow,
    toggle: &Button,
    click_through: &Rc<Cell<bool>>,
    kwin_connection: &Rc<RefCell<Option<zbus::Connection>>>,
    remote_input: &Rc<RefCell<Option<RemoteInput>>>,
) -> ashpd::Result<()> {
    let proxy = GlobalShortcuts::new().await?;
    let session = proxy.create_session(CreateSessionOptions::default()).await?;

    let window_id = WindowIdentifier::from_native(window).await;
    let shortcuts = [
        NewShortcut::new(TOGGLE_OVERLAY_ID, "Toggle click-through")
            .preferred_trigger(Some("SHIFT+space")),
        NewShortcut::new(PRICE_CHECK_ID, "Price-check hovered item")
            .preferred_trigger(Some("CTRL+d")),
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

    let mut activated = proxy.receive_activated().await?;
    while let Some(event) = activated.next().await {
        match event.shortcut_id() {
            TOGGLE_OVERLAY_ID => toggle_click_through(window, toggle, click_through),
            PRICE_CHECK_ID => price_check(window, kwin_connection, remote_input).await,
            _ => {}
        }
    }

    Ok(())
}

async fn price_check(
    window: &ApplicationWindow,
    kwin_connection: &Rc<RefCell<Option<zbus::Connection>>>,
    remote_input: &Rc<RefCell<Option<RemoteInput>>>,
) {
    let connection = kwin_connection.borrow().clone();
    match connection {
        Some(connection) => match crate::kwin_tracker::query_cursor_pos(&connection).await {
            Ok((x, y)) => println!("[shortcuts] price-check cursor at ({x}, {y})"),
            Err(err) => eprintln!("[shortcuts] price-check cursor query failed: {err}"),
        },
        None => eprintln!("[shortcuts] price-check: kwin connection not ready yet"),
    }

    let ctrl_c_result = {
        let remote_ref = remote_input.borrow();
        match remote_ref.as_ref() {
            Some(remote) => Some(remote.press_ctrl_c().await),
            None => None,
        }
    };
    match ctrl_c_result {
        Some(Ok(())) => {}
        Some(Err(err)) => eprintln!("[shortcuts] price-check ctrl+c injection failed: {err}"),
        None => eprintln!("[shortcuts] price-check: remote input not ready yet"),
    }

    match window.clipboard().read_text_future().await {
        Ok(Some(text)) => println!("[shortcuts] price-check clipboard:\n{text}"),
        Ok(None) => println!("[shortcuts] price-check clipboard: empty"),
        Err(err) => eprintln!("[shortcuts] price-check clipboard read failed: {err}"),
    }
}
