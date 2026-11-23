//! Names as Finder orders them (`file3` before `file29`): the TS box's
//! `localeCompare(…, { numeric: true, sensitivity: "base" })`, which its runtime (bun) collates as
//! `en-US` whatever the computer's language, that is ICU's root collation: numbers by value, case
//! and accents alike, every script in the root order.

use std::cmp::Ordering;

use icu_collator::options::{CollatorOptions, Strength};
use icu_collator::preferences::CollationNumericOrdering;
use icu_collator::{Collator, CollatorBorrowed, CollatorPreferences};

/// The order names are listed in.
pub(crate) struct FinderOrder(CollatorBorrowed<'static>);

impl FinderOrder {
    pub(crate) fn new() -> Self {
        let mut options = CollatorOptions::default();
        // (`sensitivity: "base"`: only the letters themselves count.)
        options.strength = Some(Strength::Primary);
        let mut preferences = CollatorPreferences::default();
        preferences.numeric_ordering = Some(CollationNumericOrdering::True);
        Self::with(preferences, options)
    }

    /// A plain `localeCompare(b)`: case and accents count, digits one by one.
    pub(crate) fn plain() -> Self {
        Self::with(CollatorPreferences::default(), CollatorOptions::default())
    }

    fn with(preferences: CollatorPreferences, options: CollatorOptions) -> Self {
        // The root collation's data is compiled in: it's always there.
        Self(Collator::try_new(preferences, options).expect("the root collation is compiled in"))
    }

    /// Two names in Finder's order; equal where only case or accents differ.
    pub(crate) fn compare(&self, a: &str, b: &str) -> Ordering {
        self.0.compare(a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_by_value_case_and_accents_alike() {
        let order = FinderOrder::new();
        let mut names = vec![
            "file29", "File3", "b", "É", "a b", "a_b", "a-b", "ab", "10", "9", "Zeta", "e",
        ];
        names.sort_by(|a, b| order.compare(a, b));
        assert_eq!(
            names,
            [
                "9", "10", "a b", "a_b", "a-b", "ab", "b", "É", "e", "File3", "file29", "Zeta"
            ]
        );
        assert_eq!(order.compare("01", "1"), Ordering::Equal);
    }

    /// As bun sorts them (`localeCompare`, numeric, base), any `LANG`.
    #[test]
    fn other_scripts_in_the_root_order() {
        let order = FinderOrder::new();
        let mut names = vec![
            "Ölçü",
            "ıspanak",
            "İzmir",
            "ilk",
            "zebra",
            "Ärger",
            "ça",
            "cz",
            "日本",
            "中文",
            "한국",
            "Δέλτα",
            "alpha",
            "Привет",
            "ябло",
            "العربية",
            "file10",
            "file9",
            "_x",
            "ﬁle",
            "ǅ",
            "😀",
            "a b",
            "ab",
        ];
        names.sort_by(|a, b| order.compare(a, b));
        assert_eq!(
            names,
            [
                "_x",
                "😀",
                "a b",
                "ab",
                "alpha",
                "Ärger",
                "ça",
                "cz",
                "ǅ",
                "ﬁle",
                "file9",
                "file10",
                "ilk",
                "İzmir",
                "ıspanak",
                "Ölçü",
                "zebra",
                "Δέλτα",
                "Привет",
                "ябло",
                "العربية",
                "한국",
                "中文",
                "日本",
            ]
        );
    }
}
