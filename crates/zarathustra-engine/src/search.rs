//! Full-text search inside a book: case-, accent- and ligature-insensitive substring search
//! over a chapter's [`TextMap`](crate::anchor::TextMap) text, with snippets for a result list.
//!
//! Search runs on the chapter text, not on laid-out pages: parsing a chapter is cheap, laying
//! it out is not. A hit is a byte range in the `TextMap` text, which the reader turns into a
//! page only when the result is opened.

use std::ops::Range;

use memchr::memmem::Finder;

/// The base letter of a lowercase accented Latin letter (Latin-1 and Latin Extended-A).
fn base_letter(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' | 'ð' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => 's',
        'ţ' | 'ť' | 'ŧ' | 'ț' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        _ => c,
    }
}

/// Appends the search form of `c` to `out`; `space` tracks whether `out` ends in a space, so
/// whitespace runs (no-break and thin spaces included) fold to one space.
// ponytail: a hand table for the Latin alphabets the user reads (Italian, English, the rest
// of Western and Central Europe). Use Unicode decomposition (icu_normalizer) when Greek,
// Cyrillic or Vietnamese books show up.
fn fold_char(c: char, out: &mut String, space: &mut bool) {
    if c.is_whitespace() {
        if !*space {
            out.push(' ');
            *space = true;
        }
        return;
    }
    let folded: &str = match c {
        // Soft hyphen, zero-width spaces and joiners, word joiner, BOM: invisible.
        '\u{ad}' | '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}' => return,
        // Combining diacritics (decomposed text): the base letter is already out.
        '\u{300}'..='\u{36f}' => return,
        '\u{2018}' | '\u{2019}' | '\u{201b}' | '\u{2032}' | '`' | '\u{b4}' => "'",
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{ab}' | '\u{bb}' => "\"",
        'ﬀ' => "ff",
        'ﬁ' => "fi",
        'ﬂ' => "fl",
        'ﬃ' => "ffi",
        'ﬄ' => "ffl",
        'ﬅ' | 'ﬆ' => "st",
        'æ' | 'Æ' => "ae",
        'œ' | 'Œ' => "oe",
        'ß' | 'ẞ' => "ss",
        c if c.is_ascii() => {
            out.push(c.to_ascii_lowercase());
            *space = false;
            return;
        }
        c => {
            for l in c.to_lowercase() {
                match l {
                    '\u{300}'..='\u{36f}' => {} // 'İ' lowercases to 'i' plus a combining dot
                    'þ' => out.push_str("th"),
                    l => out.push(base_letter(l)),
                }
            }
            *space = false;
            return;
        }
    };
    out.push_str(folded);
    *space = false;
}

/// The search form of `s`: lowercase, accents and ligatures folded, whitespace collapsed.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        fold_char(c, &mut out, &mut space);
    }
    out
}

/// A text in search form, with where each of its bytes came from.
struct Folded {
    text: String,
    /// Source byte offset of the character each folded byte comes from, plus the source
    /// length at the end.
    from: Vec<u32>,
}

impl Folded {
    fn new(s: &str) -> Folded {
        let mut text = String::with_capacity(s.len());
        let mut from = Vec::with_capacity(s.len() + 1);
        let mut space = false;
        for (i, c) in s.char_indices() {
            fold_char(c, &mut text, &mut space);
            from.resize(text.len(), i as u32);
        }
        from.push(s.len() as u32);
        Folded { text, from }
    }

    /// Source range of folded byte range `r`.
    fn source(&self, r: Range<usize>) -> Range<usize> {
        self.from[r.start] as usize..self.from[r.end] as usize
    }
}

/// What the user searches for, in search form.
pub struct Query {
    folded: String,
}

impl Query {
    /// `None` for a query with nothing to look for (empty or blank).
    pub fn new(text: &str) -> Option<Query> {
        let folded = fold(text).trim().to_string();
        (!folded.is_empty()).then_some(Query { folded })
    }

    /// Byte ranges of the matches in `text`, in order and not overlapping, at most `limit`.
    pub fn find(&self, text: &str, limit: usize) -> Vec<Range<usize>> {
        // Most chapters have no match: a cheap pass over the plain lowercase text could skip
        // the fold, but folding is a single linear pass already (see the timing test).
        let folded = Folded::new(text);
        let finder = Finder::new(self.folded.as_bytes());
        (finder.find_iter(folded.text.as_bytes()))
            .take(limit)
            .map(|at| folded.source(at..at + self.folded.len()))
            .collect()
    }
}

/// A line of context around `range` of `text`: about `before` characters before it and
/// `after` after it, whitespace collapsed, cut at word boundaries with an ellipsis. Returns the
/// snippet and where the match lies in it.
pub fn snippet(text: &str, range: Range<usize>, before: usize, after: usize) -> (String, Range<usize>) {
    let start = text.floor_char_boundary(range.start.min(text.len()));
    let end = text.ceil_char_boundary(range.end.clamp(start, text.len()));
    let mut lo = (text[..start].char_indices().rev().nth(before)).map_or(0, |(i, _)| i);
    let mut hi = (text[end..].char_indices().nth(after)).map_or(text.len(), |(i, _)| end + i);
    // Cut at the nearest whitespace inside the window, so no word shows half.
    if lo > 0 {
        lo = text[lo..start].find(char::is_whitespace).map_or(lo, |i| lo + i);
    }
    if hi < text.len() {
        hi = text[end..hi].rfind(char::is_whitespace).map_or(hi, |i| end + i);
    }
    let mut out = String::new();
    if lo > 0 {
        out.push('…');
    }
    // No leading space, except after the ellipsis.
    let mut space = lo == 0;
    let mut push = |s: &str, out: &mut String| {
        for c in s.chars().filter(|&c| c != '\u{ad}') {
            if c.is_whitespace() {
                if !space {
                    out.push(' ');
                }
                space = true;
            } else {
                out.push(c);
                space = false;
            }
        }
    };
    push(&text[lo..start], &mut out);
    let bold_start = out.len();
    push(&text[start..end], &mut out);
    let bold = bold_start..out.len();
    push(&text[end..hi], &mut out);
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    if hi < text.len() {
        out.push('…');
    }
    (out, bold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_accents_ligatures_and_spaces() {
        assert_eq!(fold("Perché  PIÙ\u{a0}caffè"), "perche piu caffe");
        assert_eq!(fold("ﬁne ﬂusso Œuvre straße"), "fine flusso oeuvre strasse");
        assert_eq!(fold("l’amore\u{2009}—\nè"), "l'amore — e");
        assert_eq!(fold("cafe\u{301}"), "cafe", "decomposed accent");
        assert_eq!(fold("in\u{ad}fi\u{200b}nito"), "infinito");
        assert_eq!(fold("ŁÓDŹ Ærø"), "lodz aero");
    }

    #[test]
    fn finds_matches_in_source_offsets() {
        let text = "Il PERCHÉ del perche: ﬁne.\n  La  fine.";
        let q = Query::new("  perché ").expect("query");
        let hits = q.find(text, 10);
        let words: Vec<_> = hits.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(words, ["PERCHÉ", "perche"]);
        let fine = Query::new("FINE").expect("query").find(text, 10);
        assert_eq!(fine.iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(), ["ﬁne", "fine"]);
        // Across a line break and a run of spaces.
        let la = Query::new("fine. la fine").expect("query").find(text, 10);
        assert_eq!(&text[la[0].clone()], "ﬁne.\n  La  fine");
        assert_eq!(Query::new("perche").expect("query").find(text, 1).len(), 1, "limit");
        assert!(Query::new(" \u{a0} ").is_none());
    }

    #[test]
    fn snippets_cut_at_words_and_mark_the_match() {
        let text = "Nel mezzo del cammin di nostra vita mi ritrovai per una selva oscura";
        let at = text.find("nostra").expect("in text");
        let (s, bold) = snippet(text, at..at + 6, 12, 14);
        assert_eq!(&s[bold.clone()], "nostra");
        assert_eq!(s, "… cammin di nostra vita mi…");
        let (s, bold) = snippet("  a\n\n  vita  ", 7..11, 40, 40);
        assert_eq!((s.as_str(), &s[bold]), ("a vita", "vita"));
    }
}

#[cfg(test)]
mod book_tests {
    use std::path::Path;
    use std::time::Instant;

    use super::*;
    use crate::anchor::TextMap;
    use crate::book::Book;

    /// Searches every local book end to end and prints the time (run with `--nocapture`).
    /// Hits are real text: each one folds to the query.
    #[test]
    fn dev_books_search_fast() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let books = ["dev-books", "dev-books/azw3"].iter().filter_map(|d| std::fs::read_dir(root.join(d)).ok());
        let q = Query::new("che").expect("query");
        for path in books.flatten().flatten().map(|e| e.path()).filter(|p| p.is_file()) {
            let t = Instant::now();
            let Ok(book) = Book::open(&path) else { continue };
            let opened = t.elapsed();
            let (mut hits, mut bytes, mut first) = (0, 0, None);
            for i in 0..book.chapters.len() {
                let Ok(dom) = book.chapter_dom(i) else { continue };
                let map = TextMap::new(&dom);
                bytes += map.text().len();
                for r in q.find(map.text(), usize::MAX) {
                    assert_eq!(fold(&map.text()[r.clone()]), "che", "{}", &map.text()[r]);
                    hits += 1;
                    first.get_or_insert(t.elapsed());
                }
            }
            eprintln!(
                "{}: open {opened:?}, first hit {first:?}, {} chapters, {} KB text, {hits} hits in {:?}",
                path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
                book.chapters.len(),
                bytes / 1024,
                t.elapsed()
            );
        }
    }
}
