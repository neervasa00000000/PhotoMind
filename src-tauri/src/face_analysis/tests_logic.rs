#[cfg(test)]
mod eye_logic {
    use crate::face_analysis::types::{EyeAnalysis, EyeState};

    fn classify(openness: f32, visibility: f32) -> (EyeState, f32) {
        const OPEN: f32 = 0.52;
        const CLOSED: f32 = 0.38;
        match openness {
            s if s >= OPEN => (
                EyeState::Open,
                ((s - OPEN) / 0.35 + 0.55).clamp(0.0, 1.0) * visibility,
            ),
            s if s <= CLOSED => (
                EyeState::Closed,
                ((CLOSED - s) / 0.35 + 0.55).clamp(0.0, 1.0) * visibility,
            ),
            _ => (EyeState::Uncertain, 0.45 * visibility),
        }
    }

    #[test]
    fn uncertain_band_does_not_force_open_or_closed() {
        let (state, _) = classify(0.45, 1.0);
        assert_eq!(state, EyeState::Uncertain);
    }

    #[test]
    fn low_confidence_stays_uncertain_when_borderline() {
        let eye = EyeAnalysis {
            visibility_confidence: 0.2,
            openness_score: Some(0.49),
            state: EyeState::Uncertain,
            confidence: 0.2,
            region_sharpness: None,
        };
        assert_eq!(eye.state, EyeState::Uncertain);
    }
}
