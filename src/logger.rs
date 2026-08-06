use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::server::EventBus;

// Not actually "remote" (matching the real app's RemoteLogger.ts naming) —
// just an in-memory buffer new WS clients get replayed on connect, plus a
// live MAIN->CLIENT::log-entry broadcast to whoever's already connected.
pub struct Logger {
    history: Mutex<String>,
    events: Arc<EventBus>,
}

impl Logger {
    pub fn new(events: Arc<EventBus>) -> Self {
        Self {
            history: Mutex::new(String::new()),
            events,
        }
    }

    pub fn history(&self) -> String {
        self.history.lock().unwrap().clone()
    }

    pub fn write(&self, message: &str) {
        {
            let mut history = self.history.lock().unwrap();
            history.push_str(message);
            history.push('\n');
        }
        self.events
            .broadcast("MAIN->CLIENT::log-entry", json!({ "message": message }));
    }
}
