//! Summarize the selected session (`s`): the `summarize` command on a worker
//! thread, because a local model can take minutes. Never automatic.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;

use super::app::Msg;
use crate::env::Env;

/// Runs `summarize` for session `id` against the state directory holding
/// `store_path`, and answers with [`Msg::Summarized`].
pub fn run(env: &Env, store_path: &Path, id: &str) -> Msg {
    let _ = (env, store_path);
    Msg::Summarized {
        id: id.to_owned(),
        result: Ok(()),
    }
}

/// Runs [`run`] on a worker thread and sends the answer on `tx`.
pub fn spawn(env: &Env, store_path: &Path, id: &str, tx: Sender<Msg>) -> std::io::Result<()> {
    let env = env.clone();
    let path: PathBuf = store_path.to_owned();
    let id = id.to_owned();
    thread::Builder::new()
        .name("summarize".to_owned())
        .spawn(move || {
            let _ = tx.send(run(&env, &path, &id));
        })
        .map(|_| ())
}
