//! Harness self-tuning pass.
//!
//! The harness watches what the operator actually does across recent
//! sessions and, if a clear behaviour signal emerges, auto-flips a
//! default in `.peridot/config.toml` so the next session "just works"
//! without the operator having to learn the toggle existed. Each field
//! is auto-adjusted at most once across the project's lifetime — once
//! the harness has spoken, the operator owns it.
//!
//! Today the pass watches two fields:
//!
//! - `git.auto_commit` — flipped to `true` when the operator manually
//!   ran `git_commit` in ≥ 50% of recent sessions.
//! - `git.auto_branch` — flipped to `true` when the operator ran
//!   `git_branch` (or any other branch-creating tool) in ≥ 50% of
//!   recent sessions.
//!
//! The pass is invoked from the 7-day idle Curator trigger so it never
//! delays an active session, and every change is logged as an
//! `AuditEvent` so the operator can see why the toggle moved.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use peridot_common::PeridotConfig;
use peridot_memory::MemoryStore;
use peridot_tools::audit::{AuditEvent, append_audit_event};

use crate::commands::set_config_key;

/// Minimum recent-session sample before any auto-adjustment fires.
/// Below this the signal is just noise.
const MIN_SAMPLE_SIZE: usize = 5;

/// Maximum recent sessions inspected per pass. Keeps the SQLite read
/// bounded and biases the signal toward fresh behaviour.
const SAMPLE_WINDOW: usize = 30;

/// Window age cap. Sessions older than this are ignored even if they're
/// inside `SAMPLE_WINDOW` by count — keeps stale workflows from
/// triggering adjustments on long-idle projects.
const SAMPLE_AGE_DAYS: u64 = 60;

/// Threshold (fraction of sampled sessions) above which a tool's
/// presence justifies flipping the matching default to true.
const ADJUSTMENT_THRESHOLD: f64 = 0.5;

/// One auto-tune decision the pass is about to apply.
#[derive(Debug, Clone)]
pub struct ProposedAdjustment {
    /// Dot-path config key (matches `peridot config set`).
    pub field: &'static str,
    /// Stringified previous value (for audit logging).
    pub previous_value: String,
    /// Stringified new value (for audit logging and write).
    pub new_value: String,
    /// Human-readable signal description ("git_commit in 7/12 sessions").
    pub signal: String,
}

/// Result of one pass.
#[derive(Debug, Default)]
pub struct HarnessLearnReport {
    /// Adjustments that were applied to `config.toml`.
    pub applied: Vec<ProposedAdjustment>,
    /// Adjustments considered but skipped (already adjusted, sample
    /// too small, signal too weak).
    pub skipped: Vec<String>,
}

/// Inspects recent session behaviour and decides which (if any) config
/// defaults the harness should flip. Pure function — does not touch
/// the filesystem. Caller drives `apply_adjustment` on each item.
pub fn propose_adjustments(
    store: &MemoryStore,
    config: &PeridotConfig,
    now_unix: u64,
) -> Result<Vec<ProposedAdjustment>> {
    let since = now_unix.saturating_sub(SAMPLE_AGE_DAYS * 24 * 3600);
    let sessions = store
        .recent_tool_sequences(SAMPLE_WINDOW, since)
        .with_context(|| "reading recent_tool_sequences")?;
    if sessions.len() < MIN_SAMPLE_SIZE {
        return Ok(Vec::new());
    }
    let total = sessions.len() as f64;
    let mut proposed = Vec::new();

    // git.auto_commit
    if !config.git.auto_commit
        && !store
            .was_field_auto_adjusted("git.auto_commit")
            .unwrap_or(false)
    {
        let sessions_with_commit = sessions
            .iter()
            .filter(|seq| seq.iter().any(|tool| tool == "git_commit"))
            .count();
        let rate = sessions_with_commit as f64 / total;
        if rate >= ADJUSTMENT_THRESHOLD {
            proposed.push(ProposedAdjustment {
                field: "git.auto_commit",
                previous_value: "false".to_string(),
                new_value: "true".to_string(),
                signal: format!(
                    "git_commit observed in {sessions_with_commit}/{} recent sessions",
                    sessions.len()
                ),
            });
        }
    }

    // git.auto_branch
    if !config.git.auto_branch
        && !store
            .was_field_auto_adjusted("git.auto_branch")
            .unwrap_or(false)
    {
        let sessions_with_branch = sessions
            .iter()
            .filter(|seq| seq.iter().any(|tool| tool == "git_branch"))
            .count();
        let rate = sessions_with_branch as f64 / total;
        if rate >= ADJUSTMENT_THRESHOLD {
            proposed.push(ProposedAdjustment {
                field: "git.auto_branch",
                previous_value: "false".to_string(),
                new_value: "true".to_string(),
                signal: format!(
                    "git_branch observed in {sessions_with_branch}/{} recent sessions",
                    sessions.len()
                ),
            });
        }
    }

    Ok(proposed)
}

/// Applies one adjustment: rewrites `.peridot/config.toml` with the
/// new value, stamps the `harness_adjustments` table so the same field
/// is never re-tuned, and writes an audit log entry the operator can
/// review later.
pub fn apply_adjustment(
    config_path: &Path,
    store: &MemoryStore,
    project_root: &Path,
    adjustment: &ProposedAdjustment,
    now_unix: u64,
) -> Result<()> {
    let mut config: PeridotConfig = toml::from_str(
        &fs::read_to_string(config_path)
            .with_context(|| format!("reading {}", config_path.display()))?,
    )
    .with_context(|| format!("parsing {}", config_path.display()))?;
    set_config_key(&mut config, adjustment.field, &adjustment.new_value)
        .with_context(|| format!("set_config_key({})", adjustment.field))?;
    fs::write(config_path, toml::to_string_pretty(&config)?)
        .with_context(|| format!("writing {}", config_path.display()))?;
    store
        .record_harness_adjustment(
            adjustment.field,
            &adjustment.previous_value,
            &adjustment.new_value,
            &adjustment.signal,
            now_unix,
        )
        .with_context(|| "recording harness adjustment")?;
    let _ = append_audit_event(
        project_root,
        &AuditEvent::tool_call(
            "harness_learn",
            true,
            format!(
                "auto-adjusted {} from {} to {} ({})",
                adjustment.field,
                adjustment.previous_value,
                adjustment.new_value,
                adjustment.signal
            ),
            serde_json::json!({
                "field": adjustment.field,
                "previous_value": adjustment.previous_value,
                "new_value": adjustment.new_value,
                "signal": adjustment.signal,
            }),
        ),
    );
    Ok(())
}

/// Runs the full propose-and-apply pass. Best-effort: any per-field
/// failure is captured in `skipped`, never raised, so the idle Curator
/// thread is never blocked by a config tuning hiccup.
pub fn run_pass(
    store: &MemoryStore,
    config: &PeridotConfig,
    config_path: &Path,
    project_root: &Path,
    now_unix: u64,
) -> HarnessLearnReport {
    let proposals = match propose_adjustments(store, config, now_unix) {
        Ok(list) => list,
        Err(err) => {
            return HarnessLearnReport {
                applied: Vec::new(),
                skipped: vec![format!("propose failed: {err}")],
            };
        }
    };
    let mut report = HarnessLearnReport::default();
    for adjustment in proposals {
        match apply_adjustment(config_path, store, project_root, &adjustment, now_unix) {
            Ok(()) => report.applied.push(adjustment),
            Err(err) => report
                .skipped
                .push(format!("apply {}: {err}", adjustment.field)),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use peridot_common::PeridotConfig;

    fn fresh_store(label: &str) -> (std::path::PathBuf, MemoryStore) {
        let root = std::env::temp_dir().join(format!(
            "peridot-harness-learn-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = MemoryStore::new(root.join("memory.db"));
        store.initialize().unwrap();
        (root, store)
    }

    fn seed_sessions(store: &MemoryStore, sequences: &[&[&str]], now: u64) {
        for (i, seq) in sequences.iter().enumerate() {
            let tools: Vec<String> = seq.iter().map(|s| s.to_string()).collect();
            store
                .save_tool_sequence(&format!("sess-{i}"), &tools, "auto-test", 3, now - i as u64)
                .unwrap();
        }
    }

    #[test]
    fn proposes_auto_commit_when_majority_used_git_commit() {
        let (root, store) = fresh_store("commit");
        let now = 1_700_000_000;
        // 7 of 10 sessions invoked git_commit → 70% > 50% threshold.
        let with = &["file_write", "git_commit"];
        let without = &["file_read", "file_write"];
        seed_sessions(
            &store,
            &[
                with, with, with, with, with, with, with, without, without, without,
            ],
            now,
        );

        let config = PeridotConfig::default();
        let proposals = propose_adjustments(&store, &config, now).unwrap();
        assert!(proposals.iter().any(|p| p.field == "git.auto_commit"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_when_sample_size_too_small() {
        let (root, store) = fresh_store("small");
        let now = 1_700_000_000;
        // Only 3 sessions, all using git_commit — sample below
        // MIN_SAMPLE_SIZE so no proposal fires.
        let with = &["file_write", "git_commit"];
        seed_sessions(&store, &[with, with, with], now);

        let config = PeridotConfig::default();
        let proposals = propose_adjustments(&store, &config, now).unwrap();
        assert!(proposals.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_when_signal_below_threshold() {
        let (root, store) = fresh_store("weak");
        let now = 1_700_000_000;
        // 2 of 10 sessions used git_commit → 20% < 50%.
        let with = &["file_write", "git_commit"];
        let without = &["file_read", "file_write"];
        seed_sessions(
            &store,
            &[
                with, with, without, without, without, without, without, without, without, without,
            ],
            now,
        );

        let config = PeridotConfig::default();
        let proposals = propose_adjustments(&store, &config, now).unwrap();
        assert!(proposals.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_when_already_adjusted() {
        let (root, store) = fresh_store("already");
        let now = 1_700_000_000;
        let with = &["file_write", "git_commit"];
        seed_sessions(&store, &[with, with, with, with, with, with], now);

        // Pre-stamp the adjustment row so the next propose pass skips.
        store
            .record_harness_adjustment("git.auto_commit", "false", "true", "manual", now - 100)
            .unwrap();
        let config = PeridotConfig::default();
        let proposals = propose_adjustments(&store, &config, now).unwrap();
        assert!(
            proposals.iter().all(|p| p.field != "git.auto_commit"),
            "already-adjusted field must not be re-proposed: {proposals:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_when_user_explicitly_turned_field_on() {
        let (root, store) = fresh_store("user_on");
        let now = 1_700_000_000;
        // 100% of sessions used git_commit.
        let with = &["file_write", "git_commit"];
        seed_sessions(&store, &[with, with, with, with, with, with], now);

        // Operator already set the field to true — no proposal needed.
        let mut config = PeridotConfig::default();
        config.git.auto_commit = true;
        let proposals = propose_adjustments(&store, &config, now).unwrap();
        assert!(
            proposals.iter().all(|p| p.field != "git.auto_commit"),
            "field already true: {proposals:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn apply_adjustment_writes_config_and_audit() {
        let (root, store) = fresh_store("apply");
        let now = 1_700_000_000;
        let config_path = root.join("config.toml");
        std::fs::write(
            &config_path,
            toml::to_string_pretty(&PeridotConfig::default()).unwrap(),
        )
        .unwrap();

        let adjustment = ProposedAdjustment {
            field: "git.auto_commit",
            previous_value: "false".to_string(),
            new_value: "true".to_string(),
            signal: "git_commit in 6/10 sessions".to_string(),
        };
        apply_adjustment(&config_path, &store, &root, &adjustment, now).unwrap();

        // Config file now contains auto_commit = true.
        let new_text = std::fs::read_to_string(&config_path).unwrap();
        let new_config: PeridotConfig = toml::from_str(&new_text).unwrap();
        assert!(new_config.git.auto_commit);
        // Adjustments table remembers the field so future passes skip.
        assert!(store.was_field_auto_adjusted("git.auto_commit").unwrap());
        // Audit log records the change.
        let audit_path = root.join(".peridot/logs/audit.jsonl");
        assert!(audit_path.exists());
        let audit = std::fs::read_to_string(&audit_path).unwrap();
        assert!(audit.contains("auto-adjusted git.auto_commit"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
