//! Skills (`kuvo/skills*`, `kuvo/skill-trash*`, `kuvo/skill-library*`).

pub mod library;
pub mod names;
pub mod store;

use std::{path::Path, sync::Arc};

use kuvo_server::Registry;

pub const FEATURES: &[&str] = &["skills", "skills.trash"];

pub struct ResidentSkills {
    pub store: Arc<store::SkillStore>,
    pub library: Arc<library::SkillLibrary>,
    sweeper: tokio::task::JoinHandle<()>,
}

impl ResidentSkills {
    pub fn start(routes: &mut Registry, home: &Path, account_home: &Path) -> Self {
        let store = Arc::new(store::SkillStore::for_home(home, account_home));
        let library = Arc::new(library::SkillLibrary::for_store(&store));
        let sweeper = store.start_trash_sweeper();
        store::register(routes, store.clone());
        library::register(routes, store.clone(), library.clone());
        Self { store, library, sweeper }
    }

    pub fn stop(self) {
        self.sweeper.abort();
    }
}
