use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const GITHUB_OWNER: &str = "Pirs26";
pub const GITHUB_REPO: &str = "voxel";
pub const RELEASES_API_URL: &str = "https://api.github.com/repos/Pirs26/voxel/releases?per_page=15";
pub const CHECKSUMS_ASSET_NAME: &str = "SHA256SUMS.txt";
pub const APP_VERSION: &str = env!("RESOLVED_APP_VERSION");

const STREAM_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)) {
            Ordering::Equal => match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                // Stable release > pre-release (e.g. 0.2.0 > 0.2.0-rc.1)
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => compare_prerelease(a, b),
            },
            ord => ord,
        }
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn compare_prerelease(a: &str, b: &str) -> Ordering {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();

    for (id_a, id_b) in a_parts.iter().zip(b_parts.iter()) {
        if id_a == id_b {
            continue;
        }
        let a_num = parse_digits_u64(id_a);
        let b_num = parse_digits_u64(id_b);
        let ord = match (a_num, b_num) {
            (Some(na), Some(nb)) => na.cmp(&nb),
            // Per SemVer 2.0.0: numeric identifiers always have lower precedence than non-numeric.
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => id_a.cmp(id_b),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }

    a_parts.len().cmp(&b_parts.len())
}

fn parse_digits_u64(s: &str) -> Option<u64> {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse::<u64>().ok()
    } else {
        None
    }
}

pub fn parse_version(raw: &str) -> Option<SemVer> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let stripped = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);

    let without_build = stripped.split('+').next()?;
    if without_build.is_empty() {
        return None;
    }

    let (core, pre) = match without_build.split_once('-') {
        Some((c, p)) => {
            if p.is_empty() || p.split('.').any(|part| part.is_empty()) {
                return None;
            }
            (c, Some(p.to_string()))
        }
        None => (without_build, None),
    };

    let parts: Vec<&str> = core.split('.').collect();
    match parts.as_slice() {
        [maj, min] => Some(SemVer {
            major: parse_digits_u64(maj)?,
            minor: parse_digits_u64(min)?,
            patch: 0,
            pre,
        }),
        [maj, min, pat] => Some(SemVer {
            major: parse_digits_u64(maj)?,
            minor: parse_digits_u64(min)?,
            patch: parse_digits_u64(pat)?,
            pre,
        }),
        _ => None,
    }
}

pub fn normalize_version_tag(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
    {
        format!("v{rest}")
    } else {
        format!("v{trimmed}")
    }
}

pub fn is_dev_build_path_only(exe_path: &Path) -> bool {
    let has_target_component = exe_path.components().any(|comp| {
        comp.as_os_str()
            .to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("target"))
    });
    if has_target_component {
        return true;
    }

    let raw_str = exe_path.to_string_lossy();
    if raw_str
        .split(['/', '\\'])
        .any(|part| part.eq_ignore_ascii_case("target"))
    {
        return true;
    }

    exe_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .is_some_and(|parent| parent.join("Cargo.toml").exists())
}

pub fn is_dev_build_exe(exe_path: &Path) -> bool {
    cfg!(debug_assertions) || is_dev_build_path_only(exe_path)
}

pub fn current_platform_asset_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "echoes-below-windows-x64.zip"
    }
    #[cfg(target_os = "macos")]
    {
        "echoes-below-macos-arm64.zip"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "echoes-below-linux-x64.tar.gz"
    }
}

pub fn current_platform_binary_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "echoes-below.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "echoes-below"
    }
}

fn old_binary_path(exe_path: &Path) -> PathBuf {
    let mut os = exe_path.as_os_str().to_os_string();
    os.push(".old");
    PathBuf::from(os)
}

pub fn cleanup_stale_old_binaries(exe_path: &Path) {
    let _ = fs::remove_file(old_binary_path(exe_path));
    if let Some(parent) = exe_path.parent() {
        let assets_old = parent.join("assets.old");
        let _ = fs::remove_dir_all(&assets_old);
        let _ = fs::remove_file(&assets_old);

        let staging = parent.join(".echoes-update-staging");
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_file(&staging);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub prerelease: bool,
    pub draft: bool,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<GitHubAsset>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version_tag: String,
    pub parsed_version: SemVer,
    pub prerelease: bool,
    pub release_notes: String,
    pub html_url: String,
    pub archive_name: String,
    pub archive_url: String,
    pub archive_size_bytes: Option<u64>,
    pub checksums_url: Option<String>,
}

pub fn select_best_update(
    current_version: &str,
    releases: &[GitHubRelease],
    include_prereleases: bool,
    target_asset_name: &str,
) -> Option<UpdateInfo> {
    let current_semver = parse_version(current_version)?;
    let mut best: Option<UpdateInfo> = None;

    for release in releases {
        if release.draft {
            continue;
        }
        if release.prerelease && !include_prereleases {
            continue;
        }
        let Some(parsed_version) = parse_version(&release.tag_name) else {
            continue;
        };
        if parsed_version <= current_semver {
            continue;
        }
        let Some(archive_asset) = release.assets.iter().find(|a| a.name == target_asset_name)
        else {
            continue;
        };
        let checksums_url = release
            .assets
            .iter()
            .find(|a| a.name == CHECKSUMS_ASSET_NAME)
            .map(|a| a.browser_download_url.clone());

        let candidate = UpdateInfo {
            version_tag: normalize_version_tag(&release.tag_name),
            parsed_version,
            prerelease: release.prerelease,
            release_notes: release.body.clone().unwrap_or_default(),
            html_url: release.html_url.clone(),
            archive_name: archive_asset.name.clone(),
            archive_url: archive_asset.browser_download_url.clone(),
            archive_size_bytes: archive_asset.size,
            checksums_url,
        };

        let should_replace = match &best {
            None => true,
            Some(existing) => candidate.parsed_version > existing.parsed_version,
        };
        if should_replace {
            best = Some(candidate);
        }
    }

    best
}

pub fn check_for_updates(
    current_version: &str,
    include_prereleases: bool,
) -> Result<Option<UpdateInfo>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(5))
        .build();

    let user_agent = format!("echoes-below-updater/{APP_VERSION}");
    let response = agent
        .get(RELEASES_API_URL)
        .set("User-Agent", &user_agent)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("Failed to query GitHub Releases API: {e}"))?;

    let releases: Vec<GitHubRelease> = serde_json::from_reader(response.into_reader())
        .map_err(|e| format!("Failed to parse GitHub Releases JSON: {e}"))?;

    Ok(select_best_update(
        current_version,
        &releases,
        include_prereleases,
        current_platform_asset_name(),
    ))
}

pub fn parse_sha256sums(contents: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let Some(hash_raw) = parts.next() else {
            continue;
        };
        let Some(rest) = parts.next() else {
            continue;
        };

        let hash = hash_raw.trim().to_ascii_lowercase();
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }

        let mut filename = rest.trim_start();
        if let Some(stripped) = filename.strip_prefix('*') {
            filename = stripped;
        }
        filename = filename.trim();
        while let Some(stripped) = filename.strip_prefix("./") {
            filename = stripped;
        }

        if !filename.is_empty() {
            map.insert(filename.to_string(), hash);
        }
    }
    map
}

pub fn compute_file_sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|e| format!("Failed to open {} for hashing: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_CHUNK_SIZE];

    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Failed to read {} while hashing: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for &byte in digest.as_slice() {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

pub fn verify_file_sha256(path: &Path, expected_hex: &str) -> Result<(), String> {
    let expected = expected_hex.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("Invalid expected SHA-256 hex digest: '{expected_hex}'"));
    }
    let actual = compute_file_sha256(path)?;
    if actual != expected {
        return Err(format!(
            "SHA-256 verification failed for {}: expected {expected}, got {actual}",
            path.display()
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateProgress {
    Downloading {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Verifying,
    Extracting,
    Installing,
    ReadyToRelaunch {
        exe_path: PathBuf,
    },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedPayload {
    pub binary_path: PathBuf,
    pub assets_dir: Option<PathBuf>,
}

fn sanitize_archive_entry_path(raw_name: &str) -> Result<PathBuf, String> {
    if raw_name.starts_with('/') || raw_name.starts_with('\\') || raw_name.contains(':') {
        return Err(format!("Unsafe archive entry path: {raw_name}"));
    }
    let normalized = raw_name.replace('\\', "/");
    let mut clean = PathBuf::new();
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err(format!("Path traversal detected in archive entry: {raw_name}"));
        }
        clean.push(segment);
    }

    for comp in clean.components() {
        match comp {
            Component::Normal(_) => {}
            _ => {
                return Err(format!(
                    "Disallowed path component in archive entry: {raw_name}"
                ));
            }
        }
    }

    Ok(clean)
}

pub fn extract_archive_to_dir(
    archive_path: &Path,
    archive_name: &str,
    dest_dir: &Path,
) -> Result<ExtractedPayload, String> {
    fs::create_dir_all(dest_dir)
        .map_err(|e| format!("Failed to create extraction directory {}: {e}", dest_dir.display()))?;

    let lower_name = archive_name.to_ascii_lowercase();
    if lower_name.ends_with(".zip") {
        extract_zip(archive_path, dest_dir)?;
    } else if lower_name.ends_with(".tar.gz") || lower_name.ends_with(".tgz") {
        extract_tar_gz(archive_path, dest_dir)?;
    } else {
        return Err(format!("Unsupported archive format: {archive_name}"));
    }

    locate_extracted_payload(dest_dir)
}

fn extract_zip(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|e| format!("Failed to open zip archive {}: {e}", archive_path.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| format!("Failed to read zip archive {}: {e}", archive_path.display()))?;

    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Failed to read zip entry #{i}: {e}"))?;
        let raw_name = entry.name().to_string();
        let rel_path = sanitize_archive_entry_path(&raw_name)?;
        if rel_path.as_os_str().is_empty() {
            continue;
        }

        let out_path = dest_dir.join(&rel_path);
        if entry.is_dir() || raw_name.ends_with('/') || raw_name.ends_with('\\') {
            fs::create_dir_all(&out_path).map_err(|e| {
                format!(
                    "Failed to create directory {} during zip extraction: {e}",
                    out_path.display()
                )
            })?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "Failed to create parent directory {} during zip extraction: {e}",
                        parent.display()
                    )
                })?;
            }
            let mut out_file = File::create(&out_path).map_err(|e| {
                format!(
                    "Failed to create file {} during zip extraction: {e}",
                    out_path.display()
                )
            })?;
            io::copy(&mut entry, &mut out_file).map_err(|e| {
                format!(
                    "Failed to extract file {} from zip archive: {e}",
                    out_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn extract_tar_gz(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|e| format!("Failed to open tar.gz archive {}: {e}", archive_path.display()))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read tar.gz entries: {e}"))?;

    for entry_res in entries {
        let mut entry = entry_res.map_err(|e| format!("Invalid tar.gz entry: {e}"))?;
        let raw_name = entry
            .path()
            .map_err(|e| format!("Invalid path in tar.gz entry: {e}"))?
            .to_string_lossy()
            .into_owned();
        let rel_path = sanitize_archive_entry_path(&raw_name)?;
        if rel_path.as_os_str().is_empty() {
            continue;
        }

        let entry_type = entry.header().entry_type();
        let out_path = dest_dir.join(&rel_path);
        if entry_type.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| {
                format!(
                    "Failed to create directory {} during tar.gz extraction: {e}",
                    out_path.display()
                )
            })?;
        } else if entry_type.is_file() || entry_type.is_contiguous() {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "Failed to create parent directory {} during tar.gz extraction: {e}",
                        parent.display()
                    )
                })?;
            }
            let mut out_file = File::create(&out_path).map_err(|e| {
                format!(
                    "Failed to create file {} during tar.gz extraction: {e}",
                    out_path.display()
                )
            })?;
            io::copy(&mut entry, &mut out_file).map_err(|e| {
                format!(
                    "Failed to extract file {} from tar.gz archive: {e}",
                    out_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn locate_extracted_payload(dest_dir: &Path) -> Result<ExtractedPayload, String> {
    #[cfg(target_os = "windows")]
    const CANDIDATE_BINS: [&str; 4] = ["echoes-below.exe", "game.exe", "echoes-below", "game"];
    #[cfg(not(target_os = "windows"))]
    const CANDIDATE_BINS: [&str; 4] = ["echoes-below", "game", "echoes-below.exe", "game.exe"];

    let mut search_roots = vec![dest_dir.to_path_buf()];
    let nested_default = dest_dir.join("echoes-below");
    if nested_default.is_dir() {
        search_roots.push(nested_default);
    }
    if let Ok(read_dir) = fs::read_dir(dest_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() && !search_roots.contains(&path) {
                search_roots.push(path);
            }
        }
    }

    let mut found_binary: Option<PathBuf> = None;
    'outer: for root in &search_roots {
        for bin_name in CANDIDATE_BINS {
            let candidate = root.join(bin_name);
            if candidate.is_file() {
                found_binary = Some(candidate);
                break 'outer;
            }
        }
    }

    let binary_path = found_binary.ok_or_else(|| {
        format!(
            "Extracted archive in {} did not contain an echoes-below binary",
            dest_dir.display()
        )
    })?;

    let mut assets_dir = None;
    if let Some(bin_parent) = binary_path.parent() {
        let sibling_assets = bin_parent.join("assets");
        if sibling_assets.is_dir() {
            assets_dir = Some(sibling_assets);
        }
    }
    if assets_dir.is_none() {
        for root in &search_roots {
            let candidate_assets = root.join("assets");
            if candidate_assets.is_dir() {
                assets_dir = Some(candidate_assets);
                break;
            }
        }
    }

    Ok(ExtractedPayload {
        binary_path,
        assets_dir,
    })
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create directory {}: {e}", dst.display()))?;
    let entries = fs::read_dir(src)
        .map_err(|e| format!("Failed to read directory {}: {e}", src.display()))?;

    for entry_res in entries {
        let entry = entry_res.map_err(|e| format!("Failed to read dir entry: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(|e| {
                format!(
                    "Failed to copy {} to {}: {e}",
                    src_path.display(),
                    dst_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn move_or_copy_file(src: &Path, dst: &Path) -> Result<(), String> {
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    fs::copy(src, dst)
        .map(|_| ())
        .map_err(|e| format!("Failed to copy {} to {}: {e}", src.display(), dst.display()))
}

pub fn install_extracted_payload(
    install_dir: &Path,
    current_exe: &Path,
    payload: &ExtractedPayload,
) -> Result<PathBuf, String> {
    if !payload.binary_path.is_file() {
        return Err(format!(
            "Extracted binary {} does not exist",
            payload.binary_path.display()
        ));
    }

    fs::create_dir_all(install_dir).map_err(|e| {
        format!(
            "Failed to create installation directory {}: {e}",
            install_dir.display()
        )
    })?;
    if let Some(exe_parent) = current_exe.parent() {
        fs::create_dir_all(exe_parent).map_err(|e| {
            format!(
                "Failed to create executable parent directory {}: {e}",
                exe_parent.display()
            )
        })?;
    }

    #[cfg(windows)]
    {
        let old_exe = old_binary_path(current_exe);
        if old_exe.exists() {
            let _ = fs::remove_file(&old_exe);
        }
        let had_current = current_exe.exists();
        if had_current {
            fs::rename(current_exe, &old_exe).map_err(|e| {
                format!(
                    "Failed to rename running executable {} to {}: {e}",
                    current_exe.display(),
                    old_exe.display()
                )
            })?;
        }

        if let Err(err) = move_or_copy_file(&payload.binary_path, current_exe) {
            if had_current {
                let _ = fs::rename(&old_exe, current_exe);
            }
            return Err(err);
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&payload.binary_path, perms.clone()).map_err(|e| {
            format!(
                "Failed to set executable permissions on {}: {e}",
                payload.binary_path.display()
            )
        })?;

        if fs::rename(&payload.binary_path, current_exe).is_err() {
            let mut tmp_os = current_exe.as_os_str().to_os_string();
            tmp_os.push(".new");
            let tmp_new = PathBuf::from(tmp_os);
            let _ = fs::remove_file(&tmp_new);
            fs::copy(&payload.binary_path, &tmp_new).map_err(|e| {
                format!(
                    "Failed to stage updated binary to {}: {e}",
                    tmp_new.display()
                )
            })?;
            let _ = fs::set_permissions(&tmp_new, perms);
            fs::rename(&tmp_new, current_exe).map_err(|e| {
                let _ = fs::remove_file(&tmp_new);
                format!(
                    "Failed to atomically replace {} with {}: {e}",
                    current_exe.display(),
                    tmp_new.display()
                )
            })?;
        }
    }

    if let Some(new_assets) = &payload.assets_dir {
        let target_assets = install_dir.join("assets");
        let backup_assets = install_dir.join("assets.old");
        if backup_assets.exists() {
            let _ = fs::remove_dir_all(&backup_assets);
        }

        let had_old_assets = target_assets.exists();
        if had_old_assets {
            fs::rename(&target_assets, &backup_assets).map_err(|e| {
                format!(
                    "Failed to move existing assets {} to {}: {e}",
                    target_assets.display(),
                    backup_assets.display()
                )
            })?;
        }

        let assets_swap_result = if fs::rename(new_assets, &target_assets).is_ok() {
            Ok(())
        } else {
            copy_dir_recursive(new_assets, &target_assets)
        };

        if let Err(err) = assets_swap_result {
            let _ = fs::remove_dir_all(&target_assets);
            if had_old_assets {
                let _ = fs::rename(&backup_assets, &target_assets);
            }
            return Err(err);
        }

        if backup_assets.exists() {
            let _ = fs::remove_dir_all(&backup_assets);
        }
    }

    Ok(current_exe.to_path_buf())
}

struct StagingCleanupGuard {
    path: PathBuf,
}

impl Drop for StagingCleanupGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn download_and_install_update<F>(
    info: &UpdateInfo,
    current_exe: &Path,
    mut on_progress: F,
) -> Result<PathBuf, String>
where
    F: FnMut(UpdateProgress),
{
    if is_dev_build_exe(current_exe) {
        let msg = "Auto-update is disabled for dev/target builds. Download the packaged release from GitHub instead.".to_string();
        on_progress(UpdateProgress::Failed(msg.clone()));
        return Err(msg);
    }

    let result = download_and_install_inner(info, current_exe, &mut on_progress);
    if let Err(ref err) = result {
        on_progress(UpdateProgress::Failed(err.clone()));
    }
    result
}

fn download_and_install_inner<F>(
    info: &UpdateInfo,
    current_exe: &Path,
    on_progress: &mut F,
) -> Result<PathBuf, String>
where
    F: FnMut(UpdateProgress),
{
    let Some(checksums_url) = info.checksums_url.as_deref() else {
        return Err(format!(
            "Release {} is missing {}; refusing unverified update.",
            info.version_tag, CHECKSUMS_ASSET_NAME
        ));
    };

    let install_dir = current_exe.parent().ok_or_else(|| {
        format!(
            "Cannot determine installation folder from executable path {}",
            current_exe.display()
        )
    })?;

    let staging_dir = install_dir.join(".echoes-update-staging");
    if staging_dir.exists() {
        let _ = fs::remove_dir_all(&staging_dir);
    }
    fs::create_dir_all(&staging_dir).map_err(|e| {
        format!(
            "Failed to create update staging directory {}: {e}",
            staging_dir.display()
        )
    })?;
    let _staging_guard = StagingCleanupGuard {
        path: staging_dir.clone(),
    };

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build();
    let user_agent = format!("echoes-below-updater/{APP_VERSION}");

    let sums_response = agent
        .get(checksums_url)
        .set("User-Agent", &user_agent)
        .call()
        .map_err(|e| format!("Failed to download {CHECKSUMS_ASSET_NAME}: {e}"))?;

    let mut sums_text = String::new();
    sums_response
        .into_reader()
        .take(256 * 1024)
        .read_to_string(&mut sums_text)
        .map_err(|e| format!("Failed to read {CHECKSUMS_ASSET_NAME}: {e}"))?;

    let sums_map = parse_sha256sums(&sums_text);
    let expected_hash = sums_map
        .get(&info.archive_name)
        .ok_or_else(|| {
            format!(
                "Checksum for '{}' was not found in {}",
                info.archive_name, CHECKSUMS_ASSET_NAME
            )
        })?
        .clone();

    let archive_response = agent
        .get(&info.archive_url)
        .set("User-Agent", &user_agent)
        .call()
        .map_err(|e| format!("Failed to download archive {}: {e}", info.archive_name))?;

    let total_bytes = archive_response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .or(info.archive_size_bytes);

    let archive_path = staging_dir.join(&info.archive_name);
    {
        let mut archive_file = File::create(&archive_path).map_err(|e| {
            format!(
                "Failed to create staged archive {}: {e}",
                archive_path.display()
            )
        })?;
        let mut reader = archive_response.into_reader();
        let mut buf = vec![0u8; STREAM_CHUNK_SIZE];
        let mut downloaded_bytes: u64 = 0;

        on_progress(UpdateProgress::Downloading {
            downloaded_bytes: 0,
            total_bytes,
        });

        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("Error while downloading {}: {e}", info.archive_name))?;
            if n == 0 {
                break;
            }
            archive_file
                .write_all(&buf[..n])
                .map_err(|e| format!("Failed writing staged archive: {e}"))?;
            downloaded_bytes += n as u64;
            on_progress(UpdateProgress::Downloading {
                downloaded_bytes,
                total_bytes,
            });
        }
        archive_file
            .flush()
            .map_err(|e| format!("Failed flushing staged archive: {e}"))?;
    }

    on_progress(UpdateProgress::Verifying);
    verify_file_sha256(&archive_path, &expected_hash)?;

    on_progress(UpdateProgress::Extracting);
    let extract_dir = staging_dir.join("extracted");
    let payload = extract_archive_to_dir(&archive_path, &info.archive_name, &extract_dir)?;

    on_progress(UpdateProgress::Installing);
    let installed_exe = install_extracted_payload(install_dir, current_exe, &payload)?;

    let _ = fs::remove_dir_all(&staging_dir);
    on_progress(UpdateProgress::ReadyToRelaunch {
        exe_path: installed_exe.clone(),
    });

    Ok(installed_exe)
}

pub fn spawn_relaunch(exe_path: &Path) -> Result<(), String> {
    let mut cmd = Command::new(exe_path);
    if let Some(parent) = exe_path.parent()
        && !parent.as_os_str().is_empty()
    {
        cmd.current_dir(parent);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to relaunch {}: {e}", exe_path.display()))
}

pub fn format_release_notes_preview(notes: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut clean_lines: Vec<String> = Vec::new();
    for raw_line in notes.trim().lines() {
        let mut line = raw_line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        // Skip noisy auto-generated GitHub Release boilerplate
        if lower.starts_with("## what's changed")
            || lower.starts_with("### what's changed")
            || lower.starts_with("**full changelog**")
            || lower.starts_with("full changelog")
        {
            continue;
        }

        // Strip markdown heading hashes & bold asterisks
        line = line.trim_start_matches('#').trim().replace("**", "");
        // Strip trailing " by @author in https://..." GitHub PR attribution
        if let Some(idx) = line.find(" by @") {
            line.truncate(idx);
            line = line.trim_end().to_string();
        }
        // Strip any trailing raw URL
        if let Some(idx) = line.find(" http://").or_else(|| line.find(" https://")) {
            line.truncate(idx);
            line = line.trim_end().to_string();
        }
        if line.is_empty() {
            continue;
        }

        // Convert markdown list markers (* / -) to clean bullet points
        let formatted = if let Some(rest) = line.strip_prefix("* ").or_else(|| line.strip_prefix("- ")) {
            format!("• {}", rest.trim())
        } else {
            line
        };

        clean_lines.push(formatted);
        if clean_lines.len() >= 5 {
            break;
        }
    }

    let joined = clean_lines.join("\n");
    let cleaned = joined.trim();
    if cleaned.chars().count() <= max_chars {
        return cleaned.to_string();
    }

    let limit = max_chars.saturating_sub(1);
    let truncated: String = cleaned.chars().take(limit).collect();
    format!("{}…", truncated.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempTestDir {
        path: PathBuf,
    }

    impl TempTestDir {
        fn new(prefix: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "echoes_updater_test_{prefix}_{}_{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).expect("create temp test dir");
            Self { path: dir }
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn semver_parsing_and_ordering() {
        let v013 = parse_version("v0.1.3").expect("v0.1.3");
        let v014 = parse_version("V0.1.4").expect("V0.1.4");
        let v020_short = parse_version("v0.2").expect("v0.2");
        let v020 = parse_version("0.2.0+build.42").expect("0.2.0+build.42");
        let v020_rc1 = parse_version("v0.2.0-rc.1").expect("v0.2.0-rc.1");
        let v020_rc2 = parse_version("v0.2.0-rc.2").expect("v0.2.0-rc.2");
        let v020_alpha = parse_version("v0.2.0-alpha.1").expect("v0.2.0-alpha.1");

        assert_eq!(v020_short, v020);
        assert!(v013 < v014);
        assert!(v014 < v020_rc1);
        assert!(v020_alpha < v020_rc1);
        assert!(v020_rc1 < v020_rc2);
        assert!(v020_rc2 < v020);

        assert_eq!(normalize_version_tag("0.1.4"), "v0.1.4");
        assert_eq!(normalize_version_tag("V0.1.4"), "v0.1.4");
        assert_eq!(normalize_version_tag("  v0.2.0-rc.1 "), "v0.2.0-rc.1");

        assert!(parse_version("").is_none());
        assert!(parse_version("v1").is_none());
        assert!(parse_version("v1.2.3.4").is_none());
        assert!(parse_version("v1.2.x").is_none());
        assert!(parse_version("v1.2.0-").is_none());
    }

    #[test]
    fn release_selection_filters_prereleases_drafts_and_platform_assets() {
        let target_asset = "echoes-below-windows-x64.zip";
        let releases = vec![
            GitHubRelease {
                tag_name: "v0.3.0".to_string(),
                prerelease: false,
                draft: true,
                body: Some("Draft release".to_string()),
                html_url: "https://github.com/Pirs26/voxel/releases/tag/v0.3.0".to_string(),
                assets: vec![GitHubAsset {
                    name: target_asset.to_string(),
                    browser_download_url: "https://example.com/v0.3.0.zip".to_string(),
                    size: Some(1000),
                }],
            },
            GitHubRelease {
                tag_name: "v0.2.5".to_string(),
                prerelease: false,
                draft: false,
                body: Some("Other platform only".to_string()),
                html_url: "https://github.com/Pirs26/voxel/releases/tag/v0.2.5".to_string(),
                assets: vec![GitHubAsset {
                    name: "echoes-below-linux-x64.tar.gz".to_string(),
                    browser_download_url: "https://example.com/v0.2.5.tar.gz".to_string(),
                    size: Some(1000),
                }],
            },
            GitHubRelease {
                tag_name: "v0.2.0-rc.1".to_string(),
                prerelease: true,
                draft: false,
                body: Some("Release candidate".to_string()),
                html_url: "https://github.com/Pirs26/voxel/releases/tag/v0.2.0-rc.1".to_string(),
                assets: vec![
                    GitHubAsset {
                        name: target_asset.to_string(),
                        browser_download_url: "https://example.com/v0.2.0-rc.1.zip".to_string(),
                        size: Some(2048),
                    },
                    GitHubAsset {
                        name: CHECKSUMS_ASSET_NAME.to_string(),
                        browser_download_url: "https://example.com/rc1-SHA256SUMS.txt".to_string(),
                        size: Some(128),
                    },
                ],
            },
            GitHubRelease {
                tag_name: "v0.1.4".to_string(),
                prerelease: false,
                draft: false,
                body: Some("Stable patch".to_string()),
                html_url: "https://github.com/Pirs26/voxel/releases/tag/v0.1.4".to_string(),
                assets: vec![
                    GitHubAsset {
                        name: target_asset.to_string(),
                        browser_download_url: "https://example.com/v0.1.4.zip".to_string(),
                        size: Some(4096),
                    },
                    GitHubAsset {
                        name: CHECKSUMS_ASSET_NAME.to_string(),
                        browser_download_url: "https://example.com/v0.1.4-SHA256SUMS.txt"
                            .to_string(),
                        size: Some(128),
                    },
                ],
            },
        ];

        let stable_choice = select_best_update("v0.1.3", &releases, false, target_asset)
            .expect("should pick v0.1.4 stable");
        assert_eq!(stable_choice.version_tag, "v0.1.4");
        assert!(!stable_choice.prerelease);
        assert_eq!(
            stable_choice.checksums_url.as_deref(),
            Some("https://example.com/v0.1.4-SHA256SUMS.txt")
        );

        let prerelease_choice = select_best_update("v0.1.3", &releases, true, target_asset)
            .expect("should pick v0.2.0-rc.1 when prereleases enabled");
        assert_eq!(prerelease_choice.version_tag, "v0.2.0-rc.1");
        assert!(prerelease_choice.prerelease);

        let none_when_up_to_date = select_best_update("v0.2.0", &releases, true, target_asset);
        assert!(none_when_up_to_date.is_none());
    }

    #[test]
    fn sha256sums_parsing_and_verification_valid_and_tampered() {
        let tmp = TempTestDir::new("sha256");
        let payload_path = tmp.path.join("echoes-below-windows-x64.zip");
        fs::write(&payload_path, b"echoes-below-binary-payload-v0.1.4").expect("write payload");

        let actual_hash = compute_file_sha256(&payload_path).expect("compute sha256");
        let sums_txt = format!(
            "{actual_hash}  echoes-below-windows-x64.zip\n\
             0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef *./echoes-below-linux-x64.tar.gz\n"
        );

        let parsed = parse_sha256sums(&sums_txt);
        assert_eq!(
            parsed
                .get("echoes-below-windows-x64.zip")
                .map(String::as_str),
            Some(actual_hash.as_str())
        );
        assert_eq!(
            parsed
                .get("echoes-below-linux-x64.tar.gz")
                .map(String::as_str),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );

        verify_file_sha256(&payload_path, &actual_hash).expect("valid hash should verify");

        fs::write(&payload_path, b"tampered-payload").expect("tamper file");
        let err = verify_file_sha256(&payload_path, &actual_hash)
            .expect_err("tampered payload must fail verification");
        assert!(err.contains("SHA-256"));
    }

    #[test]
    fn zip_and_targz_extraction_and_install_swap_preserves_saves() {
        let tmp = TempTestDir::new("archive_swap");
        let bin_name = current_platform_binary_name();

        // 1. Build a real .zip archive containing echoes-below/<bin> and echoes-below/assets/sfx/test.txt
        let zip_path = tmp.path.join("echoes-below-windows-x64.zip");
        {
            let file = File::create(&zip_path).expect("create zip");
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            zip.start_file(format!("echoes-below/{bin_name}"), opts)
                .expect("start zip bin");
            zip.write_all(b"NEW_BINARY_FROM_ZIP")
                .expect("write zip bin");

            zip.start_file("echoes-below/assets/sfx/test.txt", opts)
                .expect("start zip asset");
            zip.write_all(b"NEW_ASSET_FROM_ZIP")
                .expect("write zip asset");

            zip.finish().expect("finish zip");
        }

        // Mock install dir for zip test
        let install_zip_dir = tmp.path.join("install_zip");
        fs::create_dir_all(install_zip_dir.join("assets/sfx")).expect("create old assets");
        fs::create_dir_all(install_zip_dir.join("saves")).expect("create saves");
        let exe_zip_path = install_zip_dir.join(bin_name);
        fs::write(&exe_zip_path, b"OLD_BINARY").expect("write old bin");
        fs::write(install_zip_dir.join("assets/sfx/test.txt"), b"OLD_ASSET")
            .expect("write old asset");
        fs::write(install_zip_dir.join("saves/default.json"), b"{\"seed\":42}")
            .expect("write save");

        let extract_zip_dir = tmp.path.join("extract_zip");
        let zip_payload = extract_archive_to_dir(
            &zip_path,
            "echoes-below-windows-x64.zip",
            &extract_zip_dir,
        )
        .expect("extract zip");
        install_extracted_payload(&install_zip_dir, &exe_zip_path, &zip_payload)
            .expect("install zip payload");

        assert_eq!(
            fs::read(&exe_zip_path).expect("read updated zip bin"),
            b"NEW_BINARY_FROM_ZIP"
        );
        assert_eq!(
            fs::read(install_zip_dir.join("assets/sfx/test.txt")).expect("read updated zip asset"),
            b"NEW_ASSET_FROM_ZIP"
        );
        assert_eq!(
            fs::read(install_zip_dir.join("saves/default.json")).expect("read preserved save"),
            b"{\"seed\":42}"
        );

        // 2. Build a real .tar.gz archive containing echoes-below/<bin> and echoes-below/assets/sfx/test.txt
        let targz_path = tmp.path.join("echoes-below-linux-x64.tar.gz");
        {
            let file = File::create(&targz_path).expect("create tar.gz");
            let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut tar = tar::Builder::new(enc);

            let bin_bytes = b"NEW_BINARY_FROM_TARGZ";
            let mut bin_header = tar::Header::new_gnu();
            bin_header.set_size(bin_bytes.len() as u64);
            bin_header.set_mode(0o755);
            bin_header.set_cksum();
            tar.append_data(
                &mut bin_header,
                format!("echoes-below/{bin_name}"),
                &bin_bytes[..],
            )
            .expect("append tar bin");

            let asset_bytes = b"NEW_ASSET_FROM_TARGZ";
            let mut asset_header = tar::Header::new_gnu();
            asset_header.set_size(asset_bytes.len() as u64);
            asset_header.set_mode(0o644);
            asset_header.set_cksum();
            tar.append_data(
                &mut asset_header,
                "echoes-below/assets/sfx/test.txt",
                &asset_bytes[..],
            )
            .expect("append tar asset");

            let enc = tar.into_inner().expect("finish tar");
            enc.finish().expect("finish gz");
        }

        let install_tar_dir = tmp.path.join("install_tar");
        fs::create_dir_all(install_tar_dir.join("assets/sfx")).expect("create old tar assets");
        fs::create_dir_all(install_tar_dir.join("saves")).expect("create tar saves");
        let exe_tar_path = install_tar_dir.join(bin_name);
        fs::write(&exe_tar_path, b"OLD_BINARY_TAR").expect("write old tar bin");
        fs::write(
            install_tar_dir.join("assets/sfx/test.txt"),
            b"OLD_ASSET_TAR",
        )
        .expect("write old tar asset");
        fs::write(
            install_tar_dir.join("saves/default.json"),
            b"{\"seed\":99}",
        )
        .expect("write tar save");

        let extract_tar_dir = tmp.path.join("extract_tar");
        let tar_payload = extract_archive_to_dir(
            &targz_path,
            "echoes-below-linux-x64.tar.gz",
            &extract_tar_dir,
        )
        .expect("extract tar.gz");
        install_extracted_payload(&install_tar_dir, &exe_tar_path, &tar_payload)
            .expect("install tar.gz payload");

        assert_eq!(
            fs::read(&exe_tar_path).expect("read updated tar bin"),
            b"NEW_BINARY_FROM_TARGZ"
        );
        assert_eq!(
            fs::read(install_tar_dir.join("assets/sfx/test.txt")).expect("read updated tar asset"),
            b"NEW_ASSET_FROM_TARGZ"
        );
        assert_eq!(
            fs::read(install_tar_dir.join("saves/default.json")).expect("read preserved tar save"),
            b"{\"seed\":99}"
        );

        cleanup_stale_old_binaries(&exe_zip_path);
        assert!(!old_binary_path(&exe_zip_path).exists());
    }

    #[test]
    fn dev_build_path_guard_detects_target_and_allows_install_dir() {
        assert!(is_dev_build_path_only(Path::new(
            "/home/player/voxel/target/release/echoes-below"
        )));
        assert!(is_dev_build_path_only(Path::new(
            r"C:\Users\player\voxel\target\debug\game.exe"
        )));
        assert!(!is_dev_build_path_only(Path::new(
            r"C:\Games\EchoesBelow\echoes-below.exe"
        )));
        assert!(!is_dev_build_path_only(Path::new(
            "/opt/echoes-below/echoes-below"
        )));

        let tmp = TempTestDir::new("dev_guard");
        let cargo_dir = tmp.path.join("workspace_pkg");
        fs::create_dir_all(&cargo_dir).expect("create cargo_dir");
        fs::write(cargo_dir.join("Cargo.toml"), b"[package]\nname = \"test\"\n")
            .expect("write Cargo.toml");
        assert!(is_dev_build_path_only(&cargo_dir.join("echoes-below")));
    }
}
