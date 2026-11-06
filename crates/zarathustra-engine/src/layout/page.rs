//! Pagination: blocks → lines and images → pages, with widow and orphan control, headings
//! kept with what follows, forced breaks and blocks that must not split.

use super::flow::{Block, ImageRef};
use super::paragraph::{LayoutParams, Line, Placed, Run, Typesetter};
use crate::fonts::FontLibrary;
use crate::style::cascade::ComputedStyle;
use crate::style::css::TextAlign;

/// Fewest lines of a paragraph left at a page bottom (orphans) or carried to the next top (widows).
const MIN_LINES: usize = 2;
/// Share of the page height an image may fill, so a picture never quite touches the footer.
const IMAGE_MAX_HEIGHT: f32 = 0.96;
/// Never enlarge an image more than this over its pixel size (ornaments stay small).
const IMAGE_MAX_UPSCALE: f32 = 2.0;

/// A line on a page; `glyphs[].run` indexes the block's spans.
#[derive(Debug, Clone, PartialEq)]
pub struct PageLine {
    pub block: u32,
    /// Left edge and baseline, relative to the text area.
    pub x: f32,
    pub baseline: f32,
    pub glyphs: Vec<Placed>,
}

/// An image on a page; `block` indexes the chapter's blocks, whose `image` holds the source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageImage {
    pub block: u32,
    /// Top-left corner and size, relative to the text area.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub lines: Vec<PageLine>,
    pub images: Vec<PageImage>,
}

impl Page {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.images.is_empty()
    }

    /// The first block shown on the page (its reading position).
    pub fn first_block(&self) -> Option<u32> {
        let line = self.lines.first().map(|l| l.block);
        let image = self.images.first().map(|i| i.block);
        line.into_iter().chain(image).min()
    }
}

/// An image's laid-out box: left edge within the text area (centred in its column) and size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageBox {
    pub x: f32,
    pub width: f32,
    pub height: f32,
}

/// What pagination needs to know about a block besides its lines.
#[derive(Debug, Clone, Copy, Default)]
pub struct Flow {
    pub space_before: f32,
    pub x: f32,
    pub break_before: bool,
    pub keep_with_next: bool,
    pub avoid_break_inside: bool,
    /// An image block: placed whole instead of lines.
    pub image: Option<ImageBox>,
}

impl From<&Block> for Flow {
    fn from(b: &Block) -> Self {
        Flow {
            space_before: b.space_before,
            x: b.x,
            break_before: b.break_before,
            keep_with_next: b.keep_with_next,
            avoid_break_inside: b.avoid_break_inside,
            image: None,
        }
    }
}

/// Display size of `img` in a column `column` wide on a page `height` tall: CSS size when
/// given, else twice the pixel size; at most 2× the pixels, within the column and the page,
/// aspect ratio kept. `None` when the image's size is unknown (undecodable format).
pub fn image_size(img: &ImageRef, column: f32, height: f32) -> Option<(f32, f32)> {
    let (iw, ih) = img.intrinsic?;
    let (iw, ih) = (iw as f32, ih as f32);
    let aspect = ih / iw;
    let w = match (img.width, img.height) {
        (Some(w), _) => w,
        (None, Some(h)) => h / aspect,
        // A CSS px is ~3 device pixels at 300 dpi: pixel-sized pictures would be stamps.
        (None, None) => iw * IMAGE_MAX_UPSCALE,
    };
    let w = w.min(iw * IMAGE_MAX_UPSCALE).min(column).min(height * IMAGE_MAX_HEIGHT / aspect);
    (w >= 1.0 && w * aspect >= 1.0).then_some((w, w * aspect))
}

/// Typesets and paginates a chapter's blocks into pages `height` pixels tall.
pub fn layout(
    blocks: &[Block],
    styles: &[Option<ComputedStyle>],
    fonts: &FontLibrary,
    ts: &mut Typesetter,
    height: f32,
) -> Vec<Page> {
    let laid = blocks.iter().map(|b| {
        if let Some(img) = &b.image {
            let image = image_size(img, b.width, height).map(|(width, h)| ImageBox {
                x: b.x + (b.width - width) / 2.0,
                width,
                height: h,
            });
            return (Flow { image, ..Flow::from(b) }, Vec::new());
        }
        let style = |n: crate::dom::NodeId| styles.get(n.index()).and_then(Option::as_ref);
        let runs: Option<Vec<Run>> = (0..b.spans.len())
            .map(|i| {
                let s = style(b.spans[i].node)?;
                let face = fonts.pick(&s.font_family, s.font_weight, s.italic)?;
                Some(Run { text: b.span_text(i), style: s, face, lang: b.lang.as_deref() })
            })
            .collect();
        let lines = match (runs, style(b.node)) {
            (Some(runs), Some(s)) => {
                // Headings are never stretched to the measure, whatever the justify preference.
                let align = match s.text_align {
                    TextAlign::Justify if b.keep_with_next => TextAlign::Start,
                    a => a,
                };
                ts.layout(&runs, &LayoutParams { measure: b.width, indent: s.text_indent, align })
            }
            _ => Vec::new(),
        };
        (Flow::from(b), lines)
    });
    paginate(laid, height)
}

/// Stacks blocks' lines and images into pages.
pub fn paginate(blocks: impl IntoIterator<Item = (Flow, Vec<Line>)>, height: f32) -> Vec<Page> {
    let blocks: Vec<(Flow, Vec<Line>)> = blocks.into_iter().collect();
    let mut pages = Vec::new();
    let mut page = Page::default();
    let mut y = 0.0f32;
    let mut carried_space = 0.0f32;
    let mut carried_break = false;

    for (bi, (flow, lines)) in blocks.iter().enumerate() {
        if lines.is_empty() && flow.image.is_none() {
            carried_space = carried_space.max(flow.space_before);
            carried_break |= flow.break_before;
            continue;
        }
        let space = std::mem::take(&mut carried_space).max(flow.space_before);
        let forced = std::mem::take(&mut carried_break) || flow.break_before;
        let new_page = |page: &mut Page, pages: &mut Vec<Page>, y: &mut f32| {
            if !page.is_empty() {
                pages.push(std::mem::take(page));
            }
            *y = 0.0;
        };
        if forced {
            new_page(&mut page, &mut pages, &mut y);
        }
        if !page.is_empty() {
            y += space;
        }
        if let Some(img) = flow.image {
            // Images never split: on to the next page, or shrunk to fit a fresh one.
            if y + img.height > height + 0.01 && !page.is_empty() {
                new_page(&mut page, &mut pages, &mut y);
            }
            let scale = (height / img.height).min(1.0);
            let (w, h) = (img.width * scale, img.height * scale);
            page.images.push(PageImage { block: bi as u32, x: img.x + (img.width - w) / 2.0, y, width: w, height: h });
            y += h;
            continue;
        }
        // A heading moves on when it and the start of what follows don't fit together.
        if flow.keep_with_next && !page.is_empty() {
            let own: f32 = lines.iter().map(|l| l.height).sum();
            let next = blocks[bi + 1..].iter().find(|(f, l)| !l.is_empty() || f.image.is_some());
            let next_start = next.map_or(0.0, |(f, l)| {
                let start = match f.image {
                    Some(img) => img.height,
                    None => l.iter().take(MIN_LINES).map(|l| l.height).sum::<f32>(),
                };
                f.space_before + start
            });
            if y + own + next_start > height {
                new_page(&mut page, &mut pages, &mut y);
            }
        }

        let n = lines.len();
        let mut i = 0;
        while i < n {
            let mut fit = 0;
            let mut h = y;
            while i + fit < n && h + lines[i + fit].height <= height + 0.01 {
                h += lines[i + fit].height;
                fit += 1;
            }
            let mut take = fit;
            if i + fit < n {
                // Splitting: no orphans at the bottom, no widows at the top of the next page.
                let rest = n - i - take;
                if rest < MIN_LINES {
                    take = take.saturating_sub(MIN_LINES - rest);
                }
                if i == 0 && (take < MIN_LINES || flow.avoid_break_inside) {
                    take = 0;
                }
                if take == 0 && page.is_empty() {
                    take = fit.max(1); // nothing fits better on a fresh page: make progress
                }
            }
            for line in &lines[i..i + take] {
                page.lines.push(PageLine {
                    block: bi as u32,
                    x: flow.x,
                    baseline: y + line.baseline,
                    glyphs: line.glyphs.clone(),
                });
                y += line.height;
            }
            i += take;
            if i < n {
                new_page(&mut page, &mut pages, &mut y);
            }
        }
    }
    if !page.is_empty() {
        pages.push(page);
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize) -> Vec<Line> {
        vec![Line { glyphs: Vec::new(), baseline: 8.0, height: 10.0 }; n]
    }

    fn shape(pages: &[Page]) -> Vec<Vec<u32>> {
        pages.iter().map(|p| p.lines.iter().map(|l| l.block).collect()).collect()
    }

    #[test]
    fn fills_pages_and_controls_widows_and_orphans() {
        let f = Flow::default();
        // 10 lines per page. Block 0 takes 9; block 1 (5 lines) would leave 1 orphan: moves whole.
        let p = paginate([(f, lines(9)), (f, lines(5))], 100.0);
        assert_eq!(shape(&p), [vec![0; 9], vec![1; 5]]);

        // Block 1 (11 lines) after 3: 7 fit, 4 carried; fine.
        let p = paginate([(f, lines(3)), (f, lines(11))], 100.0);
        assert_eq!(p[0].lines.len(), 10);
        assert_eq!(p[1].lines.len(), 4);

        // Block 1 (8 lines) after 3: 7 fit, 1 widow → carry 2.
        let p = paginate([(f, lines(3)), (f, lines(8))], 100.0);
        assert_eq!((p[0].lines.len(), p[1].lines.len()), (9, 2));
    }

    #[test]
    fn headings_breaks_and_spacing() {
        let f = Flow::default();
        let heading = Flow { keep_with_next: true, ..f };
        // Heading at the page bottom with no room for two lines after it moves on.
        let p = paginate([(f, lines(8)), (heading, lines(1)), (f, lines(4))], 100.0);
        assert_eq!(shape(&p), [vec![0; 8], vec![1, 2, 2, 2, 2]]);

        let chapter = Flow { break_before: true, ..f };
        let p = paginate([(f, lines(2)), (chapter, lines(2))], 100.0);
        assert_eq!(p.len(), 2);

        // Space before is dropped at a page top and kept elsewhere.
        let spaced = Flow { space_before: 15.0, ..f };
        let p = paginate([(spaced, lines(1)), (spaced, lines(1))], 100.0);
        assert_eq!(p[0].lines[0].baseline, 8.0);
        assert_eq!(p[0].lines[1].baseline, 10.0 + 15.0 + 8.0);

        // A block taller than a page still makes progress.
        let p = paginate([(f, lines(25))], 100.0);
        assert_eq!(p.iter().map(|p| p.lines.len()).collect::<Vec<_>>(), [10, 10, 5]);
    }

    #[test]
    fn images_never_split_and_shrink_on_a_fresh_page() {
        let f = Flow::default();
        let img = |height| Flow { image: Some(ImageBox { x: 10.0, width: 40.0, height }), ..f };
        // 30 px image after 8 lines (80 px): doesn't fit, moves on; text continues below it.
        let p = paginate([(f, lines(8)), (img(30.0), Vec::new()), (f, lines(3))], 100.0);
        assert_eq!(shape(&p), [vec![0; 8], vec![2; 3]]);
        assert_eq!(p[1].images, [PageImage { block: 1, x: 10.0, y: 0.0, width: 40.0, height: 30.0 }]);
        assert_eq!(p[1].lines[0].baseline, 30.0 + 8.0);
        assert_eq!(p[1].first_block(), Some(1));
        // Fits after 5 lines: same page.
        let p = paginate([(f, lines(5)), (img(30.0), Vec::new())], 100.0);
        assert_eq!((p.len(), p[0].images[0].y), (1, 50.0));
        // Taller than a page on an empty page: scaled down, centred, not dropped.
        let p = paginate([(img(200.0), Vec::new()), (f, lines(1))], 100.0);
        assert_eq!(p[0].images, [PageImage { block: 0, x: 20.0, y: 0.0, width: 20.0, height: 100.0 }]);
        assert_eq!(p.len(), 2);
        // A heading followed by an image that won't fit beside it moves on with it.
        let heading = Flow { keep_with_next: true, ..f };
        let p = paginate([(f, lines(5)), (heading, lines(1)), (img(45.0), Vec::new())], 100.0);
        assert_eq!(shape(&p), [vec![0; 5], vec![1]]);
        assert_eq!(p[1].images.len(), 1);
    }

    #[test]
    fn image_sizing() {
        let img = |intrinsic, width, height| ImageRef { src: String::new(), width, height, intrinsic };
        // Small ornament: at most 2x.
        assert_eq!(image_size(&img(Some((50, 20)), None, None), 1000.0, 1000.0), Some((100.0, 40.0)));
        // Wide photo: column width.
        assert_eq!(image_size(&img(Some((2000, 1000)), None, None), 1000.0, 1000.0), Some((1000.0, 500.0)));
        // Tall cover: page height (minus a little).
        let (w, h) = image_size(&img(Some((600, 1200)), None, None), 1000.0, 1000.0).expect("size");
        assert!((h - 960.0).abs() < 0.01 && (w - 480.0).abs() < 0.01, "{w}x{h}");
        // CSS width (already resolved to pixels), still capped at 2x the pixels.
        assert_eq!(image_size(&img(Some((400, 200)), Some(500.0), None), 1000.0, 1000.0), Some((500.0, 250.0)));
        assert_eq!(image_size(&img(Some((100, 50)), Some(900.0), None), 1000.0, 1000.0), Some((200.0, 100.0)));
        assert_eq!(image_size(&img(Some((400, 200)), None, Some(100.0)), 1000.0, 1000.0), Some((200.0, 100.0)));
        assert_eq!(image_size(&img(None, Some(500.0), None), 1000.0, 1000.0), None);
    }
}
