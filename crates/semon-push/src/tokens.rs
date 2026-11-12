//! Receiver directory checks and the name of refused legacy token custody.
//! Authentication uses the enrollment registry; legacy token state is never read,
//! imported, rewritten or deleted.
use std::{fs, io, path::Path};
pub const TOKENS_FILE: &str = "tokens";

/// An existing receiver directory must be private; missing directories may be
/// created. Existing permissions are never changed implicitly.
pub fn prepare_private_dir(dir: &Path, create: bool) -> Result<(), String> {
    let failed = |error: io::Error| format!("{}: {error}", dir.display());
    match fs::metadata(dir) {
        Ok(meta) if !meta.is_dir() => Err(format!("{} is not a directory", dir.display())),
        Ok(meta) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o077 != 0 {
                    return Err(format!("{} must be private mode 0700", dir.display()));
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
            if let Some(parent) = dir.parent().filter(|parent| !parent.as_os_str().is_empty()) {
                fs::create_dir_all(parent).map_err(failed)?;
            }
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(dir).map_err(failed)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(failed(error)),
    }
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn loose_existing_directory_is_refused_without_changing_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("semon-private-dir-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(super::prepare_private_dir(&path, true).is_err());
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        std::fs::remove_dir(path).unwrap();
    }
}
