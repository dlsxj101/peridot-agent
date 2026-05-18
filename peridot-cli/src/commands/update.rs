use super::*;

#[derive(Clone, Debug)]
pub(super) struct LatestRelease {
    release: Value,
    latest: String,
    html_url: String,
    update_available: bool,
}

pub(crate) async fn maybe_print_update_notice(
    config: &PeridotConfig,
    headless: bool,
    output: OutputFormat,
) {
    if headless || output == OutputFormat::Json || !config.updates.auto_check {
        return;
    }
    if !update_check_due(&config.updates.auto_check_interval) {
        return;
    }
    let result = query_latest_release().await;
    let _ = mark_update_checked();
    let Ok(latest) = result else {
        return;
    };
    if !latest.update_available {
        return;
    }
    if config.updates.auto_install {
        match install_update(&latest.release).await {
            Ok(path) => eprintln!(
                "Peridot v{} installed at {}.",
                latest.latest,
                path.display()
            ),
            Err(err) => eprintln!(
                "Peridot v{} is available, but auto-update failed: {err}",
                latest.latest
            ),
        }
        return;
    }
    eprintln!(
        "Peridot v{} is available (current v{}). Run {} to update.",
        latest.latest,
        env!("CARGO_PKG_VERSION"),
        update_command_hint()
    );
}

pub(crate) async fn run_update_command(
    check: bool,
    force: bool,
    output: OutputFormat,
) -> Result<()> {
    let latest = query_latest_release().await?;
    let should_install = !check && (force || latest.update_available);
    let installed_path = if should_install {
        Some(install_update(&latest.release).await?)
    } else {
        None
    };
    print_json_or_text_result(
        serde_json::json!({
            "current": env!("CARGO_PKG_VERSION"),
            "latest": latest.latest,
            "update_available": latest.update_available,
            "release_url": latest.html_url,
            "checked_only": check,
            "forced": force,
            "installed_path": installed_path
        }),
        if let Some(path) = installed_path {
            if latest.update_available {
                format!(
                    "Updated Peridot from {} to {} at {}",
                    env!("CARGO_PKG_VERSION"),
                    latest.latest,
                    path.display()
                )
            } else {
                format!(
                    "Reinstalled Peridot {} at {}",
                    latest.latest,
                    path.display()
                )
            }
        } else if latest.update_available {
            format!(
                "Peridot {} is available (current {}): {}",
                latest.latest,
                env!("CARGO_PKG_VERSION"),
                latest.html_url
            )
        } else {
            format!("Peridot is up to date ({})", env!("CARGO_PKG_VERSION"))
        },
        output,
    )
}

pub(super) async fn query_latest_release() -> Result<LatestRelease> {
    let current = env!("CARGO_PKG_VERSION");
    let repo = std::env::var("PERIDOT_UPDATE_REPO")
        .unwrap_or_else(|_| env!("CARGO_PKG_REPOSITORY").to_string());
    let Some((owner, name)) = github_owner_repo(&repo) else {
        anyhow::bail!("repository is not a GitHub URL: {repo}");
    };
    let url = format!("https://api.github.com/repos/{owner}/{name}/releases/latest");
    let response = reqwest::Client::new()
        .get(&url)
        .header("user-agent", "peridot-agent")
        .send()
        .await
        .with_context(|| format!("failed to query {url}"))?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("GitHub latest release query returned {status}: {body}");
    }
    let value = serde_json::from_str::<Value>(&body)?;
    let latest = value
        .get("tag_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_start_matches('v')
        .to_string();
    let html_url = value
        .get("html_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let update_available = !latest.is_empty() && latest != current;
    Ok(LatestRelease {
        release: value,
        latest,
        html_url,
        update_available,
    })
}

pub(super) fn update_check_due(interval: &str) -> bool {
    let interval = parse_update_interval(interval);
    let Some(path) = update_check_state_path() else {
        return true;
    };
    let Ok(content) = fs::read_to_string(path) else {
        return true;
    };
    let Ok(last_checked) = content.trim().parse::<u64>() else {
        return true;
    };
    let now = unix_timestamp();
    now.saturating_sub(last_checked) >= interval.as_secs()
}

pub(super) fn mark_update_checked() -> Result<()> {
    let Some(path) = update_check_state_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, unix_timestamp().to_string())?;
    Ok(())
}

pub(super) fn update_check_state_path() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("PERIDOT_HOME") {
        return Some(PathBuf::from(home).join("update-check"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".peridot/update-check"))
}

pub(super) fn parse_update_interval(value: &str) -> Duration {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Duration::from_secs(24 * 60 * 60);
    }
    let (number, multiplier) = match trimmed.as_bytes().last().copied() {
        Some(b'm') => (&trimmed[..trimmed.len() - 1], 60),
        Some(b'h') => (&trimmed[..trimmed.len() - 1], 60 * 60),
        Some(b'd') => (&trimmed[..trimmed.len() - 1], 24 * 60 * 60),
        _ => (trimmed, 1),
    };
    let seconds = number
        .parse::<u64>()
        .ok()
        .and_then(|value| value.checked_mul(multiplier))
        .filter(|value| *value > 0)
        .unwrap_or(24 * 60 * 60);
    Duration::from_secs(seconds)
}

pub(super) fn update_command_hint() -> &'static str {
    if installed_by_homebrew() {
        "brew upgrade peridot"
    } else {
        "peri update"
    }
}

pub(super) fn installed_by_homebrew() -> bool {
    let Ok(path) = std::env::current_exe() else {
        return false;
    };
    let value = path.to_string_lossy();
    value.contains("/Cellar/peridot/") || value.contains("/homebrew/Cellar/peridot/")
}

pub(super) async fn install_update(release: &Value) -> Result<PathBuf> {
    let target = current_release_target()?;
    let asset_name = format!("peridot-{target}.tar.gz");
    let asset_url = release_asset_url(release, &asset_name)
        .with_context(|| format!("release asset not found: {asset_name}"))?;
    let checksum_url = release_asset_url(release, "SHA256SUMS")
        .with_context(|| "release asset not found: SHA256SUMS")?;
    let temp_dir = std::env::temp_dir().join(format!(
        "peridot-update-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
    ));
    fs::create_dir_all(&temp_dir)?;
    let archive_path = temp_dir.join(&asset_name);
    let client = reqwest::Client::new();
    let checksums = client
        .get(&checksum_url)
        .header("user-agent", "peridot-agent")
        .send()
        .await
        .with_context(|| format!("failed to download {checksum_url}"))?
        .error_for_status()
        .with_context(|| format!("failed to download {checksum_url}"))?
        .text()
        .await?;
    let expected_checksum = checksum_for_asset(&checksums, &asset_name)?;
    let bytes = client
        .get(&asset_url)
        .header("user-agent", "peridot-agent")
        .send()
        .await
        .with_context(|| format!("failed to download {asset_url}"))?
        .error_for_status()
        .with_context(|| format!("failed to download {asset_url}"))?
        .bytes()
        .await?;
    verify_sha256(bytes.as_ref(), &expected_checksum, &asset_name)?;
    fs::write(&archive_path, bytes)?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&temp_dir)
        .status()
        .with_context(|| "failed to run tar for update archive")?;
    if !status.success() {
        anyhow::bail!("tar failed while extracting update archive: {status}");
    }
    let binary_name = if target.contains("windows") {
        "peridot.exe"
    } else {
        "peridot"
    };
    let extracted = temp_dir.join(binary_name);
    if !extracted.exists() {
        anyhow::bail!("update archive did not contain {binary_name}");
    }
    let current_exe = std::env::current_exe()?;
    let backup = current_exe.with_file_name(format!(
        "{}.old",
        current_exe
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("peridot")
    ));
    let _ = fs::copy(&current_exe, backup);
    install_executable_update(&extracted, &current_exe)?;
    ensure_peri_alias(&current_exe, target)?;
    let _ = fs::remove_dir_all(temp_dir);
    Ok(current_exe)
}

#[cfg(unix)]
pub(super) fn install_executable_update(extracted: &Path, current_exe: &Path) -> Result<()> {
    let parent = current_exe
        .parent()
        .with_context(|| format!("{} has no parent directory", current_exe.display()))?;
    let file_name = current_exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("peridot");
    let staged = parent.join(format!(
        ".{file_name}.new-{}-{}",
        std::process::id(),
        unix_timestamp()
    ));
    let cleanup = || {
        let _ = fs::remove_file(&staged);
    };
    cleanup();
    fs::copy(extracted, &staged)
        .with_context(|| format!("failed to stage update at {}", staged.display()))?;
    set_executable_permissions(&staged)?;
    fs::rename(&staged, current_exe)
        .with_context(|| format!("failed to replace {}", current_exe.display()))?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn install_executable_update(extracted: &Path, current_exe: &Path) -> Result<()> {
    // On Windows a running executable is locked for writes but CAN be
    // renamed. Move it out of the way first so the target path is free.
    let old_path = current_exe.with_file_name(format!(
        "{}.old",
        current_exe
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("peridot")
    ));
    let _ = fs::remove_file(&old_path);
    let _ = fs::rename(current_exe, &old_path);
    fs::copy(extracted, current_exe)
        .with_context(|| format!("failed to replace {}", current_exe.display()))?;
    set_executable_permissions(current_exe)
}

pub(super) fn checksum_for_asset(checksums: &str, asset_name: &str) -> Result<String> {
    for line in checksums.lines() {
        let mut parts = line.split_whitespace();
        let Some(checksum) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        if name.trim_start_matches('*') != asset_name {
            continue;
        }
        if checksum.len() != 64
            || !checksum
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            anyhow::bail!("invalid SHA256 checksum for {asset_name}");
        }
        return Ok(checksum.to_ascii_lowercase());
    }
    anyhow::bail!("SHA256SUMS did not include {asset_name}")
}

pub(super) fn verify_sha256(bytes: &[u8], expected: &str, asset_name: &str) -> Result<()> {
    let actual = sha256_hex(bytes);
    if actual != expected.to_ascii_lowercase() {
        anyhow::bail!("SHA256 mismatch for {asset_name}: expected {expected}, got {actual}");
    }
    Ok(())
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn ensure_peri_alias(current_exe: &Path, target: &str) -> Result<PathBuf> {
    let alias_name = if target.contains("windows") {
        "peri.exe"
    } else {
        "peri"
    };
    let alias = current_exe.with_file_name(alias_name);
    if alias == current_exe {
        return Ok(alias);
    }
    install_alias(current_exe, &alias)
        .with_context(|| format!("failed to create peri alias at {}", alias.display()))?;
    Ok(alias)
}

#[cfg(unix)]
pub(super) fn install_alias(current_exe: &Path, alias: &Path) -> Result<()> {
    let _ = fs::remove_file(alias);
    std::os::unix::fs::symlink(current_exe, alias)?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn install_alias(current_exe: &Path, alias: &Path) -> Result<()> {
    let old = alias.with_file_name(format!(
        "{}.old",
        alias
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("peri")
    ));
    let _ = fs::remove_file(&old);
    let _ = fs::rename(alias, &old);
    fs::copy(current_exe, alias)?;
    Ok(())
}

pub(super) fn release_asset_url(release: &Value, asset_name: &str) -> Option<String> {
    release
        .get("assets")?
        .as_array()?
        .iter()
        .find(|asset| asset.get("name").and_then(Value::as_str) == Some(asset_name))?
        .get("browser_download_url")?
        .as_str()
        .map(str::to_string)
}

pub(super) fn current_release_target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => Ok("aarch64-pc-windows-msvc"),
        (os, arch) => anyhow::bail!("unsupported update target: {os}-{arch}"),
    }
}

pub(super) fn github_owner_repo(repository: &str) -> Option<(String, String)> {
    let trimmed = repository
        .trim()
        .trim_end_matches(".git")
        .trim_end_matches('/');
    let path = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("git@github.com:"))?;
    let mut parts = path.split('/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    Some((owner, repo))
}

#[cfg(unix)]
pub(super) fn set_executable_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn set_executable_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
