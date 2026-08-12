use std::path::PathBuf;

use lsp_types::InitializeParams;
use serde::Deserialize;

/// Papyrus language variant selected for a workspace.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PapyrusDialect {
    #[default]
    Auto,
    Skyrim,
    Fallout4,
    Starfield,
}

/// Editor-neutral workspace settings supplied through LSP initialization options.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceConfig {
    pub dialect: PapyrusDialect,
    pub source_roots: Vec<PathBuf>,
    pub import_directories: Vec<PathBuf>,
    #[serde(skip)]
    pub(crate) discovered_import_directories: Vec<PathBuf>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct InitializationOptions {
    papyrus: WorkspaceConfig,
}

impl WorkspaceConfig {
    pub(crate) fn from_initialize(params: &InitializeParams) -> Self {
        let mut config = params
            .initialization_options
            .clone()
            .and_then(|value| serde_json::from_value::<InitializationOptions>(value).ok())
            .map(|options| options.papyrus)
            .unwrap_or_default();

        if config.source_roots.is_empty() {
            config.source_roots.extend(
                params
                    .workspace_folders
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|folder| file_uri_to_path(folder.uri.as_str())),
            );
        }
        deduplicate(&mut config.source_roots);
        deduplicate(&mut config.import_directories);
        config
    }

    pub(crate) fn roots(&self) -> impl Iterator<Item = &PathBuf> {
        self.source_roots
            .iter()
            .chain(&self.import_directories)
            .chain(&self.discovered_import_directories)
    }

    pub(crate) fn add_discovered_import(&mut self, path: PathBuf) {
        if !self.roots().any(|existing| existing == &path) {
            self.discovered_import_directories.push(path);
        }
    }
}

fn deduplicate(paths: &mut Vec<PathBuf>) {
    let mut normalized = Vec::new();
    paths.retain(|path| {
        let candidate = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if normalized.iter().any(|seen| seen == &candidate) {
            false
        } else {
            normalized.push(candidate);
            true
        }
    });
}

pub(crate) fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let decoded = percent_decode(encoded)?;
    #[cfg(windows)]
    let decoded = decoded
        .strip_prefix('/')
        .unwrap_or(&decoded)
        .replace('/', "\\");
    Some(PathBuf::from(decoded))
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            output.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

#[cfg(test)]
mod tests {
    use lsp_types::InitializeParams;

    use super::{PapyrusDialect, WorkspaceConfig};

    #[test]
    fn parses_all_dialects_and_defaults_invalid_options() {
        for (name, expected) in [
            ("auto", PapyrusDialect::Auto),
            ("skyrim", PapyrusDialect::Skyrim),
            ("fallout4", PapyrusDialect::Fallout4),
            ("starfield", PapyrusDialect::Starfield),
        ] {
            let config: WorkspaceConfig = serde_json::from_value(serde_json::json!({
                "dialect": name
            }))
            .expect("dialect should deserialize");
            assert_eq!(config.dialect, expected);
        }
        assert!(
            serde_json::from_value::<WorkspaceConfig>(serde_json::json!({
                "dialect": "unknown"
            }))
            .is_err()
        );
    }

    #[test]
    fn invalid_options_fall_back_and_workspace_folders_supply_roots() {
        let invalid: InitializeParams = serde_json::from_value(serde_json::json!({
            "capabilities": {},
            "initializationOptions": { "papyrus": { "dialect": "unknown" } }
        }))
        .unwrap();
        assert_eq!(
            WorkspaceConfig::from_initialize(&invalid),
            WorkspaceConfig::default()
        );

        let workspace: InitializeParams = serde_json::from_value(serde_json::json!({
            "capabilities": {},
            "workspaceFolders": [{ "uri": "file:///workspace/My%20Mod", "name": "My Mod" }]
        }))
        .unwrap();
        let config = WorkspaceConfig::from_initialize(&workspace);
        assert_eq!(config.source_roots.len(), 1);
        assert!(config.source_roots[0].to_string_lossy().contains("My Mod"));
    }
}
