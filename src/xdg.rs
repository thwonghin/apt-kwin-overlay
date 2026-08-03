//! Minimal XDG Base Directory resolution — only what this app needs
//! (the data dir; nothing here currently wants config/cache separately).

pub fn data_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        return std::path::PathBuf::from(dir);
    }
    let home = std::env::var("HOME").expect("HOME must be set");
    std::path::PathBuf::from(home).join(".local/share")
}
