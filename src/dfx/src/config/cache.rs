use crate::config::dfx_version;
use crate::lib::error::DfxError::CacheError;
use crate::lib::error::{CacheErrorKind, DfxError, DfxResult};
use crate::util;
use indicatif::{ProgressBar, ProgressDrawTarget};
use semver::Version;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::ExitStatus;

// POSIX permissions for files in the cache.
const EXEC_READ_USER_ONLY_PERMISSION: u32 = 0o500;

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
pub trait Cache {
    fn version_str(&self) -> String;
    fn is_installed(&self) -> DfxResult<bool>;
    fn install(&self) -> DfxResult;
    fn force_install(&self) -> DfxResult;
    fn delete(&self) -> DfxResult;
    fn get_binary_command_path(&self, binary_name: &str) -> DfxResult<PathBuf>;
    fn get_binary_command(&self, binary_name: &str) -> DfxResult<std::process::Command>;
}

pub struct DiskBasedCache {
    version: Version,
}

impl DiskBasedCache {
    pub fn with_version(version: &Version) -> DiskBasedCache {
        DiskBasedCache {
            version: version.clone(),
        }
    }
}

#[allow(dead_code)]
impl Cache for DiskBasedCache {
    fn version_str(&self) -> String {
        format!("{}", self.version)
    }

    fn is_installed(&self) -> DfxResult<bool> {
        is_version_installed(&self.version_str())
    }

    fn install(&self) -> DfxResult {
        install_version(&self.version_str(), false).map(|_| {})
    }
    fn force_install(&self) -> DfxResult {
        install_version(&self.version_str(), true).map(|_| {})
    }

    fn delete(&self) -> DfxResult {
        delete_version(&self.version_str()).map(|_| {})
    }

    fn get_binary_command_path(&self, binary_name: &str) -> DfxResult<PathBuf> {
        get_binary_path_from_version(&self.version_str(), binary_name)
    }

    fn get_binary_command(&self, binary_name: &str) -> DfxResult<std::process::Command> {
        binary_command_from_version(&self.version_str(), binary_name)
    }
}

/// Provides a profile for the user.
pub fn get_profile_path() -> DfxResult<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| CacheError(CacheErrorKind::CannotFindUserHomeDirectory()))?;

    let p = PathBuf::from(home)
        .join(".dfinity-identities")
        .join("profile");

    if !p.exists() {
        if let Err(e) = std::fs::create_dir_all(&p) {
            return Err(CacheError(CacheErrorKind::CannotCreateCacheDirectory(p, e)));
        }
    } else if !p.is_dir() {
        return Err(CacheError(CacheErrorKind::CacheShouldBeADirectory(p)));
    }

    Ok(p)
}

/// Return the binary cache root. It constructs it if not present
/// already.
pub fn get_bin_cache_root() -> DfxResult<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| CacheError(CacheErrorKind::CannotFindUserHomeDirectory()))?;

    let p = PathBuf::from(home)
        .join(".cache")
        .join("dfinity")
        .join("versions");

    if !p.exists() {
        if let Err(e) = std::fs::create_dir_all(&p) {
            return Err(CacheError(CacheErrorKind::CannotCreateCacheDirectory(p, e)));
        }
    } else if !p.is_dir() {
        return Err(CacheError(CacheErrorKind::CacheShouldBeADirectory(p)));
    }

    Ok(p)
}

pub fn get_bin_cache(v: &str) -> DfxResult<PathBuf> {
    let root = get_bin_cache_root()?;
    Ok(root.join(v))
}

pub fn is_version_installed(v: &str) -> DfxResult<bool> {
    get_bin_cache(v).and_then(|c| Ok(c.is_dir()))
}

pub fn delete_version(v: &str) -> DfxResult<bool> {
    if !is_version_installed(v).unwrap_or(false) {
        return Ok(false);
    }

    let root = get_bin_cache(v)?;
    std::fs::remove_dir_all(&root)?;

    Ok(true)
}

pub fn install_version(v: &str, force: bool) -> DfxResult<PathBuf> {
    let p = get_bin_cache(v)?;
    if !force && is_version_installed(v).unwrap_or(false) {
        return Ok(p);
    }

    if Version::parse(v)? == *dfx_version() {
        // Dismiss as fast as possible. We use the current_exe variable after an
        // expensive step, and if this fails we can't continue anyway.
        let current_exe = std::env::current_exe()?;

        let b: Option<ProgressBar> = if atty::is(atty::Stream::Stderr) {
            let b = ProgressBar::new_spinner();
            b.set_draw_target(ProgressDrawTarget::stderr());
            b.set_message(&format!("Installing version {} of dfx...", v));
            b.enable_steady_tick(80);
            Some(b)
        } else {
            None
        };

        let mut binary_cache_assets = util::assets::binary_cache()?;
        // Write binaries and set them to be executable.
        for file in binary_cache_assets.entries()? {
            let mut file = file?;

            if file.header().entry_type().is_dir() {
                continue;
            }
            file.unpack_in(p.as_path())?;

            let full_path = p.join(file.path()?);
            let mut perms = std::fs::metadata(full_path.as_path())?.permissions();
            perms.set_mode(EXEC_READ_USER_ONLY_PERMISSION);
            std::fs::set_permissions(full_path.as_path(), perms)?;
        }

        // Copy our own binary in the cache.
        let dfx = p.join("dfx");
        std::fs::write(&dfx, std::fs::read(current_exe)?)?;
        // And make it executable.
        let mut perms = std::fs::metadata(&dfx)?.permissions();
        perms.set_mode(EXEC_READ_USER_ONLY_PERMISSION);
        std::fs::set_permissions(&dfx, perms)?;

        if let Some(b) = b {
            b.finish_with_message(&format!("Version v{} installed successfully.", v));
        }

        Ok(p)
    } else {
        Err(CacheError(CacheErrorKind::UnknownDfxVersion(v.to_owned())))
    }
}

pub fn get_binary_path_from_version(version: &str, binary_name: &str) -> DfxResult<PathBuf> {
    install_version(version, false)?;

    Ok(get_bin_cache(version)?.join(binary_name))
}

pub fn binary_command_from_version(version: &str, name: &str) -> DfxResult<std::process::Command> {
    let path = get_binary_path_from_version(version, name)?;
    let cmd = std::process::Command::new(path);

    Ok(cmd)
}

pub fn list_versions() -> DfxResult<Vec<Version>> {
    let root = get_bin_cache_root()?;
    let mut result: Vec<Version> = Vec::new();

    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if let Some(version) = entry.file_name().to_str() {
            result.push(Version::parse(version)?);
        }
    }

    Ok(result)
}

pub fn call_cached_dfx(v: &Version) -> DfxResult<ExitStatus> {
    let v = format!("{}", v);
    let command_path = get_binary_path_from_version(&v, "dfx")?;
    if command_path == std::env::current_exe()? {
        return Err(DfxError::Unknown(
            format_args!("Invalid cache for version {}.", v).to_string(),
        ));
    }

    std::process::Command::new(command_path)
        .args(std::env::args().skip(1))
        .status()
        .map_err(DfxError::from)
}
