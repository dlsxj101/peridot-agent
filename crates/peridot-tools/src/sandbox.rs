//! OS-level filesystem sandbox for model-generated shell/tool commands.
//!
//! # Threat model / scope boundary
//!
//! This sandbox isolates **model-generated commands only**: `shell_exec`,
//! `shell_readonly`, `verify_*`, git reads routed through
//! `run_read_only_command`, and git writes / `gh` tools routed through
//! `run_binary`. Everything here treats the model as the untrusted party.
//!
//! The following are deliberately **out of scope** and are NOT sandboxed by
//! this module:
//! * operator-authored hook runners (`hooks.rs`),
//! * operator-configured MCP stdio servers,
//! * interactive CLI commands (`ship` / `auth` / `update`).
//!
//! These are code the operator wrote or explicitly configured, i.e. trusted
//! input, so wrapping them would only break legitimate workflows without
//! improving the security posture against a misbehaving model.
//!
//! # Phase 1: filesystem only
//!
//! The policy restricts **writes** to a set of `writable_roots` (the project
//! root, the system temp dir, and toolchain caches) while leaving **reads**
//! fully unrestricted. Network isolation is explicitly out of scope for this
//! phase; `SandboxMode::Os` does not (yet) contain the command's network
//! access.
//!
//! # Platform backends
//!
//! * **Linux** — Landlock LSM. A ruleset is built in the parent process and
//!   `restrict_self()` is invoked from a `pre_exec` hook in the forked child.
//!   The `pre_exec` closure performs only syscalls (no allocation) to stay
//!   async-signal-safe.
//! * **macOS** — `sandbox-exec` wraps the command with a generated SBPL
//!   profile that denies `file-write*` outside the writable roots.
//! * **other / unsupported** — behaves like `SandboxMode::None` and emits a
//!   single process-lifetime warning.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Filesystem sandbox policy: reads are unrestricted, writes are confined to
/// `writable_roots`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SandboxPolicy {
    /// Directories under which the sandboxed command may create, modify, or
    /// delete files. Everything else on the filesystem is read-only.
    pub writable_roots: Vec<PathBuf>,
}

impl SandboxPolicy {
    /// Resolves the effective writable-root set for `project_root`, folding in
    /// the operator-configured `extra_allow` paths (from
    /// `security.sandbox_allow_write`).
    pub(crate) fn resolve(project_root: &Path, extra_allow: &[String]) -> Self {
        SandboxPolicy {
            writable_roots: default_writable_roots(project_root, extra_allow),
        }
    }
}

/// Best-effort home directory lookup without pulling in a dependency.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Expands a leading `~/` (or bare `~`) in `raw` to the home directory.
fn expand_tilde(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if trimmed == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(trimmed));
    }
    if let Some(rest) = trimmed.strip_prefix("~/")
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(trimmed)
}

/// Builds the default writable-root set:
/// * the project root and the system temp dir (always),
/// * toolchain caches under `$HOME` that actually exist (`~/.cache`,
///   `~/.cargo`, `~/.rustup`, `~/.npm`, `~/.config/gh`), so first-run
///   cargo/npm builds and `gh` token refreshes don't break,
/// * any operator-configured `extra_allow` paths (tilde-expanded).
///
/// The list is de-duplicated but order (project root first) is preserved.
pub(crate) fn default_writable_roots(project_root: &Path, extra_allow: &[String]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let push = |p: PathBuf, roots: &mut Vec<PathBuf>| {
        if !roots.contains(&p) {
            roots.push(p);
        }
    };
    push(project_root.to_path_buf(), &mut roots);
    push(std::env::temp_dir(), &mut roots);
    if let Some(home) = home_dir() {
        // Toolchain / tooling caches: only include when present so we never
        // hand out write access to a directory that doesn't exist yet.
        for sub in [".cache", ".cargo", ".rustup", ".npm", ".config/gh"] {
            let candidate = home.join(sub);
            if candidate.exists() {
                push(candidate, &mut roots);
            }
        }
    }
    for extra in extra_allow {
        if !extra.trim().is_empty() {
            push(expand_tilde(extra), &mut roots);
        }
    }
    roots
}

/// Escapes a path for embedding inside a double-quoted SBPL string literal
/// (macOS `sandbox-exec` profile). Backslashes and double quotes are escaped.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn escape_sbpl(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Generates a macOS `sandbox-exec` SBPL profile that allows everything by
/// default, denies all file writes, then re-grants `file-write*` beneath each
/// writable root. Kept as a pure function so it can be unit-tested on any
/// platform.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn macos_sandbox_profile(policy: &SandboxPolicy) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
    for root in &policy.writable_roots {
        let path = escape_sbpl(&root.display().to_string());
        profile.push_str(&format!("(allow file-write* (subpath \"{path}\"))\n"));
    }
    profile
}

/// Emits a single process-lifetime warning when `SandboxMode::Os` is selected
/// but no native backend is available (unsupported platform or a Linux kernel
/// without Landlock). The command still runs — unsandboxed — matching
/// `SandboxMode::None`.
pub(crate) fn warn_os_sandbox_unavailable_once(reason: &str) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "warning: os sandbox unavailable on this platform; running unsandboxed ({reason}). \
             Commands run directly on the host with the agent's privileges; the deterministic \
             command checks are best-effort defense-in-depth, not a containment boundary."
        );
    });
}

/// Builds a `Command` that runs `program` with `args` from `cwd` under the OS
/// filesystem sandbox described by `policy`.
///
/// This is the single reuse point for both the shell chokepoint
/// (`sh -c <command>`) and the `run_binary` path (`git` / `gh`). The env
/// scrub (provider credentials) is applied by the shell chokepoint, not here,
/// so `run_binary` keeps `GH_TOKEN` etc.
pub(crate) fn sandboxed_command(
    program: &str,
    args: &[&str],
    cwd: &Path,
    policy: &SandboxPolicy,
) -> Command {
    #[cfg(target_os = "linux")]
    {
        let mut command = Command::new(program);
        command.args(args).current_dir(cwd);
        linux::install_landlock(&mut command, policy);
        command
    }
    #[cfg(target_os = "macos")]
    {
        let profile = macos_sandbox_profile(policy);
        let mut command = Command::new("sandbox-exec");
        command.arg("-p").arg(profile).arg(program).args(args);
        command.current_dir(cwd);
        command
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = policy;
        warn_os_sandbox_unavailable_once("no native sandbox backend for this platform");
        let mut command = Command::new(program);
        command.args(args).current_dir(cwd);
        command
    }
}

/// Introspection helper (used by tests and behavioural checks): does the
/// current platform actually *enforce* the OS sandbox, as opposed to falling
/// back to unsandboxed execution?
#[cfg(all(target_os = "linux", test))]
pub(crate) fn os_sandbox_enforces() -> bool {
    linux::landlock_available()
}

#[cfg(target_os = "linux")]
mod linux {
    use super::SandboxPolicy;
    use landlock::{
        ABI, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr, RulesetCreated,
        RulesetCreatedAttr, path_beneath_rules,
    };
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::sync::OnceLock;

    /// Target Landlock ABI. `from_write(ABI::V3)` covers every filesystem
    /// *mutation* access right (create / write / remove / rename / truncate)
    /// without pulling in the ABI v5 `IoctlDev` restriction, which would deny
    /// benign device ioctls (e.g. `isatty` on `/dev/null`) and risk breaking
    /// otherwise read-only tooling. Best-effort negotiation downgrades this on
    /// older kernels automatically.
    const TARGET_ABI: ABI = ABI::V3;

    /// One-time probe: is Landlock actually usable on the running kernel?
    /// Uses `HardRequirement` so `create()` returns an error (rather than a
    /// silent no-op ruleset) when the kernel lacks Landlock support. This only
    /// creates a ruleset fd; it never calls `restrict_self`, so the probing
    /// thread is not itself restricted.
    pub(super) fn landlock_available() -> bool {
        static AVAILABLE: OnceLock<bool> = OnceLock::new();
        *AVAILABLE.get_or_init(|| {
            Ruleset::default()
                .set_compatibility(CompatLevel::HardRequirement)
                .handle_access(AccessFs::from_write(ABI::V1))
                .and_then(|r| r.create())
                .is_ok()
        })
    }

    /// Builds a `RulesetCreated` (in the parent process) that handles all
    /// write accesses and grants them beneath each writable root. Reads are
    /// left unhandled, so they remain unrestricted. Non-existent writable
    /// roots are silently skipped by `path_beneath_rules`.
    fn build_ruleset(policy: &SandboxPolicy) -> Result<RulesetCreated, landlock::RulesetError> {
        let write = AccessFs::from_write(TARGET_ABI);
        let created = Ruleset::default()
            .handle_access(write)?
            .create()?
            .add_rules(path_beneath_rules(&policy.writable_roots, write))?;
        Ok(created)
    }

    /// Installs a Landlock restriction on `command` via `pre_exec`. The
    /// ruleset (including the open directory fds) is built here, in the
    /// parent; the `pre_exec` closure only calls `restrict_self()`, which
    /// performs syscalls without heap allocation and is therefore safe to run
    /// after `fork()`.
    pub(super) fn install_landlock(command: &mut Command, policy: &SandboxPolicy) {
        if !landlock_available() {
            super::warn_os_sandbox_unavailable_once(
                "running kernel does not support Landlock (ENOSYS/EOPNOTSUPP)",
            );
            return;
        }
        let ruleset = match build_ruleset(policy) {
            Ok(ruleset) => ruleset,
            Err(err) => {
                super::warn_os_sandbox_unavailable_once(&format!(
                    "failed to build Landlock ruleset: {err}"
                ));
                return;
            }
        };
        let mut ruleset = Some(ruleset);
        // SAFETY: the closure runs in the forked child before exec and only
        // issues the prctl / landlock_restrict_self syscalls (no allocation,
        // no locks), which is async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                // The ruleset is consumed on first use. A second spawn of the
                // same Command would otherwise run silently UNSANDBOXED —
                // fail loudly instead so a future refactor that re-spawns a
                // prepared Command cannot become a sandbox bypass.
                let Some(ruleset) = ruleset.take() else {
                    return Err(std::io::Error::other(
                        "landlock: prepared command spawned twice; ruleset already consumed",
                    ));
                };
                ruleset
                    .restrict_self()
                    .map_err(|err| std::io::Error::other(format!("landlock: {err}")))?;
                Ok(())
            });
        }
    }
}
