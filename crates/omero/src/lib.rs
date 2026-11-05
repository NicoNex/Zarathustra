//! The Omero shell: home, reader and the state they share.

pub mod home;
pub mod menu;
pub mod quick;
pub mod reader;
pub mod settings;
pub mod sleep;
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
        Ctx { fonts, ts, raster: Rasterizer::new(1.3), state_dir }
    }

    /// The reading font (also used for book titles).
    pub fn serif(&self, bold: bool) -> Option<&Face> {
        self.fonts.pick(&["serif".into()], if bold { 700 } else { 400 }, false)
    }

    /// The UI font.
    pub fn sans(&self, bold: bool) -> Option<&Face> {
        self.fonts.pick(&["sans-serif".into()], if bold { 700 } else { 400 }, false)
    }

    /// Where a book's reading position is saved.
    pub fn state_file(&self, book: &Path) -> PathBuf {
        let stem = book.file_stem().map_or_else(|| "book".into(), |s| s.to_string_lossy().into_owned());
        self.state_dir.join(format!("{stem}.pos"))
    }
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
    Open(PathBuf),
    Home,
    Quit,
}
