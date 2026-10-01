//! Recording and playback in the browser, which come with `MediaRecorder` in a later phase;
//! until then recordings neither start nor play, and say why.

use std::{convert::Infallible, path::Path};

/// Why the browser does not record yet.
const UNAVAILABLE: &str = "Recording isn't available in the browser yet.";

pub struct Player(Infallible);

impl Player {
    pub fn open(_: &Path, _: bool) -> Option<Self> {
        None
    }

    pub fn play(&mut self) {
        match self.0 {}
    }

    pub fn pause(&mut self) {
        match self.0 {}
    }

    pub fn playing(&self) -> bool {
        match self.0 {}
    }

    pub fn position_ms(&self) -> u32 {
        match self.0 {}
    }

    pub fn duration_ms(&self) -> u32 {
        match self.0 {}
    }

    pub fn seek(&mut self, _: u32) {
        match self.0 {}
    }
}

pub struct Recorder(Infallible);

impl Recorder {
    pub fn audio(_: &Path, _: u32) -> Result<Self, String> {
        Err(UNAVAILABLE.into())
    }

    pub fn video(_: &Path) -> Result<Self, String> {
        Err(UNAVAILABLE.into())
    }

    pub fn pause(&mut self, _: bool) {
        match self.0 {}
    }

    pub fn stop(self) -> Result<impl FnOnce() -> Result<(), String> + Send, String> {
        match self.0 {}
        #[allow(unreachable_code)]
        Ok(|| Ok(()))
    }
}
