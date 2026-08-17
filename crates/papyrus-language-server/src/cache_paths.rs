use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

/// Returns the private per-user cache directory used by the language server.
///
/// The function deliberately has no shared-temporary-directory fallback. Callers
/// should disable persistent caching when this directory cannot be established.
pub(crate) fn cache_directory() -> io::Result<PathBuf> {
    let base = platform_cache_base().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "a private per-user cache directory is unavailable",
        )
    })?;
    let directory = base.join("papyrus-language-server").join("cache");
    create_private_directory(&directory)?;
    Ok(directory)
}

fn platform_cache_base() -> Option<PathBuf> {
    platform_cache_base_from(&CacheEnvironment::current())
}

#[cfg(windows)]
fn platform_cache_base_from(environment: &CacheEnvironment) -> Option<PathBuf> {
    environment
        .local_app_data
        .as_ref()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

#[cfg(target_os = "macos")]
fn platform_cache_base_from(environment: &CacheEnvironment) -> Option<PathBuf> {
    environment
        .home
        .as_ref()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|home| home.join("Library").join("Caches"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_cache_base_from(environment: &CacheEnvironment) -> Option<PathBuf> {
    environment
        .xdg_cache_home
        .as_ref()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            environment
                .home
                .as_ref()
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".cache"))
        })
}

#[cfg(not(any(windows, unix)))]
fn platform_cache_base_from(_environment: &CacheEnvironment) -> Option<PathBuf> {
    None
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("cache path is not a regular directory: {}", path.display()),
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }

    #[cfg(not(unix))]
    fs::create_dir_all(path)?;

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("cache path is not a regular directory: {}", path.display()),
        ));
    }
    Ok(())
}

#[derive(Default)]
struct CacheEnvironment {
    #[cfg(windows)]
    local_app_data: Option<OsString>,
    #[cfg(all(unix, not(target_os = "macos")))]
    xdg_cache_home: Option<OsString>,
    #[cfg(unix)]
    home: Option<OsString>,
}

impl CacheEnvironment {
    fn current() -> Self {
        Self {
            #[cfg(windows)]
            local_app_data: std::env::var_os("LOCALAPPDATA"),
            #[cfg(all(unix, not(target_os = "macos")))]
            xdg_cache_home: std::env::var_os("XDG_CACHE_HOME"),
            #[cfg(unix)]
            home: std::env::var_os("HOME"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::PathBuf};

    use super::{CacheEnvironment, platform_cache_base_from};

    #[test]
    fn selects_the_platform_private_cache_base_without_a_temp_fallback() {
        let mut environment = CacheEnvironment::default();

        #[cfg(windows)]
        {
            assert_eq!(platform_cache_base_from(&environment), None);
            environment.local_app_data = Some(OsString::from("relative/cache"));
            assert_eq!(platform_cache_base_from(&environment), None);
            environment.local_app_data = Some(OsString::from(r"C:\Users\Example\AppData\Local"));
            assert_eq!(
                platform_cache_base_from(&environment),
                Some(PathBuf::from(r"C:\Users\Example\AppData\Local"))
            );
        }

        #[cfg(target_os = "macos")]
        {
            assert_eq!(platform_cache_base_from(&environment), None);
            environment.home = Some(OsString::from("relative/cache"));
            assert_eq!(platform_cache_base_from(&environment), None);
            environment.home = Some(OsString::from("/Users/example"));
            assert_eq!(
                platform_cache_base_from(&environment),
                Some(PathBuf::from("/Users/example/Library/Caches"))
            );
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            assert_eq!(platform_cache_base_from(&environment), None);
            environment.xdg_cache_home = Some(OsString::from("relative/cache"));
            environment.home = Some(OsString::from("relative/home"));
            assert_eq!(platform_cache_base_from(&environment), None);
            environment.home = Some(OsString::from("/home/example"));
            assert_eq!(
                platform_cache_base_from(&environment),
                Some(PathBuf::from("/home/example/.cache"))
            );
            environment.xdg_cache_home = Some(OsString::from("/var/cache/example"));
            assert_eq!(
                platform_cache_base_from(&environment),
                Some(PathBuf::from("/var/cache/example"))
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn creates_private_directories_and_rejects_a_symlink_destination() {
        use std::{
            fs,
            os::unix::fs::{PermissionsExt, symlink},
            time::{SystemTime, UNIX_EPOCH},
        };

        let root = std::env::temp_dir().join(format!(
            "papyrus-cache-path-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let private = root.join("private").join("cache");
        super::create_private_directory(&private).unwrap();
        assert_eq!(
            fs::metadata(&private).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        let link = root.join("link");
        symlink(&target, &link).unwrap();
        assert!(super::create_private_directory(&link).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
