use serde::Deserialize;
use zbus::interface;

const PLUGIN_NAME: &str = "apt-wayland-overlay-tracker";
const CALLBACK_IFACE: &str = "dev.spike.apt_wayland_overlay.Callback";
const CALLBACK_PATH: &str = "/";

#[derive(Debug, Deserialize)]
pub struct WindowEvent {
    // Not read yet — kept for later matching a specific tracked window
    // (e.g. the real PoE window) across repeated geometry events.
    #[allow(dead_code)]
    pub id: String,
    #[serde(rename = "className")]
    pub class_name: String,
    pub caption: String,
    pub pid: i64,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug)]
pub enum TrackerEvent {
    Activated(WindowEvent),
    GeometryChanged(WindowEvent),
}

struct Callback {
    sender: async_channel::Sender<TrackerEvent>,
}

#[interface(name = "dev.spike.apt_wayland_overlay.Callback")]
impl Callback {
    async fn activated(&self, json: String) {
        match serde_json::from_str::<WindowEvent>(&json) {
            Ok(event) => {
                let _ = self.sender.send(TrackerEvent::Activated(event)).await;
            }
            Err(err) => eprintln!("[kwin_tracker] bad activated payload: {err}"),
        }
    }

    #[allow(non_snake_case)]
    async fn geometryChanged(&self, json: String) {
        match serde_json::from_str::<WindowEvent>(&json) {
            Ok(event) => {
                let _ = self.sender.send(TrackerEvent::GeometryChanged(event)).await;
            }
            Err(err) => eprintln!("[kwin_tracker] bad geometryChanged payload: {err}"),
        }
    }
}

fn tracker_script(dbus_addr: &str) -> String {
    format!(
        r#"
function emit(kind, w) {{
    callDBus("{dbus_addr}", "{path}", "{iface}", kind, JSON.stringify({{
        id: w.internalId.toString(),
        className: w.resourceClass,
        caption: w.caption,
        pid: w.pid,
        x: w.frameGeometry.x,
        y: w.frameGeometry.y,
        width: w.frameGeometry.width,
        height: w.frameGeometry.height
    }}));
}}
function hook(w) {{
    w.frameGeometryChanged.connect(function () {{ emit("geometryChanged", w); }});
}}
workspace.windowList().forEach(hook);
workspace.windowAdded.connect(hook);
workspace.windowActivated.connect(function (w) {{ if (w) emit("activated", w); }});
if (workspace.activeWindow) emit("activated", workspace.activeWindow);
"#,
        dbus_addr = dbus_addr,
        path = CALLBACK_PATH,
        iface = CALLBACK_IFACE,
    )
}

/// Spawns a dedicated thread running its own zbus connection: registers our
/// callback interface, loads a persistent KWin script that hooks window
/// activation/geometry signals, and forwards every event to the GTK main
/// loop via `sender`. The KWin script and this connection are meant to live
/// for the whole process lifetime (never `stop()`/`unloadScript`'d).
pub fn spawn(sender: async_channel::Sender<TrackerEvent>) {
    std::thread::spawn(move || {
        if let Err(err) = async_io::block_on(run(sender)) {
            eprintln!("[kwin_tracker] error: {err}");
        }
    });
}

async fn run(sender: async_channel::Sender<TrackerEvent>) -> zbus::Result<()> {
    let connection = zbus::connection::Builder::session()?
        .serve_at(CALLBACK_PATH, Callback { sender })?
        .build()
        .await?;

    let dbus_addr = connection.unique_name().expect("connection has a unique name").to_string();

    let script_path = std::env::temp_dir().join(format!("{PLUGIN_NAME}.js"));
    std::fs::write(&script_path, tracker_script(&dbus_addr))?;

    let scripting = zbus::Proxy::new(
        &connection,
        "org.kde.KWin",
        "/Scripting",
        "org.kde.kwin.Scripting",
    )
    .await?;

    let already_loaded: bool = scripting.call("isScriptLoaded", &(PLUGIN_NAME,)).await?;
    if already_loaded {
        let _: bool = scripting.call("unloadScript", &(PLUGIN_NAME,)).await?;
    }

    let script_id: i32 = scripting
        .call(
            "loadScript",
            &(script_path.to_string_lossy().to_string(), PLUGIN_NAME),
        )
        .await?;

    let script = zbus::Proxy::new(
        &connection,
        "org.kde.KWin",
        format!("/Scripting/Script{script_id}"),
        "org.kde.kwin.Script",
    )
    .await?;
    script.call_method("run", &()).await?;

    println!("[kwin_tracker] persistent script loaded (id {script_id}), watching for events");

    // Keep this thread (and the connection/object-server it owns) alive for
    // the process lifetime.
    std::future::pending::<()>().await;
    Ok(())
}
