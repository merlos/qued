//! Validated question batches and an in-memory answer/review state machine. No I/O.
use crate::document::Document;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MAX_QUESTIONS: usize = 8;
pub const MAX_OPTIONS: usize = 3;
pub const MAX_ANSWER_BYTES: usize = 4096;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionRequest {
    pub questions: Vec<Question>,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn request(count: usize) -> QuestionRequest {
        QuestionRequest {
            questions: (0..count)
                .map(|index| Question {
                    question: format!("Question {}?", index + 1),
                    header: String::new(),
                    options: vec![
                        OptionItem {
                            label: "First".into(),
                            description: "Option one".into(),
                        },
                        OptionItem {
                            label: "Second".into(),
                            description: String::new(),
                        },
                    ],
                })
                .collect(),
        }
    }

    #[test]
    fn batches_wait_for_confirmation_and_answers_can_be_revised() {
        let mut state = Questionnaire::new(request(2)).unwrap();
        assert!(state.act(Action::Confirm).is_none());
        assert!(state.error.is_some());
        assert!(state.act(Action::Next).is_none());
        assert_eq!(state.page, Page::Question(0));
        assert!(state.act(Action::Pick(0)).is_none());
        assert_eq!(state.page, Page::Question(1));
        assert!(state.act(Action::Pick(1)).is_none());
        assert_eq!(state.page, Page::Summary);
        assert!(state.act(Action::Edit(0)).is_none());
        assert!(state.act(Action::Pick(1)).is_none());
        assert!(state.act(Action::Next).is_none());
        let answers = state.act(Action::Confirm).unwrap();
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0].answer, "Second");
        assert_eq!(answers[0].option_index, Some(1));
        assert_eq!(answers[1].question_index, 1);
    }

    #[test]
    fn custom_drafts_survive_back_forward_and_switching_to_suggested_answers() {
        let mut state = Questionnaire::new(request(2)).unwrap();
        state.act(Action::Pick(2));
        state.insert("界e\u{301}👩‍💻\nSecond line");
        state.act(Action::Next);
        state.act(Action::Previous);
        assert!(state.editing);
        assert_eq!(state.input().unwrap().text, "界e\u{301}👩‍💻\nSecond line");
        state.act(Action::Pick(0));
        state.act(Action::Previous);
        assert!(!state.editing);
        state.act(Action::Pick(2));
        assert!(state.editing);
        state.act(Action::Next);
        state.act(Action::Pick(0));
        let answers = state.act(Action::Confirm).unwrap();
        assert_eq!(answers[0].option_index, None);
        assert_eq!(answers[0].answer, "界e\u{301}👩‍💻\nSecond line");
    }

    #[test]
    fn single_questions_submit_directly_and_open_questions_always_have_other() {
        let mut state = Questionnaire::new(request(1)).unwrap();
        assert_eq!(state.act(Action::Pick(1)).unwrap()[0].answer, "Second");
        let mut open = request(1);
        open.questions[0].options.clear();
        let mut state = Questionnaire::new(open).unwrap();
        assert_eq!(state.actions()[0], Action::Pick(0));
        assert!(state.act(Action::Pick(0)).is_none());
        assert!(state.editing);
        assert!(state.act(Action::Next).is_none());
        state.insert("Custom answer");
        assert_eq!(state.act(Action::Next).unwrap()[0].answer, "Custom answer");
    }

    #[test]
    fn answer_size_selection_replacement_and_controls_are_bounded() {
        let mut state = Questionnaire::new(request(1)).unwrap();
        state.act(Action::Pick(2));
        state.insert(&"x".repeat(MAX_ANSWER_BYTES + 1));
        assert!(state.input().unwrap().text.is_empty());
        assert!(state.error.is_some());
        state.insert(&"x".repeat(MAX_ANSWER_BYTES));
        let input = state.input_mut().unwrap();
        input.move_to(0, false);
        input.move_to(input.text.len(), true);
        state.insert("é\r\n界\0");
        assert_eq!(state.input().unwrap().text, "é\n界");
        assert_eq!(state.act(Action::Next).unwrap()[0].answer, "é\n界");
    }

    #[test]
    fn validates_counts_duplicates_text_and_unknown_arguments() {
        assert!(Questionnaire::new(request(0)).is_err());
        assert!(Questionnaire::new(request(MAX_QUESTIONS + 1)).is_err());
        let mut req = request(1);
        let options = req.questions[0].options.clone();
        req.questions[0].options.extend(options);
        assert!(req.validate().is_err());
        let mut req = request(1);
        req.questions[0].options[1].label = "first".into();
        assert!(req.validate().is_err());
        for text in ["", "   ", "Bad\x1bquestion"] {
            let mut req = request(1);
            req.questions[0].question = text.into();
            assert!(req.validate().is_err());
        }
        assert!(
            serde_json::from_value::<QuestionRequest>(json!({"questions":[], "unexpected":true}))
                .is_err()
        );
        let mut req = request(MAX_QUESTIONS);
        for question in &mut req.questions {
            question.options.push(OptionItem {
                label: "Third".into(),
                description: String::new(),
            });
        }
        assert!(req.validate().is_ok());
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub header: String,
    pub options: Vec<OptionItem>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionItem {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub question_index: usize,
    pub question: String,
    pub answer: String,
    /// Zero-based suggested-option index. Absent for a custom answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option_index: Option<usize>,
}

impl QuestionRequest {
    pub fn validate(&self) -> Result<()> {
        if self.questions.is_empty() || self.questions.len() > MAX_QUESTIONS {
            bail!("Question batches require 1–8 questions");
        }
        for question in &self.questions {
            if !valid_text(&question.question, 2048, false)
                || !valid_text(&question.header, 120, true)
                || question.header.chars().count() > 30
                || question.header.chars().any(char::is_control)
            {
                bail!(
                    "Invalid question text or header (question: 2048 bytes; header: 30 characters)"
                );
            }
            if question.options.len() > MAX_OPTIONS {
                bail!(
                    "Each question allows at most 3 suggested options; Other is added automatically"
                );
            }
            for (index, option) in question.options.iter().enumerate() {
                if !valid_text(&option.label, 120, false)
                    || option.label.chars().any(char::is_control)
                    || !valid_text(&option.description, 1024, true)
                {
                    bail!("Invalid option label or description");
                }
                if question.options[..index]
                    .iter()
                    .any(|other| other.label.trim().eq_ignore_ascii_case(option.label.trim()))
                {
                    bail!("Question option labels must be unique");
                }
            }
        }
        Ok(())
    }
}

fn valid_text(text: &str, limit: usize, empty: bool) -> bool {
    text.len() <= limit
        && (empty || !text.trim().is_empty())
        && !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array", "minItems": 1, "maxItems": MAX_QUESTIONS,
                "items": {
                    "type": "object",
                    "properties": {
                        "question": {"type": "string", "minLength": 1, "maxLength": 2048},
                        "header": {"type": "string", "maxLength": 30},
                        "options": {
                            "type": "array", "maxItems": MAX_OPTIONS,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": {"type": "string", "minLength": 1, "maxLength": 120},
                                    "description": {"type": "string", "maxLength": 1024}
                                },
                                "required": ["label"], "additionalProperties": false
                            }
                        }
                    },
                    "required": ["question", "options"], "additionalProperties": false
                }
            }
        },
        "required": ["questions"], "additionalProperties": false
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Question(usize),
    Summary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Pick(usize),
    Previous,
    Next,
    Edit(usize),
    Confirm,
    Input,
}

#[derive(Debug, Default)]
pub struct Draft {
    pub choice: Option<usize>,
    /// Reuses the editor's grapheme-safe operations, never filesystem operations.
    pub input: Document,
}

#[derive(Debug)]
pub struct Questionnaire {
    pub request: QuestionRequest,
    pub drafts: Vec<Draft>,
    pub page: Page,
    pub selected: usize,
    pub editing: bool,
    pub error: Option<String>,
}

impl Questionnaire {
    pub fn new(request: QuestionRequest) -> Result<Self> {
        request.validate()?;
        let drafts = (0..request.questions.len())
            .map(|_| Draft::default())
            .collect();
        Ok(Self {
            request,
            drafts,
            page: Page::Question(0),
            selected: 0,
            editing: false,
            error: None,
        })
    }

    pub fn actions(&self) -> Vec<Action> {
        match self.page {
            Page::Question(index) => (0..=self.request.questions[index].options.len())
                .map(Action::Pick)
                .chain([Action::Previous, Action::Next])
                .collect(),
            Page::Summary => (0..self.drafts.len())
                .map(Action::Edit)
                .chain([Action::Confirm, Action::Previous])
                .collect(),
        }
    }

    pub fn selected_action(&self) -> Action {
        let actions = self.actions();
        actions[self.selected.min(actions.len() - 1)]
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.error = None;
        let count = self.actions().len();
        self.selected = self.selected.saturating_add_signed(delta).min(count - 1);
        self.editing = false;
    }

    pub fn input(&self) -> Option<&Document> {
        match self.page {
            Page::Question(index) => Some(&self.drafts[index].input),
            Page::Summary => None,
        }
    }

    pub fn input_mut(&mut self) -> Option<&mut Document> {
        match self.page {
            Page::Question(index) => Some(&mut self.drafts[index].input),
            Page::Summary => None,
        }
    }

    pub fn insert(&mut self, text: &str) {
        if !self.editing {
            return;
        }
        let text: String = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect();
        let input = self.input_mut().expect("Question input");
        let replaced = input.selection().map_or(0, |range| range.len());
        if input.text.len() - replaced + text.len() > MAX_ANSWER_BYTES {
            self.error = Some("Custom answers are limited to 4096 UTF-8 bytes".into());
            return;
        }
        input.insert(&text);
        self.error = None;
    }

    /// Returns answers only after a single answer is submitted or a batch is confirmed.
    pub fn act(&mut self, action: Action) -> Option<Vec<Answer>> {
        self.error = None;
        match self.apply(action) {
            Ok(answers) => answers,
            Err(error) => {
                self.error = Some(error.to_string());
                None
            }
        }
    }

    fn apply(&mut self, action: Action) -> Result<Option<Vec<Answer>>> {
        match (self.page, action) {
            (Page::Question(index), Action::Pick(choice)) => {
                let other = self.request.questions[index].options.len();
                if choice > other {
                    bail!("Unknown question option");
                }
                self.drafts[index].choice = Some(choice);
                self.selected = choice;
                self.editing = choice == other;
                if !self.editing {
                    return self.advance();
                }
            }
            (Page::Question(index), Action::Input) => {
                return self.apply(Action::Pick(self.request.questions[index].options.len()));
            }
            (Page::Question(_), Action::Next) => return self.advance(),
            (Page::Question(index), Action::Previous) if index > 0 => self.visit(index - 1),
            (Page::Summary, Action::Previous) => self.visit(self.drafts.len() - 1),
            (Page::Summary, Action::Edit(index)) if index < self.drafts.len() => self.visit(index),
            (Page::Summary, Action::Confirm) => return Ok(Some(self.answers()?)),
            (_, Action::Confirm) => bail!("Review the answers before confirming"),
            _ => {}
        }
        Ok(None)
    }

    fn advance(&mut self) -> Result<Option<Vec<Answer>>> {
        let Page::Question(index) = self.page else {
            return Ok(None);
        };
        self.answer(index)?;
        if self.drafts.len() == 1 {
            return Ok(Some(self.answers()?));
        }
        if index + 1 < self.drafts.len() {
            self.visit(index + 1);
        } else {
            self.answers()?;
            self.page = Page::Summary;
            self.editing = false;
            self.selected = self.drafts.len(); // Highlight explicit confirmation.
        }
        Ok(None)
    }

    fn visit(&mut self, index: usize) {
        self.page = Page::Question(index);
        self.selected = self.drafts[index].choice.unwrap_or(0);
        self.editing =
            self.drafts[index].choice == Some(self.request.questions[index].options.len());
    }

    pub fn answer(&self, index: usize) -> Result<Answer> {
        let question = &self.request.questions[index];
        let draft = &self.drafts[index];
        let Some(choice) = draft.choice else {
            bail!("Answer question {} before continuing", index + 1);
        };
        let (answer, option_index) = if choice == question.options.len() {
            let text = draft.input.text.trim();
            if text.is_empty() || text.len() > MAX_ANSWER_BYTES {
                bail!("Enter a nonempty custom answer for question {}", index + 1);
            }
            (text.to_owned(), None)
        } else {
            let Some(option) = question.options.get(choice) else {
                bail!("Unknown question option")
            };
            (option.label.clone(), Some(choice))
        };
        Ok(Answer {
            question_index: index,
            question: question.question.clone(),
            answer,
            option_index,
        })
    }

    fn answers(&self) -> Result<Vec<Answer>> {
        (0..self.drafts.len())
            .map(|index| self.answer(index))
            .collect()
    }
}
