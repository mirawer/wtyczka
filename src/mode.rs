use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AnswerMode {
    #[default]
    Digits,
    Dots,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionMode {
    #[default]
    Single,
    Multiple,
}

impl SelectionMode {
    pub fn description(self) -> &'static str {
        match self {
            Self::Single => "jedna poprawna odpowiedź",
            Self::Multiple => "wiele poprawnych odpowiedzi",
        }
    }
}

impl AnswerMode {
    pub fn max_choices(self) -> u8 {
        match self {
            Self::Digits => 9,
            Self::Dots => 6,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Digits => "cyfry 1–9 (A–I)",
            Self::Dots => "kropki 1–6 (A–F)",
        }
    }
}
