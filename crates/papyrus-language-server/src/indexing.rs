use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
};

use crate::{
    cache::materialize_starfield_sources_with_cancel,
    config::{PapyrusDialect, WorkspaceConfig},
    discovery::discover_starfield_sources,
    workspace::WorkspaceIndex,
};

/// Hard limits applied while walking configured workspace and import roots.
#[derive(Clone, Copy)]
pub(crate) struct IndexingLimits {
    pub(crate) max_entries: usize,
    pub(crate) max_files: usize,
    pub(crate) max_file_bytes: u64,
    pub(crate) max_total_bytes: u64,
    pub(crate) max_depth: usize,
}

impl Default for IndexingLimits {
    fn default() -> Self {
        Self {
            max_entries: 1_000_000,
            max_files: 250_000,
            max_file_bytes: 16 * 1024 * 1024,
            max_total_bytes: 4 * 1024 * 1024 * 1024,
            max_depth: 128,
        }
    }
}

/// A periodic snapshot of bounded workspace indexing work.
#[derive(Clone)]
pub(crate) struct IndexingProgress {
    pub(crate) phase: &'static str,
    pub(crate) files: usize,
    pub(crate) bytes: u64,
    pub(crate) message: Option<String>,
}

/// Messages sent from the indexing worker to the foreground LSP loop.
pub(crate) enum IndexingEvent {
    Progress(IndexingProgress),
    Completed(Box<Result<WorkspaceIndex, String>>),
}

/// Cooperative cancellation and progress reporting shared with the scanner.
pub(crate) struct IndexingControl {
    cancelled: Arc<AtomicBool>,
    sender: Sender<IndexingEvent>,
    pub(crate) limits: IndexingLimits,
}

impl IndexingControl {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub(crate) fn cancellation_flag(&self) -> &AtomicBool {
        &self.cancelled
    }

    pub(crate) fn report(
        &self,
        phase: &'static str,
        files: usize,
        bytes: u64,
        message: Option<String>,
    ) {
        let _ = self.sender.send(IndexingEvent::Progress(IndexingProgress {
            phase,
            files,
            bytes,
            message,
        }));
    }
}

/// Handle used by the foreground server to observe and cancel background indexing.
pub(crate) struct IndexingTask {
    receiver: Receiver<IndexingEvent>,
    cancelled: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl IndexingTask {
    pub(crate) fn start(config: WorkspaceConfig) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker = thread::Builder::new()
            .name("papyrus-workspace-index".to_owned())
            .spawn(move || {
                let control = IndexingControl {
                    cancelled: worker_cancelled,
                    sender: sender.clone(),
                    limits: IndexingLimits::default(),
                };
                let result = prepare_config(config, &control)
                    .and_then(|config| WorkspaceIndex::new_with_control(&config, &control));
                let _ = sender.send(IndexingEvent::Completed(Box::new(result)));
            })
            .map_err(|error| format!("failed to start workspace indexing worker: {error}"))?;
        Ok(Self {
            receiver,
            cancelled,
            worker: Some(worker),
        })
    }

    pub(crate) fn try_recv(&self) -> Result<IndexingEvent, TryRecvError> {
        self.receiver.try_recv()
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for IndexingTask {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn prepare_config(
    mut config: WorkspaceConfig,
    control: &IndexingControl,
) -> Result<WorkspaceConfig, String> {
    control.report(
        "discovery",
        0,
        0,
        Some("Discovering source roots".to_owned()),
    );
    if control.is_cancelled() {
        return Err("workspace indexing cancelled".to_owned());
    }
    if config.dialect != PapyrusDialect::Starfield {
        return Ok(config);
    }
    let Some(sources) = discover_starfield_sources() else {
        eprintln!("papyrus-language-server: Starfield Creation Kit sources not found");
        return Ok(config);
    };
    if let Some(source_directory) = sources.source_directory {
        eprintln!(
            "papyrus-language-server: using SFCK sources {}",
            source_directory.display()
        );
        config.add_discovered_import(source_directory);
    } else if let Some(archive) = sources.archive {
        control.report(
            "extraction",
            0,
            0,
            Some(format!("Preparing {}", archive.display())),
        );
        match materialize_starfield_sources_with_cancel(&archive, control.cancellation_flag()) {
            Ok(cache) => {
                eprintln!(
                    "papyrus-language-server: SFCK cache {} (indexed {}, excluded {})",
                    cache.root.display(),
                    cache.indexed,
                    cache.excluded
                );
                config.add_discovered_import(cache.root);
            }
            Err(error) => eprintln!(
                "papyrus-language-server: failed to materialize {}: {error}",
                archive.display()
            ),
        }
    }
    if control.is_cancelled() {
        Err("workspace indexing cancelled".to_owned())
    } else {
        Ok(config)
    }
}
