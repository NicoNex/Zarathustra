//! Liang hyphenation with dictionaries loaded at runtime from a directory of
//! `<language>.standard.bincode` files (shipped as data, see `assets/hyphenation`).

use std::collections::HashMap;
use std::path::PathBuf;

use hyphenation::{Hyphenator, Language, Load, Standard};

pub struct Hyphenators {
    dir: PathBuf,
    /// Loaded (or known-missing) dictionaries by normalized tag.
    cache: HashMap<String, Option<Standard>>,
}

impl Hyphenators {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Hyphenators { dir: dir.into(), cache: HashMap::new() }
    }

    /// Byte offsets inside `word` where it may be hyphenated, for BCP 47 tag `lang`
    /// (`it-IT` falls back to `it`, plain `en` means `en-us`).
    pub fn breaks(&mut self, lang: &str, word: &str) -> Vec<usize> {
        let tag = lang.to_ascii_lowercase().replace('_', "-");
        let base = tag.split('-').next().unwrap_or_default().to_string();
        let candidates = [tag.clone(), if base == "en" { "en-us".into() } else { base }];
        for t in candidates {
            let dir = &self.dir;
            let dict = self.cache.entry(t.clone()).or_insert_with(|| {
                let lang = Language::try_from_code(&t)?;
                Standard::from_path(lang, dir.join(format!("{t}.standard.bincode"))).ok()
            });
            if let Some(d) = dict {
                return d.hyphenate(word).breaks;
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyphenates_english_and_italian_from_data_files() {
        let mut h = Hyphenators::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/hyphenation"));
        assert_eq!(h.breaks("en", "hyphenation"), [2, 6, 7]);
        let it = h.breaks("it-IT", "pianoforte");
        assert!(!it.is_empty(), "{it:?}");
        assert!(h.breaks("xx", "whatever").is_empty(), "unknown language: no hyphens");
    }
}
