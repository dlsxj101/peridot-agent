use super::*;

pub(crate) async fn run_skill_command(
    command: &SkillCommand,
    project_root: &Path,
    output: OutputFormat,
    config: Option<&PeridotConfig>,
) -> Result<()> {
    match command {
        SkillCommand::List => {
            let skills = collect_skills(project_root)?;
            match output {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &skills
                            .iter()
                            .map(skill_json)
                            .collect::<Vec<serde_json::Value>>()
                    )?
                ),
                OutputFormat::Text => {
                    for skill in skills {
                        println!("{}\t{}\t{}", skill.name, skill.scope, skill.path.display());
                    }
                }
            }
        }
        SkillCommand::Install { source } => {
            let installed = install_skill(project_root, source).await?;
            print_json_or_text_result(
                serde_json::json!({
                    "installed": true,
                    "name": installed.name,
                    "path": installed.path
                }),
                format!(
                    "installed skill {} to {}",
                    installed.name,
                    installed.path.display()
                ),
                output,
            )?;
        }
        SkillCommand::Show { name } => {
            let skill = find_skill(project_root, name)?
                .with_context(|| format!("skill not found: {name}"))?;
            let content = fs::read_to_string(&skill.path)
                .with_context(|| format!("failed to read {}", skill.path.display()))?;
            match output {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "name": skill.name,
                        "scope": skill.scope,
                        "path": skill.path,
                        "content": content
                    }))?
                ),
                OutputFormat::Text => print!("{content}"),
            }
        }
        SkillCommand::Curate { dry_run, llm } => {
            let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
            let now = unix_timestamp();
            // 30/90-day automatic rules run first. When a row graduates
            // to Archive we also move its .md from auto/ into archive/.
            let rule_decisions = store
                .apply_auto_rules(now, *dry_run)
                .with_context(|| "failed to apply Curator auto-rules")?;
            if !*dry_run {
                for (name, verdict) in &rule_decisions {
                    if matches!(verdict, peridot_memory::AutoRuleVerdict::Archive) {
                        move_auto_skill_to_archive(project_root, name).with_context(|| {
                            format!("moving archived auto-skill file for {name}")
                        })?;
                    }
                }
            }

            // LLM reflection pass — opt-in, costs tokens. dry-run skips
            // it so operators can preview the cheap rule-only pass.
            let llm_report = if *llm && !*dry_run {
                let config = config.context(
                    "--llm requires a loaded peridot config; run from inside a peridot project",
                )?;
                let model = config.models.main.as_str();
                let provider = crate::providers::live_provider(config, model, project_root).await?;
                Some(
                    crate::curator::run_llm_curator(
                        provider.as_ref(),
                        model,
                        &store,
                        project_root,
                        now,
                    )
                    .await?,
                )
            } else {
                None
            };

            match output {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "dry_run": *dry_run,
                        "llm": *llm,
                        "rule_decisions": rule_decisions
                            .iter()
                            .map(|(name, verdict)| serde_json::json!({
                                "name": name,
                                "verdict": format!("{verdict:?}").to_lowercase(),
                            }))
                            .collect::<Vec<_>>(),
                        "llm_report": llm_report.as_ref().map(|report| serde_json::json!({
                            "evaluated": report.evaluated,
                            "applied": report.applied
                                .iter()
                                .map(|(name, action)| serde_json::json!({"name": name, "action": action}))
                                .collect::<Vec<_>>(),
                            "ignored": report.ignored,
                        })),
                    }))?
                ),
                OutputFormat::Text => {
                    if rule_decisions.is_empty() {
                        println!("no auto-skills to curate");
                    }
                    for (name, verdict) in &rule_decisions {
                        println!("{:<8}\t{name}", format!("{verdict:?}").to_lowercase());
                    }
                    if let Some(report) = &llm_report {
                        println!(
                            "\nLLM curator: evaluated {}, applied {}, ignored {}",
                            report.evaluated.len(),
                            report.applied.len(),
                            report.ignored.len(),
                        );
                        for (name, action) in &report.applied {
                            println!("{action:<14}\t{name}");
                        }
                    }
                    if *dry_run {
                        println!("(dry run — no writes)");
                    }
                }
            }
        }
        SkillCommand::Remove { name } => {
            let skill = find_skill(project_root, name)?
                .with_context(|| format!("skill not found: {name}"))?;
            let project_skills = project_root.join(".peridot/skills");
            if !skill.path.starts_with(&project_skills) {
                anyhow::bail!(
                    "refusing to remove non-project skill {} ({})",
                    skill.name,
                    skill.path.display()
                );
            }
            fs::remove_file(&skill.path)
                .with_context(|| format!("failed to remove {}", skill.path.display()))?;
            print_json_or_text_result(
                serde_json::json!({
                    "removed": true,
                    "name": skill.name,
                    "path": skill.path
                }),
                format!("removed skill {name}"),
                output,
            )?;
        }
    }
    Ok(())
}

pub(super) async fn install_skill(project_root: &Path, source: &str) -> Result<SkillEntry> {
    let content = read_skill_source(source).await?;
    if content.trim().is_empty() {
        anyhow::bail!("skill source is empty: {source}");
    }
    let name = skill_name_from_source(source);
    let target_dir = project_root.join(".peridot/skills/community");
    fs::create_dir_all(&target_dir)?;
    let path = target_dir.join(format!("{name}.md"));
    fs::write(&path, content)?;
    Ok(SkillEntry {
        name,
        scope: "project-community",
        path,
    })
}

pub(super) async fn read_skill_source(source: &str) -> Result<String> {
    if source.starts_with("https://") || source.starts_with("http://") {
        let response = reqwest::Client::new()
            .get(source)
            .header("user-agent", "peridot-agent")
            .send()
            .await
            .with_context(|| format!("failed to download skill {source}"))?;
        let status = response.status();
        let content = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("skill download returned {status}: {content}");
        }
        Ok(content)
    } else {
        fs::read_to_string(source).with_context(|| format!("failed to read skill {source}"))
    }
}

pub(super) fn skill_name_from_source(source: &str) -> String {
    let source = source.trim_end_matches('/');
    let last = source.rsplit('/').next().unwrap_or(source);
    let stem = last
        .strip_suffix(".md")
        .or_else(|| last.strip_suffix(".markdown"))
        .unwrap_or(last);
    sanitize_skill_name(stem)
}

pub(super) fn sanitize_skill_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "skill".to_string()
    } else {
        sanitized
    }
}

pub(super) fn collect_skills(project_root: &Path) -> Result<Vec<SkillEntry>> {
    let mut skills = Vec::new();
    collect_skill_dir(
        &project_root.join(".peridot/skills"),
        "project",
        false,
        &mut skills,
    )?;
    collect_skill_dir(
        &project_root.join(".peridot/skills/community"),
        "project-community",
        true,
        &mut skills,
    )?;
    collect_skill_dir(
        &project_root.join(".peridot/skills/auto"),
        "project-auto",
        true,
        &mut skills,
    )?;
    if let Some(home) = std::env::var_os("HOME") {
        let global = PathBuf::from(home).join(".peridot/skills");
        collect_skill_dir(&global, "global", false, &mut skills)?;
        collect_skill_dir(&global.join("community"), "community", true, &mut skills)?;
    }
    skills.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.scope.cmp(right.scope))
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(skills)
}

pub(super) fn collect_skill_dir(
    root: &Path,
    scope: &'static str,
    recursive: bool,
    skills: &mut Vec<SkillEntry>,
) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            let skill_path = path.join("SKILL.md");
            if skill_path.is_file()
                && let Some(name) = path.file_name().and_then(|name| name.to_str())
            {
                skills.push(SkillEntry {
                    name: name.to_string(),
                    scope,
                    path: skill_path,
                });
            }
            if recursive {
                collect_skill_dir(&path, scope, recursive, skills)?;
            }
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        skills.push(SkillEntry {
            name: name.to_string(),
            scope,
            path,
        });
    }
    Ok(())
}

pub(super) fn find_skill(project_root: &Path, name: &str) -> Result<Option<SkillEntry>> {
    Ok(collect_skills(project_root)?.into_iter().find(|skill| {
        skill.name == name || skill.path.file_stem().and_then(|stem| stem.to_str()) == Some(name)
    }))
}

/// Renames `.peridot/skills/auto/<name>.md` to
/// `.peridot/skills/archive/<name>.md` so the on-disk catalog matches
/// the DB's `archived_at_unix` stamp. A missing source file is fine;
/// only fs errors during create_dir_all / rename surface as failures.
pub(crate) fn move_auto_skill_to_archive(project_root: &Path, name: &str) -> Result<()> {
    let source = project_root
        .join(".peridot/skills/auto")
        .join(format!("{name}.md"));
    if !source.exists() {
        return Ok(());
    }
    let archive_dir = project_root.join(".peridot/skills/archive");
    fs::create_dir_all(&archive_dir)
        .with_context(|| format!("failed to create {}", archive_dir.display()))?;
    let target = archive_dir.join(format!("{name}.md"));
    fs::rename(&source, &target)
        .with_context(|| format!("rename {} -> {}", source.display(), target.display()))?;
    Ok(())
}

pub(super) fn skill_json(skill: &SkillEntry) -> serde_json::Value {
    serde_json::json!({
        "name": skill.name,
        "scope": skill.scope,
        "path": skill.path
    })
}
