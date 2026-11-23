use kuvo_os::platform::{Account, Environment, Platform, box_home};
use std::{
    io,
    path::{Path, PathBuf},
};

/// Locations inside the frozen machine layout. The move owns the retired
/// app-folder paths and environment overrides used to find its sources.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Home(PathBuf);

impl Home {
    pub fn new(root: PathBuf) -> io::Result<Self> {
        if !root.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Kuvo home must be absolute",
            ));
        }
        Ok(Self(root))
    }

    pub fn resolve(account: &Account, platform: Platform, env: &Environment) -> io::Result<Self> {
        Self::new(box_home(account, platform, env)?)
    }

    pub fn root(&self) -> &Path {
        &self.0
    }
    pub fn run(&self) -> PathBuf {
        self.0.join("run")
    }
    pub fn settings(&self) -> PathBuf {
        self.0.join("box")
    }
    pub fn bin(&self) -> PathBuf {
        self.0.join("bin")
    }
    pub fn engine_data(&self) -> PathBuf {
        self.0.join("data/opencode")
    }
    pub fn bots(&self) -> PathBuf {
        self.0.join("bots")
    }
    pub fn wakes(&self) -> PathBuf {
        self.0.join("wakes")
    }
    pub fn groups(&self) -> PathBuf {
        self.0.join("groups")
    }
    pub fn chats(&self) -> PathBuf {
        self.0.join("chats")
    }
    pub fn attachments(&self) -> PathBuf {
        self.0.join("attachments")
    }
    pub fn search(&self) -> PathBuf {
        self.0.join("search")
    }
    pub fn updates(&self) -> PathBuf {
        self.0.join("updates")
    }
    pub fn integrations(&self) -> PathBuf {
        self.0.join("integrations")
    }
}
