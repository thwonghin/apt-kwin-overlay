use std::cell::RefCell;
use std::os::unix::net::UnixStream;
use std::rc::Rc;

use ashpd::desktop::remote_desktop::{ConnectToEISOptions, DeviceType, RemoteDesktop};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use ashpd::enumflags2::BitFlags as AshpdBitFlags;
use futures_util::StreamExt;
use reis::enumflags2::BitFlags as EiBitFlags;
use reis::event::{DeviceCapability, EiEvent};
use reis::{ei, event};

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
    let home = std::env::var("HOME").expect("HOME must be set");
    std::path::PathBuf::from(home)
        .join(".local/share/apt-wayland-overlay/remote_desktop_restore_token")
}

struct KeyboardHandle {
    device: ei::Device,
    keyboard: ei::Keyboard,
}

pub struct RemoteInput {
    context: ei::Context,
    connection: event::Connection,
    keyboard: Rc<RefCell<Option<KeyboardHandle>>>,
    // Kept alive, not otherwise used post-setup: dropping the D-Bus portal
    // session could tear down the EIS grant it authorized, even though the
    // raw EIS socket (`context`) is a separate connection.
    #[allow(dead_code)]
    session: Session<RemoteDesktop>,
}

impl RemoteInput {
    pub async fn setup() -> ashpd::Result<Self> {
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
            .handshake_async_io("apt-wayland-overlay", ei::handshake::ContextType::Sender)
            .await
            .map_err(|err| ashpd::Error::IO(std::io::Error::other(err.to_string())))?;

        let keyboard = Rc::new(RefCell::new(None));
        {
            let keyboard = keyboard.clone();
            gtk4::glib::spawn_future_local(async move {
                while let Some(result) = events.next().await {
                    let Ok(event) = result else { continue };
                    match event {
                        EiEvent::SeatAdded(added) => {
                            added
                                .seat
                                .bind_capabilities(EiBitFlags::from(DeviceCapability::Keyboard));
                        }
                        EiEvent::DeviceAdded(added) => {
                            if let Some(kbd) = added.device.interface::<ei::Keyboard>() {
                                keyboard.replace(Some(KeyboardHandle {
                                    device: added.device.device().clone(),
                                    keyboard: kbd,
                                }));
                            }
                        }
                        EiEvent::DeviceResumed(resumed) => {
                            resumed.device.device().start_emulating(0, 0);
                        }
                        _ => {}
                    }
                }
            });
        }

        Ok(Self {
            context,
            connection,
            keyboard,
            session,
        })
    }

    pub fn press_ctrl_c(&self) {
        let handle = self.keyboard.borrow();
        let Some(handle) = handle.as_ref() else {
            eprintln!("[remote_input] keyboard device not ready yet");
            return;
        };

        for (keycode, state) in [
            (KEY_LEFTCTRL, ei::keyboard::KeyState::Press),
            (KEY_C, ei::keyboard::KeyState::Press),
            (KEY_C, ei::keyboard::KeyState::Released),
            (KEY_LEFTCTRL, ei::keyboard::KeyState::Released),
        ] {
            handle.keyboard.key(keycode, state);
            handle.device.frame(self.connection.serial(), 0);
        }
        let _ = self.context.flush();
    }
}
