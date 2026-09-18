use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum DecisionReason {
    SharpestInGroup,
    StrongFaceSharpness,
    PossibleClosedEyes,
    AllEyesLikelyOpen,
    StrongerExpression,
    LowerExpression,
    GoodExposure,
    ExposureProblem,
    ExactDuplicate,
    StrongerAlternative,
    UniqueMoment,
    NoBetterAlternative,
    BestOfGroup,
    EyeStateUncertain,
    FaceCountMismatch,
    CloseComparison,
    KeptByUser,
    TechnicalUnavailable,
}

impl DecisionReason {
    pub fn message(self) -> &'static str {
        match self {
            DecisionReason::SharpestInGroup => "Strongest measured sharpness in this group",
            DecisionReason::StrongFaceSharpness => "Strongest face sharpness in this group",
            DecisionReason::PossibleClosedEyes => "One or more faces may have closed eyes",
            DecisionReason::AllEyesLikelyOpen => "All detected eyes likely open",
            DecisionReason::StrongerExpression => {
                "Stronger reliable expression score than alternatives"
            }
            DecisionReason::LowerExpression => "Lower expression score than the suggested keeper",
            DecisionReason::GoodExposure => "Exposure acceptable",
            DecisionReason::ExposureProblem => "Possible exposure problem",
            DecisionReason::ExactDuplicate => "Identical file contents; a keeper is available",
            DecisionReason::StrongerAlternative => "A stronger equivalent exists in this group",
            DecisionReason::UniqueMoment => "Unique photo; no equivalent alternative found",
            DecisionReason::NoBetterAlternative => {
                "No clearly better alternative found; review rather than reject"
            }
            DecisionReason::BestOfGroup => "Best overall candidate in this group",
            DecisionReason::EyeStateUncertain => "One or more eye states are uncertain",
            DecisionReason::FaceCountMismatch => {
                "Face count differs from other frames in this group"
            }
            DecisionReason::CloseComparison => "Top candidates are very close; review recommended",
            DecisionReason::KeptByUser => "Kept by you",
            DecisionReason::TechnicalUnavailable => {
                "Technical analysis unavailable; inspect the original"
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonBundle {
    pub messages: Vec<String>,
    pub codes: Vec<DecisionReason>,
}

impl ReasonBundle {
    pub fn from_codes(codes: Vec<DecisionReason>) -> Self {
        let messages = codes.iter().map(|c| c.message().to_string()).collect();
        Self { messages, codes }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{\"messages\":[],\"codes\":[]}".into())
    }
}

pub fn parse_reasoning(raw: &str) -> Vec<String> {
    if let Ok(bundle) = serde_json::from_str::<ReasonBundle>(raw) {
        return bundle.messages;
    }
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_else(|_| vec![raw.to_string()])
}
