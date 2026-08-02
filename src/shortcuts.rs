use std::cell::Cell;
use std::rc::Rc;

use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use ashpd::desktop::CreateSessionOptions;
use ashpd::WindowIdentifier;
use futures_util::StreamExt;
use gtk4::{glib, ApplicationWindow, Button};

use crate::toggle_click_through;

const SHORTCUT_ID: &str = "toggle-overlay";

pub fn spawn(window: ApplicationWindow, toggle: Button, click_through: Rc<Cell<bool>>) {
    glib::spawn_future_local(async move {
        if let Err(err) = run(&window, &toggle, &click_through).await {
            eprintln!("[shortcuts] error: {err}");
        }
    });
}

async fn run(
    window: &ApplicationWindow,
    toggle: &Button,
    click_through: &Rc<Cell<bool>>,
) -> ashpd::Result<()> {
    let proxy = GlobalShortcuts::new().await?;
    let session = proxy.create_session(CreateSessionOptions::default()).await?;

    let window_id = WindowIdentifier::from_native(window).await;
    let shortcuts = [NewShortcut::new(SHORTCUT_ID, "Toggle click-through")
        .preferred_trigger(Some("SHIFT+space"))];

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
        if event.shortcut_id() == SHORTCUT_ID {
            toggle_click_through(window, toggle, click_through);
        }
    }

    Ok(())
}
