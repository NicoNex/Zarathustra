//! Plain-text books, turned into synthetic XHTML chapters so layout, positions and
//! highlights work as for an EPUB.

use std::fmt::Write;

use quick_xml::escape::escape;

use crate::epub::TocEntry;

/// A chapter grows past this many bytes of text only until the next paragraph ends.
const CHAPTER_BYTES: usize = 200 * 1024;

/// Archive-style path of synthetic chapter `i` (what `Book::chapters` and TOC targets hold).
pub fn chapter_path(i: usize) -> String {
    format!("text{i:04}.xhtml")
}

/// The file's XHTML chapters and a TOC of the headings it found.
pub fn chapters(bytes: &[u8]) -> (Vec<String>, Vec<TocEntry>) {
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s.trim_start_matches('\u{feff}').replace("\r\n", "\n"),
        // Anything that is not UTF-8 is most likely Windows-1252 (a superset of Latin-1's printables).
        Err(_) => crate::mobi::decode(bytes, 1252).replace("\r\n", "\n"),
    };
    let (mut chapters, mut toc, mut body) = (Vec::new(), Vec::new(), String::new());
    let flush = |body: &mut String, chapters: &mut Vec<String>| {
        if !body.is_empty() {
            chapters.push(format!(
                "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title></title></head><body>{body}</body></html>"
            ));
            body.clear();
        }
    };
    for block in paragraphs(&text) {
        if let Some(title) = heading(&block) {
            flush(&mut body, &mut chapters);
            toc.push(TocEntry { title: title.into(), target: chapter_path(chapters.len()), children: Vec::new() });
            let _ = write!(body, "<h2>{}</h2>", escape(title));
            continue;
        }
        let _ = write!(body, "<p>{}</p>", escape(block.as_str()));
        if body.len() > CHAPTER_BYTES {
            flush(&mut body, &mut chapters);
        }
    }
    if chapters.is_empty() && body.is_empty() {
        body.push_str("<p></p>"); // an empty file still opens, on one blank page
    }
    flush(&mut body, &mut chapters);
    (chapters, toc)
}

/// Paragraphs: blocks between blank lines, their hard-wrapped lines joined. A block with
/// long lines is one paragraph per line (files that only break at paragraph ends).
// ponytail: a poem's short lines get joined into one paragraph; keep breaks as <br/> if
// verse-heavy TXTs show up.
fn paragraphs(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split("\n\n").flat_map(|block| {
        let lines: Vec<&str> = block.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if lines.iter().any(|l| l.chars().count() > 100) {
            lines.into_iter().map(String::from).collect()
        } else if lines.is_empty() {
            Vec::new()
        } else {
            vec![lines.join(" ")]
        }
    })
}

/// A short line naming a numbered chapter ("Chapter 3", "CAPITOLO IV.", "Part II: The Sea").
// ponytail: spelled-out numbers ("Chapter One") are not headings; add a word list if such
// books show up.
fn heading(par: &str) -> Option<&str> {
    const WORDS: [&str; 8] = ["chapter", "chapitre", "capitolo", "kapitel", "capítulo", "part", "parte", "book"];
    if par.len() > 240 {
        return None; // longer than 60 characters of up to 4 bytes: a paragraph
    }
    let mut words = par.split_whitespace();
    let first = words.next()?.to_lowercase();
    let number = words.next()?.trim_end_matches(['.', ':']);
    let numeric = !number.is_empty()
        && (number.bytes().all(|b| b.is_ascii_digit()) || number.bytes().all(|b| b"IVXLC".contains(&b)));
    (par.chars().count() <= 60 && numeric && WORDS.contains(&first.as_str())).then_some(par)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Dom;

    #[test]
    fn splits_on_headings_and_joins_wrapped_lines() {
        let src = "Preface line one\nline two.\r\n\r\nChapter 1\n\nIt was <dark> & stormy.\n\nCHAPTER II.\n\nEnd.";
        let (chapters, toc) = chapters(src.as_bytes());
        assert_eq!(chapters.len(), 3);
        assert!(chapters[0].contains("<p>Preface line one line two.</p>"), "{}", chapters[0]);
        assert!(chapters[1].contains("<h2>Chapter 1</h2><p>It was &lt;dark&gt; &amp; stormy.</p>"));
        let titles: Vec<_> = toc.iter().map(|e| (e.title.as_str(), e.target.as_str())).collect();
        assert_eq!(titles, [("Chapter 1", "text0001.xhtml"), ("CHAPTER II.", "text0002.xhtml")]);
        assert!(chapters.iter().all(|c| Dom::parse_xml(c).is_ok()));
    }

    #[test]
    fn falls_back_to_windows_1252_and_caps_chapter_size() {
        let (chapters, _) = chapters(b"caf\xe9 \x93q\x94");
        assert!(chapters[0].contains("café “q”"), "{}", chapters[0]);
        let long = "word ".repeat(20).trim_end().to_string() + "\n\n";
        let (chapters, _) = super::chapters(long.repeat(5000).as_bytes());
        assert!(chapters.len() > 1 && chapters.iter().all(|c| c.len() < CHAPTER_BYTES + 1024));
    }
}
