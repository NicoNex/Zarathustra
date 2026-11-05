//! Liang hyphenation with dictionaries loaded at runtime from a directory of
//! `<language>.standard.bincode` files (shipped as data, see `assets/hyphenation`).

use std::collections::HashMap;
use std::path::PathBuf;

use hyphenation::{Hyphenator, Language, Load, Standard};

pub struct Hyphenators {
    dir: PathBuf,
    /// Dictionary per language tag as the book spells it (`it-IT`, `en`…); `None` = none available.
    by_tag: HashMap<String, Option<usize>>,
    /// Index into `dicts` per dictionary file name, so `it` and `it-IT` share one copy.
    by_name: HashMap<String, usize>,
    dicts: Vec<Standard>,
}

/// Dictionary file names for tags whose plain form has no dictionary of its own.
fn alias(base: &str) -> Option<&'static str> {
    match base {
        "en" => Some("en-us"),
        "de" => Some("de-1996"),
        "no" => Some("nb"),
        _ => None,
    }
}

impl Hyphenators {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Hyphenators { dir: dir.into(), by_tag: HashMap::new(), by_name: HashMap::new(), dicts: Vec::new() }
    }

    /// Byte offsets inside `word` where it may be hyphenated, for BCP 47 tag `lang`
    /// (`it-IT` falls back to `it`; `en`, `de`, `no` map to their default dictionaries).
    pub fn breaks(&mut self, lang: &str, word: &str) -> Vec<usize> {
        let idx = match self.by_tag.get(lang) {
            Some(&idx) => idx,
            None => {
                let idx = self.load(lang);
                self.by_tag.insert(lang.to_string(), idx);
                idx
            }
        };
        idx.map(|i| self.dicts[i].hyphenate(word).breaks).unwrap_or_default()
    }

    fn load(&mut self, lang: &str) -> Option<usize> {
        let tag = lang.to_ascii_lowercase().replace('_', "-");
        let base = tag.split('-').next().unwrap_or_default();
        let candidates = [Some(tag.as_str()), (base != tag).then_some(base), alias(base)];
        for name in candidates.into_iter().flatten() {
            if let Some(&i) = self.by_name.get(name) {
                return Some(i);
            }
            let Some(language) = Language::try_from_code(name) else { continue };
            if let Ok(dict) = Standard::from_path(language, self.dir.join(format!("{name}.standard.bincode"))) {
                self.dicts.push(dict);
                self.by_name.insert(name.to_string(), self.dicts.len() - 1);
                return Some(self.dicts.len() - 1);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyphenates_from_data_files_with_tag_fallbacks() {
        let mut h = Hyphenators::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/hyphenation"));
        assert_eq!(h.breaks("en", "hyphenation"), [2, 6, 7]);
        assert!(!h.breaks("it-IT", "pianoforte").is_empty());
        assert!(!h.breaks("it", "pianoforte").is_empty());
        assert_eq!(h.dicts.len(), 2, "it and it-IT share one dictionary");
        assert!(!h.breaks("de-DE", "Silbentrennung").is_empty(), "de → de-1996");
        assert!(h.breaks("xx", "whatever").is_empty(), "unknown language: no hyphens");
    }
}
