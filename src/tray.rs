//! System tray icon (freedesktop StatusNotifierItem, via `ksni`), replacing
//! real APT's `AppTray.ts`. GTK4 dropped `GtkStatusIcon`, so this is the
//! only widget-free way to quit/reopen the app. "Settings/League" (real
//! APT's message-box item pointing at the in-game overlay-key hint) is
//! dropped: there's no equivalent in-overlay Settings-access hint flow here
//! for it to point at.

use gtk4::gdk_pixbuf::Pixbuf;
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, Tray, TrayMethods};

pub enum TrayAction {
    OpenBrowser,
    OpenConfigFolder,
    Quit,
}

struct AppTray {
    action_tx: async_channel::Sender<TrayAction>,
}

impl Tray for AppTray {
    fn id(&self) -> String {
        crate::APP_ID.to_string()
    }

    fn title(&self) -> String {
        "apt-kwin-overlay".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: format!("apt-kwin-overlay v{}", env!("CARGO_PKG_VERSION")),
            ..Default::default()
        }
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        load_icon().into_iter().collect()
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open in Browser".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.action_tx.try_send(TrayAction::OpenBrowser);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open config folder".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.action_tx.try_send(TrayAction::OpenConfigFolder);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.action_tx.try_send(TrayAction::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Spawns a dedicated thread that registers the tray's D-Bus service, then
/// exits — mirrors `kwin_tracker::spawn`'s shape, but the tray itself
/// doesn't need this thread to stay alive: `Tray::spawn().await` hands its
/// background D-Bus/menu handling off to `ksni`'s own globally-shared
/// executor thread (see `ksni::compat`'s `async-io` backend), which keeps
/// running independently for the process's lifetime.
pub fn spawn() -> async_channel::Receiver<TrayAction> {
    let (action_tx, action_rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        if let Err(err) = async_io::block_on(async { AppTray { action_tx }.spawn().await.map(drop) }) {
            eprintln!("[tray] error: {err}");
        }
    });
    action_rx
}

fn load_icon() -> Option<Icon> {
    let path = crate::server::dist_dir().join("icon.png");
    let pixbuf = match Pixbuf::from_file(&path) {
        Ok(pixbuf) => pixbuf,
        Err(err) => {
            eprintln!("[tray] failed to load icon {path:?}: {err}");
            return None;
        }
    };
    let width = pixbuf.width();
    let height = pixbuf.height();
    let rowstride = pixbuf.rowstride();
    let n_channels = pixbuf.n_channels();
    // SAFETY: only read from the returned slice; the Pixbuf outlives it.
    let pixels = unsafe { pixbuf.pixels() };
    Some(Icon {
        width,
        height,
        data: rgba_to_argb(pixels, width, height, rowstride, n_channels),
    })
}

/// Converts gdk-pixbuf's row-major RGB(A) buffer (rows may be padded to
/// `rowstride`, wider than `width * n_channels`) into the tightly-packed
/// ARGB32 network-byte-order format `ksni::Icon::data` expects.
fn rgba_to_argb(pixels: &[u8], width: i32, height: i32, rowstride: i32, n_channels: i32) -> Vec<u8> {
    let (width, height, rowstride, n_channels) =
        (width as usize, height as usize, rowstride as usize, n_channels as usize);
    let mut data = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let row = &pixels[y * rowstride..y * rowstride + width * n_channels];
        for pixel in row.chunks_exact(n_channels) {
            let alpha = if n_channels == 4 { pixel[3] } else { 255 };
            data.extend_from_slice(&[alpha, pixel[0], pixel[1], pixel[2]]);
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::rgba_to_argb;

    #[test]
    fn converts_rgba_rows_respecting_padding_to_tightly_packed_argb() {
        // 2x1 RGBA image with 4 bytes of row padding, to exercise both the
        // channel reordering and the rowstride-skip logic in one case.
        let pixels = [
            10, 20, 30, 40, // pixel 0: R, G, B, A
            50, 60, 70, 80, // pixel 1: R, G, B, A
            0, 0, 0, 0, // padding
        ];
        let argb = rgba_to_argb(&pixels, 2, 1, 12, 4);
        assert_eq!(argb, vec![40, 10, 20, 30, 80, 50, 60, 70]);
    }

    #[test]
    fn defaults_missing_alpha_channel_to_opaque() {
        let pixels = [10, 20, 30]; // single RGB pixel, no alpha channel
        let argb = rgba_to_argb(&pixels, 1, 1, 3, 3);
        assert_eq!(argb, vec![255, 10, 20, 30]);
    }
}
