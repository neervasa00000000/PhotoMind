//! Interpretable weights for deterministic group ranking.
//! Adjust here rather than scattering magic numbers.
//!
//! SAFETY NOTE: Eye-based ranking is gated by ENABLE_EYE_BASED_RANKING in decisions.rs.
//! When disabled, eye_score is set to neutral (0.5) regardless of detection results,
//! so the eyes_open weight effectively contributes a constant term and doesn't affect
//! relative ranking. This allows eye detection to run and be validated without
//! influencing important culling decisions until empirical accuracy is confirmed.

pub struct DecisionWeights {
    pub technical_sharpness: f64,
    pub technical_clipping: f64,
    pub face_sharpness: f64,
    pub eyes_open: f64,          // Gated by ENABLE_EYE_BASED_RANKING (currently disabled)
    pub expression: f64,
    pub exposure_ok: f64,
}

pub const WEIGHTS: DecisionWeights = DecisionWeights {
    technical_sharpness: 0.35,
    technical_clipping: 0.10,
    face_sharpness: 0.25,
    eyes_open: 0.20,
    expression: 0.10,
    exposure_ok: 0.05,
};

/// Minimum score gap to claim a confident best-of-group winner.
pub const BEST_GAP_CONFIDENT: f64 = 0.06;

/// Top two within this gap are treated as tied.
pub const BEST_TIE_EPSILON: f64 = 0.025;
