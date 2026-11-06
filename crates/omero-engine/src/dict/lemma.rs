//! Forms to try when a word is not a headword: the word as written, lowercased, without an
//! elided article (`l'aria`) or a possessive (`dog's`), then guesses at its lemma by stripping
//! common English and Italian inflections.
//!
//! Dictionaries with a `.syn` file (KOReader's Wiktionary packs) list inflected forms
//! themselves; this is the fallback for those without.
// ponytail: a few dozen suffix rules, no dictionary of exceptions (went → go, uomini → uomo);
// irregular forms only resolve through a `.syn`. Swap in a real stemmer/lemmatizer (Snowball
// rules, or Hunspell .aff affix tables already on most systems) if lookups miss too often.

/// (suffix, replacements) for English: plurals, verb forms, comparatives, adverbs.
const EN: &[(&str, &[&str])] = &[
    ("ies", &["y"]),
    ("ves", &["f", "fe"]),
    ("ied", &["y"]),
    ("ier", &["y"]),
    ("iest", &["y"]),
    ("ily", &["y"]),
    ("sses", &["ss"]),
    ("es", &["", "e"]),
    ("s", &[""]),
    ("ed", &["", "e", "-"]),
    ("ing", &["", "e", "-"]),
    ("er", &["", "e", "-"]),
    ("est", &["", "e", "-"]),
    ("ly", &[""]),
];

/// (suffix, replacements) for Italian: plurals and feminines, then common verb endings back
/// to the infinitive.
const IT: &[(&str, &[&str])] = &[
    ("chi", &["co"]),
    ("ghi", &["go"]),
    ("che", &["ca"]),
    ("ghe", &["ga"]),
    ("ando", &["are"]),
    ("endo", &["ere", "ire"]),
    ("arono", &["are"]),
    ("erono", &["ere"]),
    ("irono", &["ire"]),
    ("avano", &["are"]),
    ("evano", &["ere"]),
    ("ivano", &["ire"]),
    ("iamo", &["are", "ere", "ire"]),
    ("ammo", &["are"]),
    ("emmo", &["ere"]),
    ("immo", &["ire"]),
    ("ato", &["are"]),
    ("ata", &["are"]),
    ("ati", &["are"]),
    ("ate", &["are"]),
    ("uto", &["ere"]),
    ("uta", &["ere"]),
    ("uti", &["ere"]),
    ("ute", &["ere"]),
    ("ito", &["ire"]),
    ("ita", &["ire"]),
    ("iti", &["ire"]),
    ("ava", &["are"]),
    ("avo", &["are"]),
    ("eva", &["ere"]),
    ("evo", &["ere"]),
    ("iva", &["ire"]),
    ("ivo", &["ire"]),
    ("erò", &["are", "ere"]),
    ("erà", &["are", "ere"]),
    ("irò", &["ire"]),
    ("irà", &["ire"]),
    ("ano", &["are"]),
    ("ono", &["ere", "ire"]),
    ("ai", &["are"]),
    ("ò", &["are"]),
    ("ì", &["ire"]),
    ("i", &["o", "e", "a", "are", "ere", "ire"]),
    ("e", &["a", "o", "ere", "ire"]),
    ("a", &["o", "are"]),
    ("o", &["are", "ere", "ire"]),
];

/// The forms to look up for `word`, best first, without duplicates. `lang` is the book's
/// language (BCP 47); unknown languages get both rule sets.
pub fn candidates(word: &str, lang: Option<&str>) -> Vec<String> {
    let word: String =
        word.trim().chars().filter(|&c| c != '\u{ad}').map(|c| if c == '’' { '\'' } else { c }).collect();
    let mut out: Vec<String> = Vec::new();
    let mut push = |w: String| {
        if !w.is_empty() && !out.contains(&w) {
            out.push(w);
        }
    };
    push(word.clone());
    let lower = word.to_lowercase();
    push(lower.clone());
    let primary = lang.map(|l| l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase());
    let (en, it) = match primary.as_deref() {
        Some("en") => (true, false),
        Some("it") => (false, true),
        _ => (true, true),
    };
    // Elision (l'aria, dell'anno, un'altra) and possessives (dog's, dogs').
    let mut stems = vec![lower.clone()];
    if let Some((head, tail)) = lower.rsplit_once('\'') {
        let possessive = en && (tail == "s" || tail.is_empty());
        let stem = if possessive { head } else { tail };
        if stem.chars().count() >= 2 {
            push(stem.to_string());
            stems.push(stem.to_string());
        }
    }
    for stem in stems {
        for (on, rules) in [(en, EN), (it, IT)] {
            if on {
                for w in strip(&stem, rules) {
                    push(w);
                }
            }
        }
    }
    out
}

/// Every rule matching `word`'s ending, applied; `-` undoes a doubled final consonant
/// (stopped → stop).
fn strip(word: &str, rules: &[(&str, &[&str])]) -> Vec<String> {
    let mut out = Vec::new();
    for (suffix, reps) in rules {
        let Some(stem) = word.strip_suffix(suffix) else { continue };
        if stem.chars().count() < 2 {
            continue;
        }
        for rep in *reps {
            if *rep == "-" {
                let mut cs = stem.chars().rev();
                if let (Some(a), Some(b)) = (cs.next(), cs.next()) {
                    if a == b && !"aeiou".contains(a) {
                        out.push(stem[..stem.len() - a.len_utf8()].to_string());
                    }
                }
            } else {
                out.push(format!("{stem}{rep}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(word: &str, lang: &str, lemma: &str) -> bool {
        candidates(word, Some(lang)).iter().any(|c| c == lemma)
    }

    #[test]
    fn english_inflections() {
        for (w, l) in [
            ("Houses", "house"),
            ("cities", "city"),
            ("wolves", "wolf"),
            ("knives", "knife"),
            ("boxes", "box"),
            ("glasses", "glass"),
            ("walked", "walk"),
            ("hoped", "hope"),
            ("stopped", "stop"),
            ("carried", "carry"),
            ("running", "run"),
            ("making", "make"),
            ("bigger", "big"),
            ("happiest", "happy"),
            ("quickly", "quick"),
            ("dog's", "dog"),
            ("dogs'", "dogs"),
        ] {
            assert!(has(w, "en-GB", l), "{w} → {l}: {:?}", candidates(w, Some("en")));
        }
        assert_eq!(candidates("The", Some("en"))[..2], ["The".to_string(), "the".to_string()]);
    }

    #[test]
    fn italian_inflections() {
        for (w, l) in [
            ("case", "casa"),
            ("libri", "libro"),
            ("amiche", "amica"),
            ("laghi", "lago"),
            ("parlando", "parlare"),
            ("credendo", "credere"),
            ("mangiato", "mangiare"),
            ("venduti", "vendere"),
            ("finita", "finire"),
            ("parlavano", "parlare"),
            ("parlarono", "parlare"),
            ("parlò", "parlare"),
            ("ritrovai", "ritrovare"),
            ("parliamo", "parlare"),
            ("parlo", "parlare"),
            ("l'aria", "aria"),
            ("dell’anno", "anno"),
            ("Selve", "selva"),
        ] {
            assert!(has(w, "it", l), "{w} → {l}: {:?}", candidates(w, Some("it")));
        }
    }

    #[test]
    fn exact_forms_come_first_and_soft_hyphens_go() {
        let c = candidates("Ca\u{ad}sa", Some("it"));
        assert_eq!(c[0], "Casa");
        assert_eq!(c[1], "casa");
        assert!(!has("a", "it", ""), "no empty forms");
        // Unknown language: both rule sets.
        assert!(candidates("cities", None).contains(&"city".to_string()));
        assert!(candidates("libri", None).contains(&"libro".to_string()));
    }
}
