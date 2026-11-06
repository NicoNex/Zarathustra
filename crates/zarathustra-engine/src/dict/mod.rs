//! Dictionary lookup: the StarDict dictionaries found in a few folders, searched for a word
//! in the book's language first.
//!
//! Amazon's own Kindle dictionaries (`.azw` in `documents/dictionaries`) are DRM-encrypted
//! MOBI and are not read; the free Wiktionary StarDict packs (see `scripts/fetch-dicts.sh`)
//! take their place.

pub mod dictzip;
pub mod lemma;
pub mod stardict;

use std::path::{Path, PathBuf};

use quick_xml::escape::escape;

pub use stardict::{Article, StarDict};

/// Articles gathered for one lookup, at most: a card has room for a few pages, not a book.
const MAX_ARTICLES: usize = 8;

/// One article found, with the dictionary it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub dict: String,
    pub article: Article,
}

#[derive(Default)]
pub struct Dictionaries {
    dicts: Vec<StarDict>,
}

impl Dictionaries {
    /// Opens every StarDict under `dirs` (two levels deep: KOReader's layout is one folder
    /// per dictionary). `cache_dir` keeps their index tables between runs.
    pub fn open(dirs: &[PathBuf], cache_dir: Option<&Path>) -> Dictionaries {
        let mut ifos = Vec::new();
        for d in dirs {
            find_ifos(d, 3, &mut ifos);
        }
        ifos.sort();
        let dicts = (ifos.iter())
            .filter_map(|p| StarDict::open(p, cache_dir).map_err(|e| eprintln!("dict {}: {e}", p.display())).ok())
            .collect();
        Dictionaries { dicts }
    }

    /// Builds (or reads back) every index table now, e.g. on a background thread at start.
    pub fn warm(&self) {
        let t = std::time::Instant::now();
        for d in &self.dicts {
            if let Err(e) = d.warm() {
                eprintln!("dict {}: {e}", d.name);
            }
        }
        eprintln!("dictionaries indexed in {:?}", t.elapsed());
    }

    pub fn is_empty(&self) -> bool {
        self.dicts.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.dicts.iter().map(|d| d.name.as_str())
    }

    /// The definitions of `word` as it appears in a book in `lang`: dictionaries of that
    /// language first, the others only when those find nothing. Within them, the first form
    /// of [`lemma::candidates`] that any of them knows wins.
    pub fn define(&self, word: &str, lang: Option<&str>) -> Vec<Definition> {
        let primary = lang.map(|l| l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase());
        let (mut matching, others): (Vec<_>, Vec<_>) =
            self.dicts.iter().partition(|d| primary.is_some() && d.lang.as_deref() == primary.as_deref());
        // Monolingual dictionaries first: a reader of Italian wants Italian definitions.
        matching.sort_by_key(|d| d.target != d.lang);
        // Other languages' dictionaries get both rule sets: the word may be a loan.
        for (group, forms) in [(matching, lemma::candidates(word, lang)), (others, lemma::candidates(word, None))] {
            for form in &forms {
                let mut out = Vec::new();
                for d in &group {
                    match d.lookup(form) {
                        Ok(articles) => out
                            .extend((articles.into_iter()).map(|article| Definition { dict: d.name.clone(), article })),
                        Err(e) => eprintln!("dict {}: {e}", d.name),
                    }
                    if out.len() >= MAX_ARTICLES {
                        break;
                    }
                }
                if !out.is_empty() {
                    out.truncate(MAX_ARTICLES);
                    return out;
                }
            }
        }
        Vec::new()
    }
}

fn find_ifos(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() && depth > 0 {
            find_ifos(&p, depth - 1, out);
        } else if p.extension().is_some_and(|x| x == "ifo") {
            out.push(p);
        }
    }
}

/// The languages of a dictionary's headwords and definitions: from a `xx-yy` file name
/// (KOReader's Wiktionary packs: `it-en`), else the first two languages named in its title
/// or file name ("English-English Wiktionary", "Wikizionario Italiano-Italiano").
// ponytail: a handful of language names in English and Italian; read the `lang=` key some
// newer .ifo files carry if dictionaries in other languages need it.
pub fn languages(stem: &str, title: &str) -> (Option<String>, Option<String>) {
    let code = |s: &str| (2..=3).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase());
    if let Some((a, b)) = stem.split_once('-') {
        if code(a) && code(b) {
            return (Some(a.into()), Some(b.into()));
        }
    }
    const NAMES: &[(&str, &str)] = &[
        ("english", "en"),
        ("inglese", "en"),
        ("italian", "it"),
        ("italiano", "it"),
        ("french", "fr"),
        ("francese", "fr"),
        ("german", "de"),
        ("tedesco", "de"),
        ("spanish", "es"),
        ("spagnolo", "es"),
    ];
    let title = format!("{title} {stem}").to_lowercase();
    let words = title.split(|c: char| !c.is_alphabetic());
    let mut found = words.filter_map(|w| NAMES.iter().find(|(n, _)| *n == w)).map(|(_, c)| c.to_string());
    (found.next(), found.next())
}

/// The definitions as one XHTML document for the layout engine: per article, the
/// dictionary's name, the headword, and the body (markup cleaned into XML, plain text split
/// into paragraphs).
pub fn to_xhtml(defs: &[Definition]) -> String {
    let mut out = String::from("<html><body>");
    let mut last_dict = None;
    for d in defs {
        if last_dict != Some(&d.dict) {
            // A heading, so it is never left alone at the bottom of a page.
            out.push_str(&format!("<h6 class=\"dict\">{}</h6>", escape(d.dict.as_str())));
            last_dict = Some(&d.dict);
        }
        out.push_str(&format!("<h1 class=\"hw\">{}</h1>", escape(d.article.headword.as_str())));
        if d.article.html {
            out.push_str(&crate::mobi::xhtml::fix(&d.article.body, true, None));
        } else {
            for para in d.article.body.split('\n').filter(|l| !l.trim().is_empty()) {
                out.push_str(&format!("<p>{}</p>", escape(para)));
            }
        }
    }
    out.push_str("</body></html>");
    out
}

#[cfg(test)]
mod tests {
    use super::stardict::tests::{temp, write_dict};
    use super::*;
    use crate::dom::Dom;

    #[test]
    fn picks_the_books_language_first_and_falls_back_to_others() {
        let dir = temp("set");
        write_dict(&dir.join("it-it"), "it-it", &[("casa", "<p>edificio</p>"), ("ritrovare", "<p>trovare</p>")], &[]);
        write_dict(
            &dir.join("en"),
            "English-English",
            &[("house", "<p>building</p>"), ("casa", "<p>a house</p>")],
            &[],
        );
        let d = Dictionaries::open(std::slice::from_ref(&dir), None);
        assert_eq!(d.names().count(), 2);
        let dicts = |w: &str, l: Option<&str>| d.define(w, l).into_iter().map(|d| d.dict).collect::<Vec<_>>();
        assert_eq!(dicts("casa", Some("it")), ["Test it-it"]);
        assert_eq!(dicts("casa", Some("en-US")), ["Test English-English"]);
        // Unknown language: everything that knows it.
        assert_eq!(dicts("casa", None).len(), 2);
        // Not in the Italian one: the English one answers.
        assert_eq!(dicts("houses", Some("it")), ["Test English-English"]);
        // Lemmatized: ritrovai → ritrovare.
        let r = d.define("Ritrovai", Some("it"));
        assert_eq!(r[0].article.headword, "ritrovare");
        assert!(d.define("nulla", Some("it")).is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn languages_from_names_and_titles() {
        let l = |stem, title| {
            let (a, b) = languages(stem, title);
            (a.unwrap_or_default(), b.unwrap_or_default())
        };
        assert_eq!(l("it-en", "Wiktionary Italian-English"), ("it".into(), "en".into()));
        assert_eq!(l("English-English Wiktionary dictionary", "x"), ("en".into(), "en".into()));
        assert_eq!(l("dict", "Wikizionario Italiano-Italiano"), ("it".into(), "it".into()));
        assert_eq!(l("dict", "Webster 1913"), (String::new(), String::new()));
    }

    #[test]
    fn definitions_become_well_formed_xhtml() {
        let art = |hw: &str, body: &str, html| Article { headword: hw.into(), body: body.into(), html };
        let defs = [
            Definition { dict: "A & B".into(), article: art("ran", "<i>verb</i><br><ol><li>past of run</ol>", true) },
            Definition { dict: "A & B".into(), article: art("run", "to move\nfast", false) },
        ];
        let x = to_xhtml(&defs);
        let dom = Dom::parse_xml(&x).expect("well-formed");
        assert_eq!(dom.find_all(Dom::ROOT, "h1").count(), 2);
        assert_eq!(dom.find_all(Dom::ROOT, "h6").count(), 1, "{x}"); // one dictionary label
        assert_eq!(dom.find_all(Dom::ROOT, "p").count(), 2, "{x}");
        assert!(x.contains("A &amp; B"));
    }

    /// The Wiktionary packs from `scripts/fetch-dicts.sh`, when present: index build, then
    /// lookup times (`--nocapture` prints them).
    #[test]
    fn real_dictionaries_answer_fast() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev-dicts"));
        if !dir.join("it-it").exists() {
            return;
        }
        let cache = temp("real");
        let t = std::time::Instant::now();
        let d = Dictionaries::open(&[dir.to_path_buf()], Some(&cache));
        eprintln!("opened {} dictionaries in {:?}", d.names().count(), t.elapsed());
        let words = [("ritrovai", "it"), ("Selva", "it"), ("smarrita", "it"), ("ran", "en"), ("houses", "en")];
        for pass in ["cold", "cached"] {
            for (w, l) in words {
                let t = std::time::Instant::now();
                let defs = d.define(w, Some(l));
                let heads: Vec<_> = defs.iter().map(|d| d.article.headword.as_str()).collect();
                eprintln!("{pass} {w}: {:?} {heads:?}", t.elapsed());
                assert!(!defs.is_empty(), "{w}");
            }
        }
        let t = std::time::Instant::now();
        let fresh = Dictionaries::open(&[dir.to_path_buf()], Some(&cache));
        fresh.define("parlavano", Some("it"));
        eprintln!("reopened + first lookup with cached tables: {:?}", t.elapsed());
        std::fs::remove_dir_all(cache).ok();
    }
}
