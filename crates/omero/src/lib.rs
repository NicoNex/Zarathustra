//! The Omero shell: home, reader and the state they share.

pub mod about;
pub mod bookmenu;
pub mod clock;
pub mod dict;
pub mod footnote;
pub mod highlight;
pub mod home;
pub mod i18n;
pub mod keyboard;
pub mod menu;
pub mod nav;
pub mod notebook;
pub mod pace;
pub mod pdfview;
pub mod quick;
pub mod reader;
pub mod search;
pub mod settings;
pub mod sleep;
pub mod stats;
pub mod toc;
pub mod ui;

use std::path::{Path, PathBuf};

use omero_engine::font::{Face, Rasterizer};
use omero_engine::fonts::FontLibrary;
use omero_engine::layout::hyphen::Hyphenators;
use omero_engine::layout::paragraph::Typesetter;

/// Screen size the layouts are designed for (the Kindle Paperwhite 5).
pub const W: i32 = 1236;
pub const H: i32 = 1648;

/// Device paths first, then the repo's dev assets on a desktop.
pub fn first_existing(candidates: &[&str]) -> PathBuf {
    let repo = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
    (candidates.iter())
        .map(|c| if c.starts_with('/') { PathBuf::from(c) } else { Path::new(repo).join(c) })
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(candidates[0]))
}

/// The reading font, regular or bold; a free function so drawing code can hold the
/// rasterizer mutably next to it.
pub fn serif(fonts: &FontLibrary, bold: bool) -> Option<&Face> {
    fonts.pick(&["serif".into()], if bold { 700 } else { 400 }, false)
}

/// The UI font, regular or bold.
pub fn sans(fonts: &FontLibrary, bold: bool) -> Option<&Face> {
    fonts.pick(&["sans-serif".into()], if bold { 700 } else { 400 }, false)
}

/// Long-lived resources every screen draws with.
pub struct Ctx {
    pub fonts: FontLibrary,
    pub ts: Typesetter,
    pub raster: Rasterizer,
    /// Per-book reading positions.
    pub state_dir: PathBuf,
}

impl Ctx {
    pub fn new() -> Ctx {
        let mut fonts = FontLibrary::new("Bookerly");
        for dir in ["/mnt/us/fonts", "/usr/java/lib/fonts"] {
            fonts.add_dir(Path::new(dir));
        }
        fonts.add_dir(&first_existing(&["dev-fonts"]));
        let ts = Typesetter::new(Hyphenators::new(first_existing(&["/mnt/us/omero/hyph", "assets/hyphenation"])));
        let state_dir = if Path::new("/mnt/us").exists() {
            PathBuf::from("/mnt/us/omero/state")
        } else {
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/omero-state")).to_path_buf()
        };
        std::fs::create_dir_all(&state_dir).ok();
        i18n::set_lang(settings::Settings::load(&state_dir).lang);
        Ctx { fonts, ts, raster: Rasterizer::new(1.3), state_dir }
    }

    /// The reading font (also used for book titles).
    pub fn serif(&self, bold: bool) -> Option<&Face> {
        serif(&self.fonts, bold)
    }

    /// The UI font.
    pub fn sans(&self, bold: bool) -> Option<&Face> {
        sans(&self.fonts, bold)
    }

    /// Where a book's reading position is saved: keyed by the full path, so books with the
    /// same file name in different folders don't share it.
    pub fn state_file(&self, book: &Path) -> PathBuf {
        let stem = book.file_stem().map_or_else(|| "book".into(), |s| s.to_string_lossy().into_owned());
        // FNV-1a: stable across Rust versions, unlike the std hasher.
        let hash = book
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
        let file = self.state_dir.join(format!("{stem}-{hash:016x}.pos"));
        // ponytail: migrates the old stem-only name; drop once devices have opened their books.
        let old = self.state_dir.join(format!("{stem}.pos"));
        if !file.exists() && old.exists() {
            std::fs::rename(&old, &file).ok();
        }
        file
    }
}

/// Replaces `path` with `data` so a crash or power loss leaves the old or the new file, never
/// a truncated one: write a temp file, flush it to disk, rename it over.
pub fn write_atomic(path: &Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(data.as_ref())?;
    f.sync_all()?;
    std::fs::rename(tmp, path)
}

impl Default for Ctx {
    fn default() -> Self {
        Ctx::new()
    }
}

/// What a screen wants after handling a tap.
pub enum Action {
    None,
    /// Redraw the screen; `turn` is a page turn (forward?) for the swipe animation.
    Redraw {
        turn: Option<bool>,
    },
    /// Redraw the screen but refresh only this area (popups, highlights).
    Region(omero_device::Rect),
    Open(PathBuf),
    /// Open the book on a saved highlight.
    OpenAt(PathBuf, Box<omero_store::AnnotationData>),
    Home,
    Quit,
}
