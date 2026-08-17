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
    #[allow(deprecated)]
    pub(crate) fn from_initialize(params: &InitializeParams) -> Self {
        let mut config = params
            .initialization_options
            .clone()
            .and_then(|value| serde_json::from_value::<InitializationOptions>(value).ok())
            .map(|options| options.papyrus)
            .unwrap_or_default();

        if config.source_roots.is_empty() {
            let workspace_roots = params
                .workspace_folders
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter_map(|folder| file_uri_to_path(folder.uri.as_str()))
                .collect::<Vec<_>>();
            if workspace_roots.is_empty() {
                config.source_roots.extend(
                    params
                        .root_uri
                        .as_ref()
                        .and_then(|uri| file_uri_to_path(uri.as_str())),
                );
            } else {
                config.source_roots.extend(workspace_roots);
            }
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
    let (scheme, remainder) = uri.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("file") || remainder.contains(['?', '#']) {
        return None;
    }
    let (authority, encoded_path) = if let Some(without_prefix) = remainder.strip_prefix("//") {
        without_prefix
            .split_once('/')
            .map(|(authority, path)| (authority, format!("/{path}")))
            .unwrap_or((without_prefix, String::new()))
    } else {
        ("", remainder.to_owned())
    };
    let authority = percent_decode(authority)?;
    let authority = (!authority.eq_ignore_ascii_case("localhost"))
        .then_some(authority)
        .filter(|authority| !authority.is_empty());
    let decoded = percent_decode(&encoded_path)?;
    #[cfg(windows)]
    {
        if let Some(authority) = authority {
            let path = decoded.trim_start_matches('/').replace('/', "\\");
            return Some(PathBuf::from(format!(r"\\{authority}\{path}")));
        }
        let mut path = decoded.replace('/', "\\");
        if path.as_bytes().first() == Some(&b'\\') && path.as_bytes().get(2) == Some(&b':') {
            path.remove(0);
        }
        std::path::Path::new(&path)
            .is_absolute()
            .then_some(PathBuf::from(path))
    }
    #[cfg(not(windows))]
    {
        if authority.is_some() || !decoded.starts_with('/') {
            return None;
        }
        Some(PathBuf::from(decoded))
    }
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

    use super::{PapyrusDialect, WorkspaceConfig, file_uri_to_path};

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

        #[cfg(windows)]
        let workspace_uri = "file:///C:/workspace/My%20Mod";
        #[cfg(not(windows))]
        let workspace_uri = "file:///workspace/My%20Mod";
        let workspace: InitializeParams = serde_json::from_value(serde_json::json!({
            "capabilities": {},
            "workspaceFolders": [{ "uri": workspace_uri, "name": "My Mod" }]
        }))
        .unwrap();
        let config = WorkspaceConfig::from_initialize(&workspace);
        assert_eq!(config.source_roots.len(), 1);
        assert!(config.source_roots[0].to_string_lossy().contains("My Mod"));

        let legacy: InitializeParams = serde_json::from_value(serde_json::json!({
            "capabilities": {},
            "rootUri": workspace_uri,
            "workspaceFolders": [{ "uri": "https://example.com/ignored", "name": "Ignored" }]
        }))
        .unwrap();
        let config = WorkspaceConfig::from_initialize(&legacy);
        assert_eq!(config.source_roots.len(), 1);
        assert!(config.source_roots[0].to_string_lossy().contains("My Mod"));
    }

    #[test]
    fn rejects_non_file_relative_and_decorated_uris() {
        assert!(file_uri_to_path("https://example.com/Script.psc").is_none());
        assert!(file_uri_to_path("file:relative/Script.psc").is_none());
        assert!(file_uri_to_path("file:///Script.psc?query").is_none());
        assert!(file_uri_to_path("file:///Script.psc#fragment").is_none());
        assert!(file_uri_to_path("file:///Bad%2").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn converts_local_and_unc_windows_file_uris() {
        assert_eq!(
            file_uri_to_path("file:///C:/Projects/My%20Mod/Script.psc").unwrap(),
            std::path::PathBuf::from(r"C:\Projects\My Mod\Script.psc")
        );
        assert_eq!(
            file_uri_to_path("file://server/share/Script.psc").unwrap(),
            std::path::PathBuf::from(r"\\server\share\Script.psc")
        );
        assert_eq!(
            file_uri_to_path("file://localhost/C:/Projects/Script.psc").unwrap(),
            std::path::PathBuf::from(r"C:\Projects\Script.psc")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn accepts_local_and_rejects_remote_unix_file_uris() {
        assert_eq!(
            file_uri_to_path("file:///workspace/My%20Mod/Script.psc").unwrap(),
            std::path::PathBuf::from("/workspace/My Mod/Script.psc")
        );
        assert!(file_uri_to_path("file://server/share/Script.psc").is_none());
    }
}
