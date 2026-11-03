//! Title normalization (PRD §10.3). **Frozen and versioned**: the output feeds the
//! uuidv5 page namespace, so changing it duplicates pages across devices. Any change
//! requires bumping [`NORMALIZATION_VERSION`] and a migration.

use unicode_normalization::UnicodeNormalization;

pub const NORMALIZATION_VERSION: u32 = 1;

/// `title_key`: NFKC, Unicode default case fold, NFKC again (fold can denormalize),
/// trimmed, and internal whitespace runs collapsed to a single ASCII space.
pub fn normalize_title(title: &str) -> String {
    let nfkc: String = title.nfkc().collect();
    let folded = caseless::default_case_fold_str(&nfkc);
    let renormalized: String = folded.nfkc().collect();
    renormalized
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Display form of a user-typed title: NFC, trimmed, whitespace collapsed. Case is kept.
pub fn clean_title(title: &str) -> String {
    let nfc: String = title.nfc().collect();
    nfc.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_and_whitespace_collapse() {
        assert_eq!(normalize_title("  Apple   Watch "), "apple watch");
        assert_eq!(
            normalize_title("apple watch"),
            normalize_title("Apple Watch")
        );
        assert_eq!(normalize_title("Apple\tWatch\n"), "apple watch");
    }

    #[test]
    fn ac_4_9_unicode_forms_collide() {
        // "é" precomposed vs. e + combining acute.
        assert_eq!(normalize_title("Caf\u{e9}"), normalize_title("Cafe\u{301}"));
        // Full-width letters fold under NFKC.
        assert_eq!(normalize_title("ＡＢＣ"), "abc");
        // German sharp s case-folds to "ss".
        assert_eq!(normalize_title("Straße"), normalize_title("STRASSE"));
    }

    #[test]
    fn clean_keeps_case() {
        assert_eq!(clean_title("  Modern   Religion "), "Modern Religion");
    }
}
