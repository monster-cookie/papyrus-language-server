use std::{collections::HashSet, fs, path::PathBuf};

use crate::indexing::{IndexingControl, IndexingLimits};

use super::{DiskIndexResult, WorkspaceIndex};

struct ScanState {
    visited: HashSet<PathBuf>,
    entries: usize,
    files: usize,
    bytes: u64,
    stopped: bool,
}

impl WorkspaceIndex {
    pub(super) fn scan(&mut self, control: Option<&IndexingControl>) {
        let limits = control.map(|control| control.limits).unwrap_or_default();
        let mut state = ScanState {
            visited: HashSet::new(),
            entries: 0,
            files: 0,
            bytes: 0,
            stopped: false,
        };
        let roots = self.config.roots().cloned().collect::<Vec<_>>();
        for root in roots {
            self.scan_path(&root, 0, limits, control, &mut state);
            if state.stopped || control.is_some_and(IndexingControl::is_cancelled) {
                break;
            }
        }
        if let Some(control) = control {
            let message = state.stopped.then(|| {
                format!(
                    "Index limit reached after {} files and {} bytes",
                    state.files, state.bytes
                )
            });
            control.report("indexing", state.files, state.bytes, message);
        }
    }

    fn scan_path(
        &mut self,
        path: &std::path::Path,
        depth: usize,
        limits: IndexingLimits,
        control: Option<&IndexingControl>,
        state: &mut ScanState,
    ) {
        if state.stopped || control.is_some_and(IndexingControl::is_cancelled) {
            return;
        }
        if depth > limits.max_depth {
            return;
        }
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if !state.visited.insert(canonical) {
            return;
        }
        if path.is_file() {
            self.scan_file(path, limits, control, state);
            return;
        }
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            state.entries += 1;
            if state.entries > limits.max_entries {
                state.stopped = true;
                break;
            }
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() && !file_type.is_symlink() {
                self.scan_path(&path, depth + 1, limits, control, state);
            } else if file_type.is_file() && !file_type.is_symlink() {
                self.scan_file(&path, limits, control, state);
            }
            if state.stopped || control.is_some_and(IndexingControl::is_cancelled) {
                break;
            }
        }
    }

    fn scan_file(
        &mut self,
        path: &std::path::Path,
        limits: IndexingLimits,
        control: Option<&IndexingControl>,
        state: &mut ScanState,
    ) {
        if state.files >= limits.max_files || state.bytes >= limits.max_total_bytes {
            state.stopped = true;
            return;
        }
        let remaining = limits.max_total_bytes - state.bytes;
        let max_bytes = limits.max_file_bytes.min(remaining);
        match self.index_disk_file_bounded(path, max_bytes) {
            DiskIndexResult::Ignored => {}
            DiskIndexResult::TooLarge => {}
            DiskIndexResult::Indexed(bytes) => {
                state.files += 1;
                state.bytes += bytes;
                if state.files % 250 == 0
                    && let Some(control) = control
                {
                    control.report("indexing", state.files, state.bytes, None);
                }
            }
        }
    }
}
