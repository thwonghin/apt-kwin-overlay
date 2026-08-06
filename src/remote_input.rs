use std::cell::RefCell;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::Arc;

use ashpd::desktop::clipboard::{Clipboard, RequestClipboardOptions};
use ashpd::desktop::remote_desktop::{ConnectToEISOptions, DeviceType, RemoteDesktop};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use ashpd::enumflags2::BitFlags as AshpdBitFlags;
use futures_util::{AsyncReadExt, StreamExt};
use reis::enumflags2::BitFlags as EiBitFlags;
use reis::event::{DeviceCapability, EiEvent};
use reis::{ei, event};

use crate::logger::Logger;

// Raw Linux evdev keycodes (linux/input-event-codes.h). notify_keyboard_keycode
// (the plain RemoteDesktop D-Bus path) turned out to have zero real-world
// effect on this KDE version despite the session being genuinely granted —
// confirmed via xdg-desktop-portal-kde debug logs showing the calls arriving
// correctly. NotifyKeyboardKeycode goes through KDE's own `fake_input`
// Wayland protocol; ConnectToEIS instead hands us a raw libei connection that
// KWin's actual remote-input backend is built around, which is the path KDE
// Connect itself uses. Keycodes stay evdev numbering either way.
const KEY_LEFTCTRL: u32 = 29;
const KEY_C: u32 = 46;

fn restore_token_path() -> std::path::PathBuf {
    crate::xdg::data_dir().join("apt-kwin-overlay/remote_desktop_restore_token")
}

struct KeyboardHandle {
    device: ei::Device,
    keyboard: ei::Keyboard,
}

pub struct RemoteInput {
    context: ei::Context,
    connection: event::Connection,
    keyboard: Rc<RefCell<Option<KeyboardHandle>>>,
    // Reference point for frame() timestamps (microseconds, monotonic) — real
    // values, not a hardcoded 0, since the EIS server likely uses them for
    // event ordering and may misorder/drop identically-timestamped events.
    start: std::time::Instant,
    // Portal-based clipboard access (org.freedesktop.portal.Clipboard), tied
    // to this RemoteDesktop session. Reading via GTK4's own gdk::Clipboard
    // turned out to only work reliably while our own window had keyboard
    // focus — Wayland gates clipboard visibility by focus for privacy, and
    // our overlay is meant to never hold focus while the game does. This
    // portal exists specifically so a RemoteDesktop-style session can read
    // the clipboard regardless of local focus.
    clipboard: Clipboard,
    // Kept alive, not otherwise used post-setup: dropping the D-Bus portal
    // session could tear down the EIS grant it authorized, even though the
    // raw EIS socket (`context`) is a separate connection.
    #[allow(dead_code)]
    session: Session<RemoteDesktop>,
    logger: Arc<Logger>,
}

impl RemoteInput {
    pub async fn setup(logger: Arc<Logger>) -> ashpd::Result<Self> {
        let proxy = RemoteDesktop::new().await?;
        let session = proxy.create_session(CreateSessionOptions::default()).await?;

        let saved_token = std::fs::read_to_string(restore_token_path()).ok();

        proxy
            .select_devices(
                &session,
                ashpd::desktop::remote_desktop::SelectDevicesOptions::default()
                    .set_devices(AshpdBitFlags::from(DeviceType::Keyboard))
                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                    .set_restore_token(saved_token.as_deref()),
            )
            .await?
            .response()?;

        // Must be requested before the session starts (portal spec).
        let clipboard = Clipboard::new().await?;
        clipboard
            .request(&session, RequestClipboardOptions::default())
            .await?;

        let response = proxy
            .start(&session, None, Default::default())
            .await?
            .response()?;

        if let Some(token) = response.restore_token() {
            let path = restore_token_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, token);
        }

        let fd = proxy
            .connect_to_eis(&session, ConnectToEISOptions::default())
            .await?;
        let stream = UnixStream::from(fd);
        let context =
            ei::Context::new(stream).map_err(|err| ashpd::Error::IO(std::io::Error::other(err)))?;

        let (connection, mut events) = context
            .handshake_async_io("apt-kwin-overlay", ei::handshake::ContextType::Sender)
            .await
            .map_err(|err| ashpd::Error::IO(std::io::Error::other(err.to_string())))?;

        let keyboard = Rc::new(RefCell::new(None));
        {
            let keyboard = keyboard.clone();
            let event_loop_context = context.clone();
            gtk4::glib::spawn_future_local(async move {
                while let Some(result) = events.next().await {
                    let event = match result {
                        Ok(event) => event,
                        Err(err) => {
                            eprintln!("[remote_input] eis event stream error: {err}");
                            continue;
                        }
                    };
                    println!("[remote_input] eis event: {event:?}");
                    match event {
                        EiEvent::SeatAdded(added) => {
                            added
                                .seat
                                .bind_capabilities(EiBitFlags::from(DeviceCapability::Keyboard));
                            let _ = event_loop_context.flush();
                        }
                        EiEvent::DeviceAdded(added) => {
                            if let Some(kbd) = added.device.interface::<ei::Keyboard>() {
                                println!("[remote_input] keyboard device bound");
                                keyboard.replace(Some(KeyboardHandle {
                                    device: added.device.device().clone(),
                                    keyboard: kbd,
                                }));
                            } else {
                                println!("[remote_input] device added without keyboard interface");
                            }
                        }
                        EiEvent::DeviceResumed(resumed) => {
                            resumed.device.device().start_emulating(0, 0);
                            let _ = event_loop_context.flush();
                        }
                        _ => {}
                    }
                }
                println!("[remote_input] eis event stream ended");
            });
        }

        Ok(Self {
            context,
            connection,
            keyboard,
            start: std::time::Instant::now(),
            clipboard,
            session,
            logger,
        })
    }

    pub async fn read_clipboard_text(&self) -> ashpd::Result<Option<String>> {
        let fd = self
            .clipboard
            .selection_read(&self.session, "text/plain;charset=utf-8")
            .await?;
        let file = std::fs::File::from(std::os::fd::OwnedFd::from(fd));
        let mut async_file =
            async_io::Async::new(file).map_err(|err| ashpd::Error::IO(std::io::Error::other(err)))?;
        let mut bytes = Vec::new();
        async_file
            .read_to_end(&mut bytes)
            .await
            .map_err(|err| ashpd::Error::IO(std::io::Error::other(err)))?;
        if bytes.is_empty() {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    pub fn press_ctrl_c(&self) {
        self.press_keys(&[
            (KEY_LEFTCTRL, ei::keyboard::KeyState::Press),
            (KEY_C, ei::keyboard::KeyState::Press),
            (KEY_C, ei::keyboard::KeyState::Released),
            (KEY_LEFTCTRL, ei::keyboard::KeyState::Released),
        ]);
    }

    fn press_keys(&self, keys: &[(u32, ei::keyboard::KeyState)]) {
        let handle = self.keyboard.borrow();
        let Some(handle) = handle.as_ref() else {
            let msg = "[remote_input] keyboard device not ready yet";
            eprintln!("{msg}");
            self.logger.write(msg);
            return;
        };

        for &(keycode, state) in keys {
            handle.keyboard.key(keycode, state);
            let timestamp = self.start.elapsed().as_micros() as u64;
            handle.device.frame(self.connection.serial(), timestamp);
        }
        let _ = self.context.flush();
    }
}
