//! Fan-in of several event sources (stub; the forwarding threads follow).

use std::time::Duration;

use super::EventSource;

pub struct MergedSource {
    _sources: Vec<Box<dyn EventSource + Send>>,
}

impl MergedSource {
    pub fn new(sources: Vec<Box<dyn EventSource + Send>>) -> Self {
        Self { _sources: sources }
    }
}

impl EventSource for MergedSource {
    fn wait(&mut self, _timeout: Duration) -> bool {
        false
    }
}
