use std::sync::Mutex;

// The renderer treats this as an opaque string blob (its whole serialized
// settings state) — we don't parse it, just persist and echo it back,
// matching main/src/host-files/ConfigStore.ts's wire contract. The on-disk
// path doesn't need to match the real app's, since the renderer only ever
// talks to it through GET /config and the save-config/config-changed events.
pub struct ConfigStore {
    path: Mutex<std::path::PathBuf>,
}

fn config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    std::path::PathBuf::from(home).join(".local/share/apt-wayland-overlay/config.json")
}

impl ConfigStore {
    pub fn new() -> Self {
        Self {
            path: Mutex::new(config_path()),
        }
    }

    pub fn load(&self) -> Option<String> {
        std::fs::read_to_string(&*self.path.lock().unwrap()).ok()
    }

    pub fn save(&self, contents: &str, is_temporary: bool) {
        let mut path = self.path.lock().unwrap();
        if is_temporary && path.extension().and_then(|e| e.to_str()) != Some("tmp") {
            let mut with_tmp = path.clone().into_os_string();
            with_tmp.push(".tmp");
            *path = with_tmp.into();
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(err) = std::fs::write(&*path, contents) {
            eprintln!("[config_store] failed to save config: {err}");
        }
    }
}
