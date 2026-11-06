//! The Zarathustra shell: home, reader and the state they share.

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
pub mod prefs;
pub mod quick;
pub mod reader;
pub mod search;
pub mod settings;
pub mod sleep;
pub mod stats;
pub mod toc;
pub mod transfer;
pub mod ui;

use std::path::{Path, PathBuf};

use zarathustra_engine::font::{Face, Rasterizer};
use zarathustra_engine::fonts::FontLibrary;
use zarathustra_engine::layout::hyphen::Hyphenators;
use zarathustra_engine::layout::paragraph::Typesetter;

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
        let ts = Typesetter::new(Hyphenators::new(first_existing(&["/mnt/us/zarathustra/hyph", "assets/hyphenation"])));
        let state_dir = if Path::new("/mnt/us").exists() {
            PathBuf::from("/mnt/us/zarathustra/state")
        } else {
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/zarathustra-state")).to_path_buf()
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
        if file.exists() {
            return file;
        }
        // A book moved to another folder (documents/ → library/) keeps its place: the one
        // position saved under the same file name, from the old stem-only name or another path.
        // ponytail: a read of the state dir per book never opened; key positions by the book's
        // identifier if that shows up at startup on a big library.
        let prefix = format!("{stem}-");
        let candidates: Vec<PathBuf> = std::fs::read_dir(&self.state_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                name == format!("{stem}.pos")
                    || (name.starts_with(&prefix) && name.ends_with(".pos") && name.len() == prefix.len() + 20)
            })
            .collect();
        if let [old] = candidates.as_slice() {
            std::fs::rename(old, &file).ok();
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

/// What an opened book shows first.
pub enum Landing {
    /// The saved position.
    Resume,
    /// The page of a saved highlight or bookmark.
    At(Box<zarathustra_store::AnnotationData>),
    Contents,
    Bookmarks,
    Search,
}

/// What a screen wants after handling a tap.
pub enum Action {
    None,
    /// Redraw the screen; `turn` is a page turn (forward?) for the swipe animation.
    Redraw {
        turn: Option<bool>,
    },
    /// Redraw the screen but refresh only this area (popups, highlights).
    Region(zarathustra_device::Rect),
    /// Open the book, showing `Landing` first.
    Open(PathBuf, Landing),
    /// The settings screen changed a setting: hand it to the quick panel, which owns them.
    Settings(settings::Settings),
    Home,
    Quit,
    /// Standby (the sleep screen), as the power button does.
    Sleep,
    /// Restart or power off the device.
    Reboot,
    PowerOff,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moved_book_keeps_its_position() {
        let dir = std::env::temp_dir().join(format!("zarathustra-pos-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let mut ctx = Ctx::new();
        ctx.state_dir = dir.clone();
        let old = ctx.state_file(Path::new("/mnt/us/documents/Libro.epub"));
        std::fs::write(&old, "3 0 0.5000 0\n").expect("write");
        let new = ctx.state_file(Path::new("/mnt/us/library/Libro.epub"));
        assert_ne!(old, new);
        assert!(new.exists() && !old.exists(), "the position moved with the book");
        assert!(!ctx.state_file(Path::new("/mnt/us/library/Altro.epub")).exists());
        std::fs::remove_dir_all(dir).ok();
    }
}
