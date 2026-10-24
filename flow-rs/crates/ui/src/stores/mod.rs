//! The window's shared state, ported from the stateful half of
//! `src/renderer/settings/lib/`: the config store (optimistic writes), the
//! catalog store, app info, microphones and the theme. The app owns one of
//! each, pumps them once per frame, and hands them to pages through
//! [`crate::pages::Cx`].

pub mod catalog;
pub mod config;
pub mod hooks;
pub mod theme;

pub use catalog::CatalogStore;
pub use config::ConfigStore;
pub use hooks::{AppInfoStore, Debounced, Flash, Microphones};
pub use theme::{Appearance, SystemTheme, appearance};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use flow_core::config::{ConfigSnapshot, validate_config};
    use flow_core::path;
    use flow_core::protocol::settings::{Event, Reply, Request};
    use parking_lot::Mutex;
    use serde_json::json;

    use super::*;
    use crate::backend::{Backend, Link};

    type Waiting = Vec<(Request, Box<dyn FnOnce(Reply) + Send>)>;

    /// A backend whose replies the test sends by hand.
    #[derive(Default)]
    struct Manual {
        waiting: Mutex<Waiting>,
    }
    impl Backend for Manual {
        fn request(&self, request: Request, reply: Box<dyn FnOnce(Reply) + Send>) {
            self.waiting.lock().push((request, reply));
        }
        fn subscribe(&self, _on_event: Box<dyn Fn(Event) + Send + Sync>) {}
    }
    impl Manual {
        fn answer(&self, index: usize, reply: Reply) -> Request {
            let (request, respond) = self.waiting.lock().remove(index);
            respond(reply);
            request
        }
        fn count(&self) -> usize {
            self.waiting.lock().len()
        }
    }

    fn snapshot(raw: serde_json::Value) -> ConfigSnapshot {
        ConfigSnapshot {
            config: validate_config(&raw).config,
            issues: vec![],
            path: "config.toml".into(),
        }
    }

    fn store() -> (Arc<Manual>, ConfigStore) {
        let backend = Arc::new(Manual::default());
        let link = Link::new(backend.clone(), egui::Context::default());
        let mut store = ConfigStore::new(link);
        assert!(!store.loaded());
        backend.answer(0, Reply::ok(&snapshot(json!({}))));
        store.pump(&[]);
        assert!(store.loaded());
        (backend, store)
    }

    #[test]
    fn writes_are_optimistic_and_only_the_newest_reply_applies() {
        let (backend, mut store) = store();
        store.set(path!["audio", "sounds"], false);
        store.set(path!["audio", "keep_mic_warm"], true);
        assert!(
            !store.config().audio.sounds,
            "the optimistic state shows at once"
        );
        assert!(store.config().audio.keep_mic_warm);
        assert_eq!(store.pending(), 2);

        // A change from elsewhere is ignored while writes are pending.
        store.pump(&[Event::ConfigChanged(snapshot(
            json!({ "audio": { "max_recording_seconds": 60 } }),
        ))]);
        assert_eq!(store.config().audio.max_recording_seconds, 300);

        // The reply to the older write does not overwrite the newer optimistic state.
        backend.answer(
            0,
            Reply::ok(&snapshot(json!({ "audio": { "sounds": false } }))),
        );
        store.pump(&[]);
        assert!(store.config().audio.keep_mic_warm);

        backend.answer(
            0,
            Reply::ok(&snapshot(
                json!({ "audio": { "sounds": false, "keep_mic_warm": true } }),
            )),
        );
        store.pump(&[]);
        assert_eq!(store.pending(), 0);
        assert!(store.config().audio.keep_mic_warm && !store.config().audio.sounds);

        store.pump(&[Event::ConfigChanged(snapshot(json!({})))]);
        assert!(
            store.config().audio.sounds,
            "changes from elsewhere apply when idle"
        );
    }

    #[test]
    fn a_failed_write_shows_the_error_and_reloads() {
        let (backend, mut store) = store();
        let mut done = store.patch(vec![flow_core::config::ConfigPatch::set(
            path!["audio", "sounds"],
            false,
        )]);
        backend.answer(0, Reply::err("disk full"));
        store.pump(&[]);
        assert_eq!(store.error(), Some("The change was not saved: disk full"));
        assert_eq!(done.take(), Some(Err("disk full".into())));
        // The store re-reads the file.
        assert_eq!(backend.count(), 1);
        assert_eq!(
            backend.answer(0, Reply::ok(&snapshot(json!({})))),
            Request::ConfigGet
        );
        store.pump(&[]);
        assert!(store.config().audio.sounds);
        store.dismiss_error();
        assert_eq!(store.error(), None);
    }
}
