//! Renders an [`App`] into a ratatui frame.

use ratatui::Frame;

use super::app::App;

pub fn view(app: &App, frame: &mut Frame) {
    let _ = (app, frame);
}
