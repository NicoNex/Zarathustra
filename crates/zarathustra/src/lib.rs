//! The Zarathustra shell: home, reader and the state they share.

pub mod about;
pub mod bookmenu;
pub mod clock;
pub mod dict;
pub mod extensions;
pub mod footnote;
pub mod highlight;
pub mod home;
pub mod i18n;
pub mod keyboard;
pub mod menu;
pub mod nav;
pub mod notebook;
pub mod outsheet;
pub mod pace;
pub mod pdfmarks;
pub mod pdfview;
pub mod plugin;
pub mod plugin_i18n;
pub mod plugin_page;
pub mod prefs;
pub mod quick;
pub mod reader;
pub mod search;
pub mod settings;
pub mod sleep;
pub mod stats;
pub mod toc;
pub mod transfer;
pub mod typing;
pub mod ui;
pub mod wifi;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

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

/// Running on the Kindle (its userstore is mounted), not a desktop.
pub fn on_device() -> bool {
    Path::new("/mnt/us").exists()
}

/// The books the home shows: library/ on the device (by hand or sent over Wi-Fi), the
/// repo's dev-books/ on a desktop.
pub fn library_dir() -> PathBuf {
    if on_device() {
        PathBuf::from("/mnt/us/library")
    } else {
        first_existing(&["dev-books"])
    }
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
        let state_dir = state_dir();
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

    /// The database in the state dir, `None` (logged) when it can't be opened.
    pub fn store(&self) -> Option<Rc<zarathustra_store::Store>> {
        open_store(&self.state_dir)
    }

    /// Books moved to another folder (documents/ → library/) keep their place: a saved
    /// position that belongs to none of `books` goes to the book of the same file name that
    /// has none. Positions an older build kept in files are taken in first.
    // ponytail: a moved book whose position is still in an old build's file loses it (the
    // file name hashes the old path); fine once a device has run this build.
    pub fn adopt_orphan_positions(&self, store: &zarathustra_store::Store, books: &[&Path]) {
        // One look at the state dir: only books whose old file is there are taken in.
        let legacy: std::collections::HashSet<PathBuf> = std::fs::read_dir(&self.state_dir)
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "pos")).collect())
            .unwrap_or_default();
        for book in books.iter().filter(|b| legacy.contains(&legacy_position_file(&self.state_dir, b))) {
            reader::Position::load(store, &self.state_dir, book);
        }
        let Ok(saved) = store.positions() else { return };
        let key = |p: &Path| p.to_string_lossy().into_owned();
        let owned: std::collections::HashSet<String> = books.iter().map(|b| key(b)).collect();
        let name = |p: &str| Path::new(p).file_name().map(|n| n.to_os_string());
        let orphans: Vec<&String> = saved.keys().filter(|p| !owned.contains(*p)).collect();
        for book in books.iter().filter(|b| !saved.contains_key(&key(b))) {
            let mine: Vec<&&String> =
                orphans.iter().filter(|o| name(o) == book.file_name().map(|n| n.to_os_string())).collect();
            if let [old] = mine.as_slice() {
                if let Err(e) = store.move_position(old, &key(book)) {
                    eprintln!("positions: {e}");
                }
            }
        }
    }
}

/// Where an older build kept a book's reading position: keyed by the full path, so books
/// with the same file name in different folders didn't share it.
pub(crate) fn legacy_position_file(state_dir: &Path, book: &Path) -> PathBuf {
    let stem = book.file_stem().map_or_else(|| "book".into(), |s| s.to_string_lossy().into_owned());
    // FNV-1a: stable across Rust versions, unlike the std hasher.
    let hash = book
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    state_dir.join(format!("{stem}-{hash:016x}.pos"))
}

/// Where positions, preferences and the database live (created if missing).
pub fn state_dir() -> PathBuf {
    let dir = if on_device() {
        PathBuf::from("/mnt/us/zarathustra/state")
    } else {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/zarathustra-state")).to_path_buf()
    };
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Preference group `name` (settings, typography…) from the database. A `<name>.txt` left by
/// an older build is imported once, then removed. Without a database (unwritable state dir)
/// the text file is the fallback. `None` when never saved.
pub fn load_pref(state_dir: &Path, name: &str) -> Option<String> {
    let file = state_dir.join(format!("{name}.txt"));
    let Some(store) = open_store(state_dir) else { return std::fs::read_to_string(file).ok() };
    match store.pref(name) {
        Ok(Some(value)) => return Some(value),
        Ok(None) => {}
        Err(e) => {
            eprintln!("prefs {name}: {e}");
            return None;
        }
    }
    let text = std::fs::read_to_string(&file).ok()?;
    match store.set_pref(name, &text) {
        Ok(()) => drop(std::fs::remove_file(&file)),
        Err(e) => eprintln!("prefs {name}: {e}"),
    }
    Some(text)
}

/// Saves preference group `name`: in the database, or its text file without one.
pub fn save_pref(state_dir: &Path, name: &str, value: &str) {
    let saved = match open_store(state_dir) {
        Some(store) => store.set_pref(name, value).map_err(|e| e.to_string()),
        None => write_atomic(&state_dir.join(format!("{name}.txt")), value).map_err(|e| e.to_string()),
    };
    if let Err(e) = saved {
        eprintln!("prefs {name}: {e}");
    }
}

/// The database file in `state_dir`.
pub(crate) fn db_path(state_dir: &Path) -> PathBuf {
    state_dir.join("zarathustra.db")
}

thread_local! {
    /// Open databases by state dir: opening one runs pragmas and the migration check, too
    /// slow on the device's flash to repeat for every preference read or page turn.
    static STORES: RefCell<HashMap<PathBuf, Rc<zarathustra_store::Store>>> = RefCell::new(HashMap::new());
}

/// The database in `state_dir`, opened once per thread; `None` (logged, retried next time)
/// when it can't be opened.
pub(crate) fn open_store(state_dir: &Path) -> Option<Rc<zarathustra_store::Store>> {
    let path = db_path(state_dir);
    if let Some(store) = STORES.with_borrow(|s| s.get(&path).cloned()) {
        return Some(store);
    }
    let store = Rc::new(zarathustra_store::Store::open(&path).map_err(|e| eprintln!("database: {e}")).ok()?);
    STORES.with_borrow_mut(|s| s.insert(path, store.clone()));
    Some(store)
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
    /// Typing: redraw the screen into [`typing::Typing::canvas`] and hand it this.
    Typed(keyboard::Typed),
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
        let store = zarathustra_store::Store::open_in_memory().expect("db");
        let (moved, here) = (Path::new("/mnt/us/library/Libro.epub"), Path::new("/mnt/us/library/a/Libro.epub"));
        store.set_position("/mnt/us/documents/Libro.epub", "3 0 0.5000 0", 1).expect("set");
        // The other Libro.epub is still in the library, its position in an older build's file:
        // taken in, and not up for grabs.
        std::fs::write(legacy_position_file(&dir, here), "1 0 0.1000 0\n").expect("write");
        ctx.adopt_orphan_positions(&store, &[moved, here]);
        let saved = store.positions().expect("positions");
        assert_eq!(saved.get(moved.to_str().expect("utf-8")).map(|v| v.0.as_str()), Some("3 0 0.5000 0"));
        assert_eq!(saved.get(here.to_str().expect("utf-8")).map(|v| v.0.as_str()), Some("1 0 0.1000 0\n"));
        assert!(!legacy_position_file(&dir, here).exists(), "the old file is gone");
        assert_eq!(saved.len(), 2);
        ctx.adopt_orphan_positions(&store, &[moved, here, Path::new("/mnt/us/library/b/Libro.epub")]);
        assert_eq!(store.positions().expect("positions").len(), 2, "owned ones stay put");
        std::fs::remove_dir_all(dir).ok();
    }
}
