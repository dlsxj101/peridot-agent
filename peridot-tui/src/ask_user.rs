use super::*;

/// Interactive ask-user prompt shown as a special TUI screen.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AskUserPanel {
    /// Question text.
    pub question: String,
    /// Selectable choices.
    pub choices: Vec<String>,
    /// Currently highlighted choice.
    pub selected_index: usize,
    /// Free-form fallback text.
    pub freeform: String,
    /// Optional explanation text shown by the [?] item.
    pub explanation: Option<String>,
    /// Whether the explanation is currently visible.
    pub showing_explanation: bool,
    /// Index of the synthetic Other choice.
    pub other_index: Option<usize>,
    /// Index of the synthetic Explain choice.
    pub explain_index: Option<usize>,
}

/// Esc menu state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MenuState {
    /// Menu options.
    pub options: Vec<String>,
    /// Highlighted option.
    pub selected_index: usize,
}

/// Approval prompt shown when a tool needs explicit user confirmation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApprovalPanel {
    /// Tool requesting approval.
    pub tool_name: String,
    /// Reason the operation is gated.
    pub reason: String,
    /// Currently highlighted choice.
    pub selected_index: usize,
}

/// User decision from an approval prompt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    /// Allow the operation.
    Approve,
    /// Deny the operation.
    Deny,
}

impl ApprovalPanel {
    /// Creates a tool approval panel.
    pub fn new(tool_name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            reason: reason.into(),
            selected_index: 0,
        }
    }

    pub(super) fn choices(&self) -> [&'static str; 2] {
        ["Approve once", "Deny"]
    }

    pub(super) fn selected_decision(&self) -> ApprovalDecision {
        if self.selected_index == 0 {
            ApprovalDecision::Approve
        } else {
            ApprovalDecision::Deny
        }
    }
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            options: vec![
                "Mode".to_string(),
                "Permission".to_string(),
                "Save Session".to_string(),
                "History".to_string(),
                "Settings".to_string(),
                "Keybindings".to_string(),
                "Quit".to_string(),
            ],
            selected_index: 0,
        }
    }
}

impl AskUserPanel {
    /// Builds a panel from an ask-user request.
    pub fn from_request(request: AskUserRequest) -> Self {
        match request {
            AskUserRequest::SingleSelect {
                question,
                options,
                default_index,
            } => Self {
                question,
                choices: ask_user_choices_with_controls(options),
                selected_index: default_index.unwrap_or(0),
                freeform: String::new(),
                explanation: Some("Peridot needs this decision before continuing.".to_string()),
                showing_explanation: false,
                other_index: None,
                explain_index: None,
            },
            AskUserRequest::MultiSelect {
                question, options, ..
            } => Self {
                question,
                choices: ask_user_choices_with_controls(options),
                selected_index: 0,
                freeform: String::new(),
                explanation: Some(
                    "Peridot needs one or more choices before continuing.".to_string(),
                ),
                showing_explanation: false,
                other_index: None,
                explain_index: None,
            },
            AskUserRequest::FreeForm {
                question, default, ..
            } => Self {
                question,
                choices: Vec::new(),
                selected_index: 0,
                freeform: default.unwrap_or_default(),
                explanation: None,
                showing_explanation: false,
                other_index: None,
                explain_index: None,
            },
        }
        .with_control_indexes()
    }

    pub(super) fn selected_answer(&self) -> String {
        self.choices
            .get(self.selected_index)
            .cloned()
            .unwrap_or_else(|| self.freeform.clone())
    }

    fn with_control_indexes(mut self) -> Self {
        if self.choices.len() >= 2 {
            let other = self.choices.len() - 2;
            let explain = self.choices.len() - 1;
            if self.choices[other] == "[o] Other" && self.choices[explain] == "[?] Explain" {
                self.other_index = Some(other);
                self.explain_index = Some(explain);
            }
        }
        self
    }

    pub(super) fn enter_other_mode(&mut self) {
        self.choices.clear();
        self.selected_index = 0;
        self.freeform.clear();
        self.other_index = None;
        self.explain_index = None;
        self.showing_explanation = false;
    }
}

pub(super) fn ask_user_choices_with_controls(mut options: Vec<String>) -> Vec<String> {
    options.push("[o] Other".to_string());
    options.push("[?] Explain".to_string());
    options
}
