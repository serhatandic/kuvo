//! The desktop identity store.

pub use kuvo_wire::keys::*;

/// Where the app keeps this device's [`Identity`]: the system's keychain on a Mac, a file only you
/// can read in the app's own folder elsewhere, memory in tests. `kuvo_box` never writes it itself.
pub trait Keeper: Send + Sync {
    /// What's kept, as [`Identity::kept`] wrote it.
    fn read(&self) -> Kept;
    /// Keeps it; false when it couldn't be.
    fn write(&self, kept: &str) -> bool;
    /// Lets it go (this device was removed, and makes a new one on your word).
    fn forget(&self);
}

/// What a [`Keeper`] holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    Found(String),
    /// Nothing's kept: this device has no key yet.
    Nothing,
    /// It couldn't be read now (the keychain is locked, or refused): there may be a key, so none
    /// is made in its place.
    Unreadable,
}

/// The identity `keeper` holds, made and kept the first time (only when it says nothing's kept).
/// None when it can't be read or kept: a device that would be a new one at each start never
/// pairs, and one whose key can't be read now isn't replaced by another.
pub fn identity(keeper: &dyn Keeper) -> Option<Identity> {
    match keeper.read() {
        Kept::Found(text) => Identity::from_kept(&text),
        Kept::Unreadable => None,
        Kept::Nothing => {
            let made = Identity::new();
            keeper.write(&made.kept()).then_some(made)
        }
    }
}

/// A keeper in memory, for tests and for a run that keeps nothing.
#[derive(Default)]
pub struct MemoryKeeper(std::sync::Mutex<Option<String>>);

impl Keeper for MemoryKeeper {
    fn read(&self) -> Kept {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone().map_or(Kept::Nothing, Kept::Found)
    }

    fn write(&self, kept: &str) -> bool {
        *self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(kept.to_owned());
        true
    }

    fn forget(&self) {
        *self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}
