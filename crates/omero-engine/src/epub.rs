//! EPUB 2/3 container: metadata, spine, table of contents and cover.

use std::collections::HashMap;
use std::io::{Read, Seek};

use percent_encoding::percent_decode_str;
use zip::ZipArchive;

use crate::dom::{Dom, NodeId, ParseError};

#[derive(Debug, thiserror::Error)]
pub enum EpubError {
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{path}: {source}")]
    Xml { path: String, source: ParseError },
    #[error("{path} is not UTF-8")]
    Encoding { path: String },
    #[error("missing {0}")]
    Missing(&'static str),
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Metadata {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f32>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpineItem {
    /// Archive path of the chapter.
    pub path: String,
    pub media_type: String,
    pub linear: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TocEntry {
    pub title: String,
    /// Archive path, plus `#fragment` when present.
    pub target: String,
    pub children: Vec<TocEntry>,
}

#[derive(Debug, Clone)]
struct ManifestItem {
    id: String,
    path: String,
    media_type: String,
    properties: String,
}

pub struct Epub<R> {
    archive: ZipArchive<R>,
    pub metadata: Metadata,
    pub spine: Vec<SpineItem>,
    pub toc: Vec<TocEntry>,
    /// Archive path of the cover image.
    pub cover: Option<String>,
}

impl<R: Read + Seek> Epub<R> {
    pub fn open(reader: R) -> Result<Self, EpubError> {
        let mut archive = ZipArchive::new(reader)?;
        let container = parse(&mut archive, "META-INF/container.xml")?;
        let opf_path = container
            .find(Dom::ROOT, "rootfile")
            .and_then(|n| container.attr(n, "full-path"))
            .ok_or(EpubError::Missing("rootfile in container.xml"))?
            .to_string();
        let opf = parse(&mut archive, &opf_path)?;

        // Document order matters for the fallbacks below (first match wins, deterministically).
        let manifest: Vec<ManifestItem> = opf
            .find_all(Dom::ROOT, "item")
            .filter_map(|n| {
                Some(ManifestItem {
                    id: opf.attr(n, "id")?.to_string(),
                    path: resolve(&opf_path, opf.attr(n, "href")?),
                    media_type: opf.attr(n, "media-type").unwrap_or_default().to_string(),
                    properties: opf.attr(n, "properties").unwrap_or_default().to_string(),
                })
            })
            .collect();
        let by_id: HashMap<&str, &ManifestItem> = manifest.iter().map(|i| (i.id.as_str(), i)).collect();

        let spine_node = opf.find(Dom::ROOT, "spine").ok_or(EpubError::Missing("spine"))?;
        let spine = opf
            .find_all(spine_node, "itemref")
            .filter_map(|n| {
                let item = by_id.get(opf.attr(n, "idref")?)?;
                Some(SpineItem {
                    path: item.path.clone(),
                    media_type: item.media_type.clone(),
                    linear: opf.attr(n, "linear") != Some("no"),
                })
            })
            .collect();

        let metadata = read_metadata(&opf);
        let cover = find_cover(&opf, &manifest, &by_id);

        let nav = manifest.iter().find(|i| has_token(&i.properties, "nav")).map(|i| i.path.clone());
        let ncx = opf
            .attr(spine_node, "toc")
            .and_then(|id| by_id.get(id).copied())
            .or_else(|| manifest.iter().find(|i| i.media_type == "application/x-dtbncx+xml"))
            .map(|i| i.path.clone());
        let mut toc = Vec::new();
        // ponytail: a broken nav/NCX just leaves the TOC empty; the book still opens.
        if let Some(path) = &nav {
            if let Ok(dom) = parse(&mut archive, path) {
                toc = nav_toc(&dom, path);
            }
        }
        if toc.is_empty() {
            if let Some(path) = &ncx {
                if let Ok(dom) = parse(&mut archive, path) {
                    toc = ncx_toc(&dom, dom.find(Dom::ROOT, "navMap").unwrap_or(Dom::ROOT), path);
                }
            }
        }

        Ok(Epub { archive, metadata, spine, toc, cover })
    }

    /// Raw bytes of an archive member (chapter, image, stylesheet, font).
    pub fn read(&mut self, path: &str) -> Result<Vec<u8>, EpubError> {
        read(&mut self.archive, path)
    }

    pub fn read_dom(&mut self, path: &str) -> Result<Dom, EpubError> {
        parse(&mut self.archive, path)
    }
}

fn read<R: Read + Seek>(archive: &mut ZipArchive<R>, path: &str) -> Result<Vec<u8>, EpubError> {
    let file = archive.by_name(path)?;
    // The header's size is untrusted: cap both the reservation and the bytes read.
    let mut buf = Vec::with_capacity(file.size().min(16 << 20) as usize);
    file.take(MAX_MEMBER).read_to_end(&mut buf)?;
    Ok(buf)
}

fn parse<R: Read + Seek>(archive: &mut ZipArchive<R>, path: &str) -> Result<Dom, EpubError> {
    let bytes = read(archive, path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| EpubError::Encoding { path: path.into() })?;
    Dom::parse_xml(text).map_err(|source| EpubError::Xml { path: path.into(), source })
}

/// Largest archive member we read into memory (big images, embedded fonts).
const MAX_MEMBER: u64 = 64 << 20;

/// Resolves `href` (relative to the file at `base`) to a normalized archive path.
/// External URLs (`https:`, `mailto:`…) are returned unchanged.
pub fn resolve(base: &str, href: &str) -> String {
    if href.split_once(':').is_some_and(|(scheme, _)| scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+')) {
        return href.to_string();
    }
    let (href, fragment) = href.split_once('#').map_or((href, None), |(h, f)| (h, Some(f)));
    let href = percent_decode_str(href).decode_utf8_lossy();
    if href.is_empty() {
        // A bare `#fragment` points into the base document itself.
        return fragment.map_or_else(|| base.to_string(), |f| format!("{base}#{f}"));
    }
    let mut parts: Vec<&str> = match base.rsplit_once('/') {
        Some((dir, _)) if !href.starts_with('/') => dir.split('/').collect(),
        _ => Vec::new(),
    };
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut path = parts.join("/");
    if let Some(f) = fragment {
        path.push('#');
        path.push_str(f);
    }
    path
}

fn has_token(list: &str, token: &str) -> bool {
    list.split_ascii_whitespace().any(|t| t == token)
}

fn read_metadata(opf: &Dom) -> Metadata {
    let Some(meta) = opf.find(Dom::ROOT, "metadata") else { return Metadata::default() };
    let first = |name| opf.find_all(meta, name).map(|n| opf.text(n)).find(|t| !t.is_empty());
    let meta_content = |key: &str| {
        opf.find_all(meta, "meta").find(|&n| opf.attr(n, "name") == Some(key)).and_then(|n| opf.attr(n, "content"))
    };
    // EPUB3 series: <meta property="belongs-to-collection" id="c">Name</meta>
    //               <meta refines="#c" property="group-position">2</meta>
    let collection = opf.find_all(meta, "meta").find(|&n| opf.attr(n, "property") == Some("belongs-to-collection"));
    let position = collection.and_then(|c| {
        let id = format!("#{}", opf.attr(c, "id")?);
        let n = opf
            .find_all(meta, "meta")
            .find(|&n| opf.attr(n, "refines") == Some(&id) && opf.attr(n, "property") == Some("group-position"))?;
        opf.text(n).parse().ok()
    });
    Metadata {
        title: first("title"),
        // Illustrators, editors, translators (opf:role other than "aut") are not authors.
        // ponytail: EPUB3 `refines … property="role"` is not read yet.
        authors: opf
            .find_all(meta, "creator")
            .filter(|&n| opf.attr(n, "role").is_none_or(|r| r == "aut"))
            .map(|n| opf.text(n))
            .filter(|t| !t.is_empty())
            .collect(),
        language: first("language"),
        identifier: first("identifier"),
        series: collection.map(|n| opf.text(n)).or_else(|| meta_content("calibre:series").map(String::from)),
        series_index: position.or_else(|| meta_content("calibre:series_index")?.parse().ok()),
        description: first("description"),
    }
}

fn find_cover(opf: &Dom, manifest: &[ManifestItem], by_id: &HashMap<&str, &ManifestItem>) -> Option<String> {
    let is_image = |i: &&ManifestItem| i.media_type.starts_with("image/");
    // EPUB3 property, then EPUB2 <meta name="cover" content="id">, then a manifest id or file name hint.
    manifest
        .iter()
        .find(|i| has_token(&i.properties, "cover-image"))
        .or_else(|| {
            let meta = opf.find_all(Dom::ROOT, "meta").find(|&n| opf.attr(n, "name") == Some("cover"))?;
            by_id.get(opf.attr(meta, "content")?).copied().filter(is_image)
        })
        .or_else(|| manifest.iter().filter(is_image).find(|i| i.id.contains("cover")))
        .or_else(|| manifest.iter().filter(is_image).find(|i| i.path.to_ascii_lowercase().contains("cover")))
        .map(|i| i.path.clone())
}

/// EPUB3 navigation document: the `<nav epub:type="toc">` ordered list.
fn nav_toc(dom: &Dom, path: &str) -> Vec<TocEntry> {
    let nav = dom
        .find_all(Dom::ROOT, "nav")
        .find(|&n| dom.attr(n, "epub:type").is_some_and(|t| has_token(t, "toc")))
        .or_else(|| dom.find(Dom::ROOT, "nav"));
    let list = nav.and_then(|n| dom.children(n).find(|&c| dom.name(c) == Some("ol")));
    list.map(|ol| nav_list(dom, ol, path)).unwrap_or_default()
}

fn nav_list(dom: &Dom, ol: NodeId, path: &str) -> Vec<TocEntry> {
    dom.children(ol)
        .filter(|&li| dom.name(li) == Some("li"))
        .filter_map(|li| {
            let label = dom.children(li).find(|&c| matches!(dom.name(c), Some("a" | "span")))?;
            Some(TocEntry {
                title: dom.text(label),
                target: dom.attr(label, "href").map(|h| resolve(path, h)).unwrap_or_default(),
                children: dom
                    .children(li)
                    .find(|&c| dom.name(c) == Some("ol"))
                    .map(|sub| nav_list(dom, sub, path))
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// EPUB2 NCX: nested `<navPoint>`s.
fn ncx_toc(dom: &Dom, parent: NodeId, path: &str) -> Vec<TocEntry> {
    dom.children(parent)
        .filter(|&n| dom.name(n) == Some("navPoint"))
        .map(|np| TocEntry {
            title: dom.children(np).find(|&c| dom.name(c) == Some("navLabel")).map(|l| dom.text(l)).unwrap_or_default(),
            target: dom
                .children(np)
                .find(|&c| dom.name(c) == Some("content"))
                .and_then(|c| dom.attr(c, "src"))
                .map(|s| resolve(path, s))
                .unwrap_or_default(),
            children: ncx_toc(dom, np, path),
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::{SimpleFileOptions, ZipWriter};

    /// Builds an in-memory EPUB from (path, content) pairs.
    pub(crate) fn epub(files: &[(&str, &str)]) -> Cursor<Vec<u8>> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (path, content) in files {
            zip.start_file(*path, SimpleFileOptions::default()).expect("zip entry");
            zip.write_all(content.as_bytes()).expect("zip write");
        }
        let mut cursor = zip.finish().expect("zip finish");
        cursor.set_position(0);
        cursor
    }

    const CONTAINER: &str = r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>
        <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

    #[test]
    fn reads_epub3_metadata_spine_nav_and_cover() {
        let opf = r##"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
            <dc:title>Moby-Dick</dc:title><dc:creator>Herman Melville</dc:creator><dc:language>en</dc:language>
            <meta property="belongs-to-collection" id="c1">Sea Stories</meta>
            <meta refines="#c1" property="group-position">2</meta></metadata>
          <manifest>
            <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
            <item id="c1" href="text/ch%201.xhtml" media-type="application/xhtml+xml"/>
            <item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
            <item id="img" href="images/front.jpg" media-type="image/jpeg" properties="cover-image"/>
          </manifest>
          <spine><itemref idref="c1"/><itemref idref="c2" linear="no"/></spine></package>"##;
        let nav = r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body>
            <nav epub:type="landmarks"><ol><li><a href="text/ch2.xhtml">Skip</a></li></ol></nav>
            <nav epub:type="toc"><ol>
              <li><a href="text/ch%201.xhtml">Loomings</a><ol><li><a href="text/ch%201.xhtml#p2">Part&#160;two</a></li></ol></li>
              <li><span>Untitled</span></li></ol></nav></body></html>"#;
        let book = Epub::open(epub(&[
            ("META-INF/container.xml", CONTAINER),
            ("OEBPS/content.opf", opf),
            ("OEBPS/nav.xhtml", nav),
        ]))
        .expect("opens");
        let m = &book.metadata;
        assert_eq!(m.title.as_deref(), Some("Moby-Dick"));
        assert_eq!(m.authors, ["Herman Melville"]);
        assert_eq!((m.series.as_deref(), m.series_index), (Some("Sea Stories"), Some(2.0)));
        let paths: Vec<_> = book.spine.iter().map(|s| (s.path.as_str(), s.linear)).collect();
        assert_eq!(paths, [("OEBPS/text/ch 1.xhtml", true), ("OEBPS/text/ch2.xhtml", false)]);
        assert_eq!(book.cover.as_deref(), Some("OEBPS/images/front.jpg"));
        assert_eq!(book.toc.len(), 2);
        assert_eq!(book.toc[0].title, "Loomings");
        assert_eq!(book.toc[0].children[0].target, "OEBPS/text/ch 1.xhtml#p2");
        assert_eq!(book.toc[1].target, "");
    }

    #[test]
    fn reads_epub2_ncx_and_meta_cover() {
        let opf = r#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata>
            <dc:title xmlns:dc="x">Libro</dc:title><meta name="cover" content="cov"/>
            <meta name="calibre:series" content="Saga"/><meta name="calibre:series_index" content="3"/></metadata>
          <manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
            <item id="ch" href="ch.html" media-type="application/xhtml+xml"/>
            <item id="cov" href="c.png" media-type="image/png"/></manifest>
          <spine toc="ncx"><itemref idref="ch"/></spine></package>"#;
        let ncx = r#"<ncx><navMap><navPoint id="a"><navLabel><text>Uno</text></navLabel><content src="ch.html#x"/>
            <navPoint id="b"><navLabel><text>Due</text></navLabel><content src="ch.html#y"/></navPoint></navPoint></navMap></ncx>"#;
        let book = Epub::open(epub(&[
            ("META-INF/container.xml", CONTAINER.replace("OEBPS/", "").as_str()),
            ("content.opf", opf),
            ("toc.ncx", ncx),
        ]))
        .expect("opens");
        assert_eq!((book.metadata.series.as_deref(), book.metadata.series_index), (Some("Saga"), Some(3.0)));
        assert_eq!(book.cover.as_deref(), Some("c.png"));
        assert_eq!(book.toc[0].title, "Uno");
        assert_eq!(book.toc[0].children[0].target, "ch.html#y");
    }

    #[test]
    fn resolve_normalizes_relative_paths() {
        assert_eq!(resolve("OEBPS/text/a.xhtml", "../img/b.png"), "OEBPS/img/b.png");
        assert_eq!(resolve("content.opf", "x%20y.html#f"), "x y.html#f");
        assert_eq!(resolve("a/b.opf", "/root.css"), "root.css");
        assert_eq!(resolve("OEBPS/nav.xhtml", "#ch3"), "OEBPS/nav.xhtml#ch3");
        assert_eq!(resolve("OEBPS/nav.xhtml", "https://example.com/x"), "https://example.com/x");
    }
}

#[cfg(test)]
mod real_books {
    use super::*;

    /// Opens every EPUB in `dev-books/` (local only, not in the repo) and checks the basics.
    #[test]
    fn opens_local_books() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev-books");
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for path in
            entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "epub"))
        {
            let mut book = Epub::open(std::fs::File::open(&path).expect("file")).expect("opens");
            assert!(!book.spine.is_empty(), "{path:?}: empty spine");
            let first = book.spine[0].path.clone();
            book.read_dom(&first).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            eprintln!(
                "{:?}: {:?} by {:?}, {} chapters, {} toc entries, cover {:?}",
                path.file_name().unwrap_or_default(),
                book.metadata.title,
                book.metadata.authors,
                book.spine.len(),
                book.toc.len(),
                book.cover
            );
        }
    }
}
