use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::mpsc;

pub enum Destination {
    Logo(PathBuf),
    Game {
        systems: Vec<crate::systems::FoundSystem>,
        system: String,
        launch: crate::browse::Launch,
        title: String,
        excluded: std::collections::HashSet<String>,
    },
}
pub enum Saved {
    Logo(PathBuf),
    Game(Vec<String>),
}

pub struct Job {
    result: mpsc::Receiver<Result<Saved, String>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Job {
    pub fn start(destination: Destination, body: Vec<u8>) -> Result<Self, String> {
        let (sender, result) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("web-artwork".into())
            .spawn(move || {
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match destination {
                        Destination::Game {
                            systems,
                            system,
                            launch,
                            title,
                            excluded,
                        } => crate::scraper::install_uploaded_image(
                            &systems, &system, &launch, &title, &body, &excluded,
                        )
                        .map(Saved::Game)
                        .map_err(|error| error.to_string()),
                        Destination::Logo(directory) => (|| {
                            let extension = crate::scraper::validate_uploaded_image(&body)
                                .map_err(|error| error.to_string())?;
                            let digest: String = Sha256::digest(&body)
                                .iter()
                                .map(|byte| format!("{byte:02x}"))
                                .collect();
                            let path = directory.join(format!("web-{digest}.{extension}"));
                            std::fs::create_dir_all(&directory).map_err(|error| {
                                format!("Creating logos folder failed: {error}")
                            })?;
                            crate::cache::write(&path, &body).map_err(|error| error.to_string())?;
                            Ok(Saved::Logo(path))
                        })(),
                    }))
                    .unwrap_or_else(|_| Err("Artwork import worker stopped unexpectedly.".into()));
                let _ = sender.send(outcome);
            })
            .map_err(|error| format!("Starting artwork import failed: {error}"))?;
        Ok(Self {
            result,
            thread: Some(thread),
        })
    }
    pub fn poll(&self) -> Option<Result<Saved, String>> {
        match self.result.try_recv() {
            Ok(value) => Some(value),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Artwork import worker disconnected.".into()))
            }
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
