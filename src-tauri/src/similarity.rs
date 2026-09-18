//! Similarity taxonomy.
//!
//! A single place that turns perceptual hash distance into a human label, so
//! every layer (moments, duplicates, later UI) agrees on what "identical"
//! means. Exact byte equality is decided by sha256 elsewhere; this module
//! grades *visual* proximity from the 64-bit dHash stored per photo.

/// Visual-proximity grades between two photos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimilarityGrade {
    /// Very close dHash values; collisions are possible, so inspect the frames.
    NearIdentical,
    /// Same shot, tiny differences (crops, compression, focus drift).
    Similar,
    /// Same scene, different framing or instant.
    Related,
    /// No close perceptual-hash match; not a keeper-decision input.
    Distinct,
}

impl SimilarityGrade {
    pub fn label(self) -> &'static str {
        match self {
            Self::NearIdentical => "near-identical",
            Self::Similar => "similar",
            Self::Related => "related",
            Self::Distinct => "distinct",
        }
    }

    /// Thresholds for intra-moment comparisons (nos. of differing hash bits).
    pub fn is_candidate(self) -> bool {
        matches!(self, Self::NearIdentical | Self::Similar)
    }
}

/// Photographic words for two hash distances.
pub fn distance_grade(distance: u32) -> SimilarityGrade {
    match distance {
        0..=4 => SimilarityGrade::NearIdentical,
        5..=12 => SimilarityGrade::Similar,
        13..=24 => SimilarityGrade::Related,
        _ => SimilarityGrade::Distinct,
    }
}

/// Hamming distance between two hex-encoded perceptual hashes.
pub fn hash_distance(h1: &str, h2: &str) -> Option<u32> {
    if h1.len() != 16 || h2.len() != 16 {
        return None;
    }
    let first = u64::from_str_radix(h1, 16).ok()?;
    let second = u64::from_str_radix(h2, 16).ok()?;
    Some((first ^ second).count_ones())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grades_follow_the_documented_thresholds() {
        assert_eq!(distance_grade(0).label(), "near-identical");
        assert_eq!(distance_grade(4).label(), "near-identical");
        assert_eq!(distance_grade(5).label(), "similar");
        assert_eq!(distance_grade(12).label(), "similar");
        assert_eq!(distance_grade(13).label(), "related");
        assert_eq!(distance_grade(24).label(), "related");
        assert_eq!(distance_grade(25).label(), "distinct");
        assert_eq!(distance_grade(64).label(), "distinct");
    }

    #[test]
    fn hash_distance_counts_differing_bits() {
        let zeros = "0000000000000000";
        let ones = "ffffffffffffffff";
        assert_eq!(hash_distance(zeros, zeros), Some(0));
        assert_eq!(hash_distance(zeros, ones), Some(64));
        let single = "0000000000000001"; // only the low bit differs
        assert_eq!(hash_distance(zeros, single), Some(1));
    }

    #[test]
    fn malformed_hashes_yield_no_grade() {
        assert!(hash_distance("", "").is_none());
        assert!(hash_distance("00", "00").is_none());
        assert!(hash_distance("abc", "0000000000000000").is_none());
        assert!(hash_distance("0000000000000000", "000000000000000").is_none());
    }

    #[test]
    fn candidates_are_only_visual_matches() {
        assert!(distance_grade(0).is_candidate());
        assert!(distance_grade(12).is_candidate());
        assert!(!distance_grade(13).is_candidate());
    }
}
