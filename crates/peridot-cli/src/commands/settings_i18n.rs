//! Translation table for the curated settings registry.
//!
//! Layout intent: one match arm per setting id, with both `en` and `ko`
//! strings co-located. Translators can scan side-by-side without
//! jumping between files, and adding a third locale (`ja`, `de`, …)
//! later means extending [`BilingualText`] with a new field plus one
//! pick branch — not editing every arm.
//!
//! Settings ids whose translation is missing here fall through to
//! [`fallback`]: the registry can then ship a sensible English default
//! rather than failing to render the row, so introducing a new field
//! never breaks the page while a Korean string is still in flight.
//!
//! NOTE: when adding a new setting in `settings_registry`, add the
//! matching translation entry here. `cargo test
//! settings_i18n_covers_registry` enforces this.
//!
//! Why one big match instead of `phf::Map`: the table is ~20 items
//! deep, fits in a screen, and avoids pulling in a perfect-hash crate.
//! Lookup is O(n) but n is small and called only when rendering the
//! settings page (cold path).

use peridot_common::Locale;

/// Resolved strings for a single settings row in one locale.
#[derive(Clone, Copy, Debug)]
pub(super) struct LocalizedSetting {
    pub group: &'static str,
    pub label: &'static str,
    pub help: Option<&'static str>,
}

/// A label/help bundle that carries both languages, picked at render
/// time. Adding `ja: &'static str` (or similar) here is the one-line
/// change needed to support another locale.
#[derive(Clone, Copy, Debug)]
struct Bilingual {
    group: BilingualText,
    label: BilingualText,
    help: Option<BilingualText>,
}

#[derive(Clone, Copy, Debug)]
struct BilingualText {
    en: &'static str,
    ko: &'static str,
}

impl BilingualText {
    const fn new(en: &'static str, ko: &'static str) -> Self {
        Self { en, ko }
    }
    fn pick(&self, locale: Locale) -> &'static str {
        match locale {
            Locale::En => self.en,
            Locale::Ko => self.ko,
        }
    }
}

impl Bilingual {
    fn pick(&self, locale: Locale) -> LocalizedSetting {
        LocalizedSetting {
            group: self.group.pick(locale),
            label: self.label.pick(locale),
            help: self.help.map(|h| h.pick(locale)),
        }
    }
}

/// Look up the translation for a setting id. Returns `None` when the
/// id isn't in the table — callers should fall back to a hardcoded
/// English default so a missing entry never breaks rendering. See
/// [`settings_i18n_covers_registry`] for the test that prevents drift.
pub(super) fn lookup(id: &str, locale: Locale) -> Option<LocalizedSetting> {
    translations(id).map(|b| b.pick(locale))
}

fn translations(id: &str) -> Option<Bilingual> {
    Some(match id {
        // ===== Autonomy =====
        "defaults.auto_verify_after_mutation" => Bilingual {
            group: BilingualText::new("Autonomy", "자동화"),
            label: BilingualText::new("Auto-verify after file changes", "파일 변경 후 자동 검증"),
            help: Some(BilingualText::new(
                "After a burst of file_write / file_patch edits settles, run verify_build once and block agent_done until it passes. The command comes from AGENTS.md `## commands` or project detection.",
                "file_write / file_patch 편집 묶음이 끝나면 verify_build를 한 번 실행하고, 통과할 때까지 agent_done을 막습니다. 명령은 AGENTS.md `## commands` 또는 프로젝트 감지에서 가져옵니다.",
            )),
        },
        "auto_fix.enabled" => Bilingual {
            group: BilingualText::new("Autonomy", "자동화"),
            label: BilingualText::new("Auto-fix circuit breaker", "자동 수정 서킷 브레이커"),
            help: Some(BilingualText::new(
                "Inject a fix directive on verify failures and abort after the same failure repeats max_attempts times. Turn off to fail hard instead.",
                "검증 실패 시 수정 지시를 주입하고, 동일 실패가 max_attempts회 반복되면 실행을 중단합니다. 끄면 즉시 하드 실패합니다.",
            )),
        },
        "auto_fix.commands" => Bilingual {
            group: BilingualText::new("Autonomy", "자동화"),
            label: BilingualText::new("Auto-fix verify commands", "자동 수정 검증 명령"),
            help: Some(BilingualText::new(
                "Explicit verification commands run by auto-verify, joined with ` && `. Overrides AGENTS.md and project detection. Empty = auto-detect.",
                "auto-verify가 실행할 명시적 검증 명령으로, ` && `로 연결됩니다. AGENTS.md와 프로젝트 감지를 무시합니다. 비우면 자동 감지합니다.",
            )),
        },
        "defaults.auto_grade_on_done" => Bilingual {
            group: BilingualText::new("Autonomy", "자동화"),
            label: BilingualText::new("Auto-grade on agent_done", "agent_done 시 자동 채점"),
            help: Some(BilingualText::new(
                "Before declaring the task done, ask an LLM to grade the change. If it fails, the loop continues with the recommendations injected.",
                "작업 완료 선언 전 LLM에게 변경 사항을 채점받습니다. 불합격이면 권장 사항을 주입하고 루프를 계속합니다.",
            )),
        },

        // ===== Defaults =====
        "defaults.mode" => Bilingual {
            group: BilingualText::new("Defaults", "기본 실행"),
            label: BilingualText::new("Default execution mode", "기본 실행 모드"),
            help: Some(BilingualText::new(
                "Plan = read-only planning; Execute = normal coding; Goal = long autonomous run",
                "Plan = 읽기 전용 계획; Execute = 일반 코딩; Goal = 긴 자율 실행",
            )),
        },
        "defaults.permission" => Bilingual {
            group: BilingualText::new("Defaults", "기본 실행"),
            label: BilingualText::new("Default permission posture", "기본 권한 정책"),
            help: Some(BilingualText::new(
                "Safe = confirm every write; Auto = confirm only destructive; Yolo = no prompts",
                "Safe = 모든 쓰기 확인; Auto = 파괴적 작업만 확인; Yolo = 확인 없음",
            )),
        },
        "defaults.max_turns" => Bilingual {
            group: BilingualText::new("Defaults", "기본 실행"),
            label: BilingualText::new("Max turns per run", "실행당 최대 턴 수"),
            help: Some(BilingualText::new(
                "Hard cap on how many model→tool cycles a single task can take.",
                "한 작업이 사용할 수 있는 모델→도구 사이클의 절대 상한입니다.",
            )),
        },
        "defaults.budget_usd" => Bilingual {
            group: BilingualText::new("Defaults", "기본 실행"),
            label: BilingualText::new("Budget per run (USD)", "실행당 예산 (USD)"),
            help: Some(BilingualText::new(
                "Cost ceiling. 0 disables the cap.",
                "비용 상한선. 0이면 상한 비활성화.",
            )),
        },
        "defaults.budget_warning_pct" => Bilingual {
            group: BilingualText::new("Defaults", "기본 실행"),
            label: BilingualText::new("Budget warning at (%)", "예산 경고 임계치 (%)"),
            help: Some(BilingualText::new(
                "Fire a hook when this share of the budget is consumed.",
                "이 비율의 예산이 소비되면 훅을 발화합니다.",
            )),
        },

        // ===== Committee =====
        "committee.mode" => Bilingual {
            group: BilingualText::new("Committee", "커미티"),
            label: BilingualText::new("Multi-agent committee", "멀티 에이전트 커미티"),
            help: Some(BilingualText::new(
                "Off = single agent; Planner = run a planner preflight; Full = planner + reviewer per mutating turn.",
                "Off = 단일 에이전트; Planner = 플래너 사전 점검 실행; Full = 변경 턴마다 플래너 + 리뷰어.",
            )),
        },
        "committee.min_task_chars" => Bilingual {
            group: BilingualText::new("Committee", "커미티"),
            label: BilingualText::new(
                "Skip planner below N chars",
                "N자 미만 작업은 플래너 건너뜀",
            ),
            help: Some(BilingualText::new(
                "Tasks shorter than this skip the planner preflight. 0 = always run.",
                "이 글자 수 미만 작업은 플래너 사전 점검을 건너뜁니다. 0이면 항상 실행.",
            )),
        },
        "committee.max_review_passes" => Bilingual {
            group: BilingualText::new("Committee", "커미티"),
            label: BilingualText::new("Max reviewer re-passes", "리뷰어 재검 최대 횟수"),
            help: Some(BilingualText::new(
                "After this many consecutive RequestChanges, auto-block the run.",
                "이 횟수만큼 연속 RequestChanges 발생 시 실행을 자동 차단합니다.",
            )),
        },
        "committee.use_llm_complexity_gate" => Bilingual {
            group: BilingualText::new("Committee", "커미티"),
            label: BilingualText::new(
                "Let the model decide task complexity",
                "모델에게 작업 복잡도 분류 위임",
            ),
            help: Some(BilingualText::new(
                "Classify the task with a capped-output call to the main model before the planner. Skips planning for chat / simple tasks, fires for complex / architectural.",
                "플래너 전에 메인 모델로 작업 복잡도를 분류합니다. 잡담/단순 작업은 계획을 건너뛰고, 복잡/구조적 작업에는 발화합니다.",
            )),
        },

        // ===== Models =====
        "models.reasoning_effort" => Bilingual {
            group: BilingualText::new("Models", "모델"),
            label: BilingualText::new("Reasoning effort", "추론 강도"),
            help: Some(BilingualText::new(
                "How hard the model thinks: off / low / medium / high / xhigh (cost grows with depth).",
                "모델의 추론 강도: off / low / medium / high / xhigh (깊을수록 비용 증가).",
            )),
        },

        // ===== Security =====
        "security.sandbox" => Bilingual {
            group: BilingualText::new("Security", "보안"),
            label: BilingualText::new("Sandbox mode", "샌드박스 모드"),
            help: Some(BilingualText::new(
                "None = run tools directly; Docker / Firejail = isolate tool execution.",
                "None = 도구를 직접 실행; Docker / Firejail = 격리 실행.",
            )),
        },
        "security.ask_before_install" => Bilingual {
            group: BilingualText::new("Security", "보안"),
            label: BilingualText::new(
                "Confirm before installing dependencies",
                "의존성 설치 전 확인",
            ),
            help: Some(BilingualText::new(
                "Block `cargo add`, `npm install`, etc. until the operator approves.",
                "`cargo add`, `npm install` 등 의존성 설치를 사용자 승인 전까지 차단합니다.",
            )),
        },
        "security.ask_before_delete" => Bilingual {
            group: BilingualText::new("Security", "보안"),
            label: BilingualText::new(
                "Confirm before destructive shell commands",
                "파괴적 셸 명령 전 확인",
            ),
            help: Some(BilingualText::new(
                "Block `rm`, `git clean`, `git reset --hard`, etc. until the operator approves.",
                "`rm`, `git clean`, `git reset --hard` 등 파괴적 명령을 사용자 승인 전까지 차단합니다.",
            )),
        },

        // ===== Git =====
        "git.auto_commit" => Bilingual {
            group: BilingualText::new("Git", "Git"),
            label: BilingualText::new("Auto-commit after each run", "실행 종료 시 자동 커밋"),
            help: Some(BilingualText::new(
                "Create a git commit at the end of every successful run.",
                "성공한 실행이 끝날 때마다 git 커밋을 생성합니다.",
            )),
        },
        "git.auto_branch" => Bilingual {
            group: BilingualText::new("Git", "Git"),
            label: BilingualText::new("Auto-branch before changes", "변경 전 자동 브랜치"),
            help: Some(BilingualText::new(
                "Create a topic branch before the agent starts editing.",
                "에이전트가 편집을 시작하기 전에 토픽 브랜치를 생성합니다.",
            )),
        },

        // ===== TUI (TUI-only — `surfaces = ["tui"]` filters these out of the VS Code webview) =====
        "tui.show_thinking" => Bilingual {
            group: BilingualText::new("TUI", "TUI"),
            label: BilingualText::new("Show model thinking", "모델 추론 표시"),
            help: Some(BilingualText::new(
                "Display extended-thinking output inline (Goal mode only).",
                "extended-thinking 출력을 인라인으로 표시합니다 (Goal 모드 한정).",
            )),
        },
        "tui.show_token_count" => Bilingual {
            group: BilingualText::new("TUI", "TUI"),
            label: BilingualText::new("Show token count", "토큰 카운트 표시"),
            help: Some(BilingualText::new(
                "Surface running token totals in the header.",
                "헤더에 누적 토큰 카운트를 표시합니다.",
            )),
        },
        "tui.show_cost" => Bilingual {
            group: BilingualText::new("TUI", "TUI"),
            label: BilingualText::new("Show running cost", "누적 비용 표시"),
            help: Some(BilingualText::new(
                "Surface the estimated USD spend in the header.",
                "헤더에 추정 USD 지출을 표시합니다.",
            )),
        },
        "tui.show_mascot" => Bilingual {
            group: BilingualText::new("TUI", "TUI"),
            label: BilingualText::new("Show the deer mascot", "사슴 마스코트 표시"),
            help: Some(BilingualText::new(
                "Toggle the idle-state pixel mascot in the side panel.",
                "사이드 패널의 아이들 상태 픽셀 마스코트를 토글합니다.",
            )),
        },
        "tui.mouse_capture" => Bilingual {
            group: BilingualText::new("TUI", "TUI"),
            label: BilingualText::new("Mouse wheel scrolling", "마우스 휠 스크롤"),
            help: Some(BilingualText::new(
                "Scroll the transcript with the mouse wheel. Text selection then uses Shift+drag (Option+drag on macOS).",
                "마우스 휠로 대화 기록을 스크롤합니다. 켜면 텍스트 선택은 Shift+드래그(맥은 Option+드래그)로 합니다.",
            )),
        },

        // ===== UI (cross-surface) =====
        "ui.language" => Bilingual {
            group: BilingualText::new("UI", "UI"),
            label: BilingualText::new("Interface language", "인터페이스 언어"),
            help: Some(BilingualText::new(
                "Locale used for setting labels and UI chrome across the TUI and the VS Code webview. Save and reopen the page to see translated labels.",
                "TUI와 VS Code 웹뷰의 설정 라벨과 UI 텍스트에 적용되는 로케일입니다. 저장 후 페이지를 다시 열면 번역된 라벨이 보입니다.",
            )),
        },

        // ===== Updates =====
        "updates.auto_check" => Bilingual {
            group: BilingualText::new("Updates", "업데이트"),
            label: BilingualText::new("Check for updates on launch", "시작 시 업데이트 확인"),
            help: Some(BilingualText::new(
                "Phone home once at startup to see if a newer Peridot is available.",
                "시작 시 한 번 원격을 호출해 새 Peridot 버전이 있는지 확인합니다.",
            )),
        },

        _ => return None,
    })
}

/// Hardcoded English defaults used when a setting id has no entry in
/// the translation table. Lets the registry add a new field without
/// blocking on the Korean string, while still putting *something*
/// reasonable on the screen.
///
/// `label` is a hardcoded literal rather than the id itself because
/// [`LocalizedSetting`] holds `&'static str`s — the registry copies
/// the id into the owned `SettingItem.label` separately when this
/// fallback fires.
pub(super) fn fallback(_id: &str) -> LocalizedSetting {
    LocalizedSetting {
        group: "Misc",
        label: "(missing translation)",
        help: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_returns_locale_specific_label() {
        let en = lookup("defaults.auto_verify_after_mutation", Locale::En).unwrap();
        let ko = lookup("defaults.auto_verify_after_mutation", Locale::Ko).unwrap();
        assert_ne!(en.label, ko.label);
        assert_eq!(en.label, "Auto-verify after file changes");
        assert!(ko.label.contains("자동 검증"));
    }

    #[test]
    fn lookup_returns_none_for_unknown_id() {
        assert!(lookup("totally.bogus.field", Locale::En).is_none());
    }

    #[test]
    fn fallback_marks_missing_translation_explicitly() {
        // The fallback string is intentionally a fixed placeholder.
        // `settings::make_item` overrides the label with the id when
        // it sees a fallback hit so operators still know *which*
        // setting is untranslated — see that helper for the actual
        // override logic.
        let result = fallback("totally.bogus.field");
        assert_eq!(result.label, "(missing translation)");
        assert_eq!(result.group, "Misc");
        assert!(result.help.is_none());
    }
}
