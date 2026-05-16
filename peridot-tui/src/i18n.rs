//! User-facing string lookup for the TUI.
//!
//! Each phrase has a stable `PhraseKey`. `tr(key, locale)` returns the rendered
//! string for the active locale. Default fallback is English so that adding new
//! keys never silently surfaces an empty string.

use peridot_common::Locale;

/// Stable identifier for one localized phrase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhraseKey {
    /// Status bar: agent is idle, awaiting user input.
    StatusIdle,
    /// Status bar: assistant stream is in flight.
    StatusProcessing,
    /// Status bar: agent run finished successfully.
    StatusDone,
    /// Status bar: agent run failed.
    StatusFailed,
    /// Status bar: agent run was interrupted by the user.
    StatusInterrupted,
    /// Status bar: waiting on an ask_user response.
    StatusWaitingUser,
    /// Status bar: waiting on a tool approval response.
    StatusWaitingApproval,
    /// Status bar: one or more tools are executing (suffix is " ...").
    StatusToolRunning,
    /// Status bar: input queue depth suffix.
    StatusQueueSuffix,
    /// Transcript notice: user input was added to the queue.
    NoticeQueued,
    /// Transcript notice: shown alongside the queued task with a running label.
    NoticeRunning,
    /// Transcript notice: shown alongside the queued task when no last task is known.
    NoticeRunningGeneric,
}

/// Looks up the rendered phrase for `key` in `locale`.
pub fn tr(key: PhraseKey, locale: Locale) -> &'static str {
    match (key, locale) {
        (PhraseKey::StatusIdle, Locale::En) => "idle",
        (PhraseKey::StatusIdle, Locale::Ko) => "대기 중",
        (PhraseKey::StatusProcessing, Locale::En) => "processing...",
        (PhraseKey::StatusProcessing, Locale::Ko) => "처리 중...",
        (PhraseKey::StatusDone, Locale::En) => "done",
        (PhraseKey::StatusDone, Locale::Ko) => "완료",
        (PhraseKey::StatusFailed, Locale::En) => "failed",
        (PhraseKey::StatusFailed, Locale::Ko) => "실패",
        (PhraseKey::StatusInterrupted, Locale::En) => "interrupted",
        (PhraseKey::StatusInterrupted, Locale::Ko) => "사용자 중단",
        (PhraseKey::StatusWaitingUser, Locale::En) => "waiting on user response",
        (PhraseKey::StatusWaitingUser, Locale::Ko) => "사용자 응답 대기",
        (PhraseKey::StatusWaitingApproval, Locale::En) => "waiting for approval",
        (PhraseKey::StatusWaitingApproval, Locale::Ko) => "승인 대기 중",
        (PhraseKey::StatusToolRunning, Locale::En) => "tool running:",
        (PhraseKey::StatusToolRunning, Locale::Ko) => "도구 실행 중:",
        (PhraseKey::StatusQueueSuffix, Locale::En) => "queued",
        (PhraseKey::StatusQueueSuffix, Locale::Ko) => "대기열",
        (PhraseKey::NoticeQueued, Locale::En) => "queued",
        (PhraseKey::NoticeQueued, Locale::Ko) => "대기열에 추가됨",
        (PhraseKey::NoticeRunning, Locale::En) => "running:",
        (PhraseKey::NoticeRunning, Locale::Ko) => "작업 중:",
        (PhraseKey::NoticeRunningGeneric, Locale::En) => "agent is busy",
        (PhraseKey::NoticeRunningGeneric, Locale::Ko) => "현재 작업 진행 중",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_matches_default_locale() {
        assert_eq!(tr(PhraseKey::StatusIdle, Locale::En), "idle");
        assert_eq!(tr(PhraseKey::StatusDone, Locale::En), "done");
        assert_eq!(tr(PhraseKey::StatusFailed, Locale::En), "failed");
    }

    #[test]
    fn korean_phrases_are_distinct() {
        assert_eq!(tr(PhraseKey::StatusIdle, Locale::Ko), "대기 중");
        assert_eq!(tr(PhraseKey::StatusDone, Locale::Ko), "완료");
        assert_eq!(tr(PhraseKey::StatusFailed, Locale::Ko), "실패");
    }
}
