use std::sync::Mutex;

// Not actually "remote" (matching the real app's RemoteLogger.ts naming) —
// just an in-memory buffer new WS clients get replayed on connect.
pub struct Logger {
    history: Mutex<String>,
}

impl Logger {
    pub fn new() -> Self {
        Self {
            history: Mutex::new(String::new()),
        }
    }

    pub fn history(&self) -> String {
        self.history.lock().unwrap().clone()
    }

    #[allow(dead_code)]
    pub fn write(&self, message: &str) {
        let mut history = self.history.lock().unwrap();
        history.push_str(message);
        history.push('\n');
    }
}
