use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use ashpd::enumflags2::BitFlags;

// Raw Linux evdev keycodes (linux/input-event-codes.h). Use these, not
// notify_keyboard_keysym: xdg-desktop-portal-kde runs headless and resolves
// keysyms against the wrong keyboard layout (KDE bug 489021). Keycodes skip
// that resolution entirely and go straight to the compositor.
const KEY_LEFTCTRL: i32 = 29;
const KEY_C: i32 = 46;

fn restore_token_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    std::path::PathBuf::from(home)
        .join(".local/share/apt-wayland-overlay/remote_desktop_restore_token")
}

pub struct RemoteInput {
    proxy: RemoteDesktop,
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
                    .set_devices(BitFlags::from(DeviceType::Keyboard))
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

        Ok(Self { proxy, session })
    }

    pub async fn press_ctrl_c(&self) -> ashpd::Result<()> {
        for (keycode, state) in [
            (KEY_LEFTCTRL, KeyState::Pressed),
            (KEY_C, KeyState::Pressed),
            (KEY_C, KeyState::Released),
            (KEY_LEFTCTRL, KeyState::Released),
        ] {
            self.proxy
                .notify_keyboard_keycode(&self.session, keycode, state, Default::default())
                .await?;
        }
        Ok(())
    }
}
