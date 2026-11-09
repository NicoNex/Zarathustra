//! The definition card: a long press on a word opens it over the page, as on the Kindle.
//!
//! The dictionaries' articles (HTML) are laid out by the book engine in the reading font and
//! paginated to the card: tap or swipe inside to turn its pages, tap outside to close it.
//! Its footer offers to highlight the word, so the card replaces the highlight popup for a
//! single word.
// ponytail: no Wikipedia. Plan: an HTTPS GET of
// `https://{lang}.wikipedia.org/api/rest_v1/page/summary/{title}` (JSON, `extract` field) on
// a worker thread, shown as one more article with a "loading" line until it lands. Measured
// cost for armv7 musl: ureq 3 + rustls + ring + webpki-roots = +1.3 MB stripped (LTO), 34
// crates; plus Wi-Fi bring-up, which the Amazon framework owns while it runs. Worth it once
// the shell manages Wi-Fi itself (store sync needs the same client).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::dict::{to_xhtml, Article, Definition, Dictionaries};
use zarathustra_engine::dom::Dom;
use zarathustra_engine::font::draw_glyph;
use zarathustra_engine::layout::flow::{self, Block};
use zarathustra_engine::layout::page::{self, Page};
use zarathustra_engine::style::cascade::{compute, Preferences, Stylesheets};
use zarathustra_engine::style::css::TextAlign;

use crate::i18n::t;
use crate::ui::{button, rounded_outline, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{screen_h, screen_w, Ctx};

/// Distance from the screen edges.
const MARGIN: i32 = 28;
/// Room left between the word and the card.
const GAP: i32 = 36;
const PAD: i32 = 44;
const FOOTER_H: i32 = 104;
const RADIUS: i32 = 28;
const BORDER: i32 = 3;
/// The card covers at most this share of the screen: the page stays in view.
const MAX_SHARE: f32 = 0.5;
/// Smallest card, so "not found" still reads as a card.
const MIN_TEXT_H: i32 = 140;
const BUTTON_W: i32 = 280;
const BUTTON_H: i32 = 72;
/// Article text relative to the reading size: a reference, not the book.
const TEXT_SCALE: f32 = 0.86;
const SWIPE: i32 = 100;

const CSS: &str = r#"
body { margin: 0 }
p { margin: .25em 0; text-indent: 0 }
h6.dict { font-size: .72em; font-weight: normal; margin: 0 0 .2em 0; page-break-before: always }
h1.hw { font-size: 1.45em; margin: .1em 0 .3em 0; text-align: left }
h1, h2, h3, h4, h5, h6 { font-size: 1em; margin: .5em 0 .15em 0 }
ul, ol { margin: .15em 0; padding-left: 1.3em }
li { margin: .1em 0 }
section, div { margin: 0 }
"#;

static DICTS: OnceLock<Dictionaries> = OnceLock::new();

/// The dictionaries on the device (the platform's dictionaries folder, `dev-dicts/` on a desktop),
/// opened on first use; their index tables are then built (or read back from the state
/// dir) on a background thread, so the first lookup does not pay for it.
pub fn dictionaries(state_dir: &Path) -> &'static Dictionaries {
    let mut opened = false;
    let dicts = DICTS.get_or_init(|| {
        opened = true;
        let dirs = vec![zarathustra_device::platform().dictionaries.clone()];
        let cache: PathBuf = state_dir.join("dict-cache");
        let d = Dictionaries::open(&dirs, Some(&cache));
        eprintln!("dictionaries: {:?}", d.names().collect::<Vec<_>>());
        d
    });
    // Spawned once the cell is set: from inside the initializer, the thread could look
    // before it is and skip the warm-up.
    if opened {
        std::thread::spawn(|| dicts.warm());
    }
    dicts
}

/// What a tap on the open card did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tap {
    /// Turned a page: refresh the card.
    Page(Rect),
    /// The Highlight button: save the word, close the card.
    Highlight,
    /// Outside the card: close it.
    Close,
    /// On the card, nothing to do.
    None,
}

pub struct Card {
    pub rect: Rect,
    /// The laid-out text; pages index into it (kept for tests to read the pages back).
    #[cfg_attr(not(test), allow(dead_code))]
    blocks: Vec<Block>,
    /// Per block: a dictionary's name (drawn soft).
    labels: Vec<bool>,
    pages: Vec<Page>,
    page: usize,
}

impl Card {
    /// The card for `word` (on screen at `word_rect`, in a book in `lang`), laid out with the
    /// reader's typography `prefs`. Placed at the screen edge away from the word.
    pub fn open(
        dicts: &Dictionaries,
        word: &str,
        lang: Option<&str>,
        word_rect: Rect,
        prefs: &Preferences,
        ctx: &mut Ctx,
    ) -> Card {
        let t0 = std::time::Instant::now();
        let mut defs = dicts.define(word, lang);
        let looked_up = t0.elapsed();
        if defs.is_empty() {
            let missing =
                t().no_dictionaries.replace("{dir}", &zarathustra_device::platform().dictionaries.to_string_lossy());
            let note = if dicts.is_empty() { missing.as_str() } else { t().no_definition };
            let article = Article { headword: word.into(), body: note.into(), html: false };
            defs.push(Definition { dict: String::new(), article });
        }
        // The side with more room: below the word when it sits in the upper half.
        let below = word_rect.y + word_rect.h / 2 < screen_h() / 2;
        let room = if below { screen_h() - MARGIN - word_rect.bottom() - GAP } else { word_rect.y - MARGIN - GAP };
        let max_h = room.min((screen_h() as f32 * MAX_SHARE) as i32).max(MIN_TEXT_H + 2 * PAD + FOOTER_H);
        let width = screen_w() - 2 * MARGIN;
        let text_w = (width - 2 * PAD) as f32;
        let text_h = (max_h - PAD - FOOTER_H) as f32;

        let prefs = Preferences {
            font_size: prefs.font_size * TEXT_SCALE,
            font_family: prefs.font_family.clone(),
            line_height: Some(1.3),
            align: Some(TextAlign::Start),
            hyphenate: true,
            page_width: text_w,
        };
        let (blocks, labels, pages) = layout(&defs, lang, &prefs, text_h, ctx);
        let used = match pages.as_slice() {
            [one] => (bottom(one) as i32).max(MIN_TEXT_H),
            _ => text_h as i32,
        };
        let h = PAD + used + FOOTER_H;
        let y = if below { screen_h() - MARGIN - h } else { MARGIN };
        eprintln!("define {word:?}: {} articles, lookup {looked_up:?}, card {:?}", defs.len(), t0.elapsed());
        Card { rect: Rect::new(MARGIN, y, width, h), blocks, labels, pages, page: 0 }
    }

    /// What the card covers: paper from it to its screen edge (hiding the footer or header
    /// there), with a margin so the outline reads against the text right behind it.
    pub fn area(&self) -> Rect {
        if self.rect.y > MARGIN {
            Rect::new(0, self.rect.y - 8, screen_w(), screen_h() - self.rect.y + 8)
        } else {
            Rect::new(0, 0, screen_w(), self.rect.bottom() + 8)
        }
    }

    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    fn footer(&self) -> Rect {
        Rect::new(self.rect.x, self.rect.bottom() - FOOTER_H, self.rect.w, FOOTER_H)
    }

    fn highlight_button(&self) -> Rect {
        let f = self.footer();
        Rect::new(f.x + PAD - 8, f.y + (f.h - BUTTON_H) / 2 - 6, BUTTON_W, BUTTON_H)
    }

    /// A touch from `(x0, y0)` lifted at `x`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), x: i32) -> Tap {
        if !self.rect.contains(x0, y0) {
            return Tap::Close;
        }
        if self.highlight_button().contains(x0, y0) {
            return Tap::Highlight;
        }
        let forward = match x - x0 {
            dx if dx < -SWIPE => true,
            dx if dx > SWIPE => false,
            _ => x0 >= self.rect.x + self.rect.w / 2,
        };
        let next = if forward { self.page + 1 } else { self.page.wrapping_sub(1) };
        if next >= self.pages.len() {
            return Tap::None;
        }
        self.page = next;
        Tap::Page(self.rect)
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(self.area().intersect(&c.bounds()), PAPER);
        rounded_outline(c, self.rect, RADIUS, BORDER, INK, PAPER);
        let (ox, oy) = (self.rect.x + PAD, self.rect.y + PAD);
        if let Some(page) = self.pages.get(self.page) {
            for line in &page.lines {
                let ink = if self.labels.get(line.block as usize) == Some(&true) { INK_SOFT } else { INK };
                for g in &line.glyphs {
                    let Some(face) = ctx.fonts.face(g.face) else { continue };
                    let (gx, glyph) = ctx.raster.glyph(face, g.gid, g.size, ox as f32 + line.x + g.x);
                    draw_glyph(c, glyph, gx, (oy as f32 + line.baseline + g.y).round() as i32, ink);
                }
            }
        }
        let f = self.footer();
        c.fill(Rect::new(f.x + PAD, f.y - 4, f.w - 2 * PAD, 2), LINE);
        let b = self.highlight_button();
        button(c, b, false);
        let Ctx { fonts, raster, .. } = ctx;
        if let Some(face) = crate::sans(fonts, true) {
            Label { face, size: 30.0, ink: INK }.centered(c, raster, t().highlight, b);
        }
        if self.pages.len() > 1 {
            if let Some(face) = crate::sans(fonts, false) {
                let label = Label { face, size: 28.0, ink: INK_SOFT };
                let text = format!("‹   {} / {}   ›", self.page + 1, self.pages.len());
                let x = (f.right() - PAD) as f32 - label.width(&text);
                label.draw(c, raster, &text, x, b.y + b.h / 2 + 10);
            }
        }
    }

    /// Text of the page shown, for tests.
    #[cfg(test)]
    fn page_text(&self) -> String {
        let page = &self.pages[self.page];
        let mut blocks: Vec<u32> = page.lines.iter().map(|l| l.block).collect();
        blocks.dedup();
        blocks.iter().map(|&b| self.blocks[b as usize].text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

/// Lowest ink on a page: the last line's baseline plus a descender.
fn bottom(p: &Page) -> f32 {
    let line = |l: &zarathustra_engine::layout::page::PageLine| {
        l.baseline + 0.3 * l.glyphs.iter().map(|g| g.size).fold(0.0, f32::max)
    };
    p.lines.iter().map(line).fold(0.0, f32::max)
}

/// Lays the definitions out into pages `height` tall: blocks, which blocks are dictionary
/// names, pages. Pictures are left out: the card has no room for them.
fn layout(
    defs: &[Definition],
    lang: Option<&str>,
    prefs: &Preferences,
    height: f32,
    ctx: &mut Ctx,
) -> (Vec<Block>, Vec<bool>, Vec<Page>) {
    let dom = match Dom::parse_xml(&to_xhtml(defs)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("definition: {e}");
            return (Vec::new(), Vec::new(), Vec::new());
        }
    };
    let mut sheets = Stylesheets::default();
    sheets.add_author(CSS);
    let styles = compute(&dom, &sheets, prefs);
    let mut blocks = flow::blocks(&dom, &styles, lang);
    blocks.retain(|b| b.image.is_none() && !b.text.trim().is_empty());
    let labels = blocks.iter().map(|b| dom.attr(b.node, "class") == Some("dict")).collect();
    let pages = page::layout(&blocks, &styles, &ctx.fonts, &mut ctx.ts, height);
    (blocks, labels, pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs() -> Preferences {
        Preferences {
            font_size: 40.0,
            font_family: None,
            line_height: None,
            align: None,
            hyphenate: true,
            page_width: 1000.0,
        }
    }

    fn card(defs: Vec<Definition>, word_y: i32, ctx: &mut Ctx) -> Card {
        let prefs = Preferences { font_size: prefs().font_size * TEXT_SCALE, ..prefs() };
        let max_h = (screen_h() as f32 * MAX_SHARE) as i32;
        let (blocks, labels, pages) = layout(&defs, Some("it"), &prefs, (max_h - PAD - FOOTER_H) as f32, ctx);
        let y = if word_y < screen_h() / 2 { screen_h() - MARGIN - max_h } else { MARGIN };
        Card { rect: Rect::new(MARGIN, y, screen_w() - 2 * MARGIN, max_h), blocks, labels, pages, page: 0 }
    }

    fn def(dict: &str, hw: &str, body: String) -> Definition {
        Definition { dict: dict.into(), article: Article { headword: hw.into(), body, html: true } }
    }

    #[test]
    fn long_definitions_paginate_and_taps_turn_them() {
        let mut ctx = Ctx::new();
        if ctx.fonts.pick(&["serif".into()], 400, false).is_none() {
            return; // no fonts on this machine
        }
        let senses: String =
            (1..=40).map(|i| format!("<li>senso numero {i} della parola, con una spiegazione</li>")).collect();
        let defs = vec![
            def("Wikizionario", "selva", format!("<ol>{senses}</ol>")),
            def("Altro", "selva", "<p>bosco</p>".into()),
        ];
        let mut c = card(defs, 300, &mut ctx);
        assert!(c.pages() > 1, "40 senses fill more than one card");
        assert!(c.labels[0] && !c.labels[1], "the dictionary name, then the headword");
        assert!(c.page_text().starts_with("Wikizionario\nselva"), "{}", c.page_text());
        let (mid_y, right, left) = (c.rect.y + 100, c.rect.right() - 50, c.rect.x + 50);
        // Back on the first page: nothing. Forward by tap, then by swipe.
        assert_eq!(c.tap((left, mid_y), left), Tap::None);
        assert_eq!(c.tap((right, mid_y), right), Tap::Page(c.rect));
        assert_eq!(c.page, 1);
        let last = c.pages() - 1;
        for _ in 0..last {
            c.tap((right, mid_y), right - 300);
        }
        assert_eq!(c.page, last);
        assert!(c.page_text().contains("bosco"), "the second dictionary ends the last page");
        assert_eq!(c.tap((right, mid_y), right), Tap::None);
        assert_eq!(c.tap((left, mid_y), left + 300), Tap::Page(c.rect));
        let b = c.highlight_button();
        assert_eq!(c.tap((b.x + 5, b.y + 5), b.x + 5), Tap::Highlight);
        assert_eq!(c.tap((10, if c.rect.y > 100 { 10 } else { screen_h() - 10 }), 10), Tap::Close);
    }

    #[test]
    fn short_definitions_get_a_short_card_away_from_the_word() {
        let mut ctx = Ctx::new();
        if ctx.fonts.pick(&["serif".into()], 400, false).is_none() {
            return;
        }
        let dicts = Dictionaries::default();
        // Word near the top: card at the bottom edge; no dictionaries: the hint shows.
        let c = Card::open(&dicts, "casa", Some("it"), Rect::new(100, 200, 120, 50), &prefs(), &mut ctx);
        assert_eq!(c.rect.bottom(), screen_h() - MARGIN);
        assert!(c.rect.h < screen_h() / 3, "{}", c.rect.h);
        assert_eq!(c.pages(), 1);
        assert!(c.page_text().contains("StarDict"));
        // Word near the bottom: card at the top.
        let c = Card::open(&dicts, "casa", Some("it"), Rect::new(100, 1400, 120, 50), &prefs(), &mut ctx);
        assert_eq!(c.rect.y, MARGIN);
    }
}
