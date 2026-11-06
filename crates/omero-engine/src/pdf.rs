//! PDF (and other fixed-layout documents MuPDF opens): metadata, outline and pages rendered
//! to 8-bit gray. Pages are pictures here, not reflowed text.

use std::path::Path;

use mupdf::{Colorspace, Device, DisplayList, Document, Matrix, MetadataName, Outline, Pixmap, Rect};

use crate::image::GrayImage;

pub use mupdf::Error as PdfError;

/// Preview size the white margins are found in: coarse but cheap (a few ms per page).
const CROP_PREVIEW: f32 = 400.0;
/// Samples darker than this are content when looking for the margins.
const CROP_INK: u8 = 240;
/// White kept around the content, as a share of the page side.
const CROP_PAD: f32 = 0.02;
/// A cropped page keeps at least this share of the page on each axis, so a page with one
/// short line (a part title) is not blown up to fill the screen.
const CROP_MIN: f32 = 0.6;

/// An outline (bookmarks) entry, flattened depth first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    pub title: String,
    pub depth: usize,
    /// Page index (0-based) the entry points at.
    pub page: usize,
}

// ponytail: MuPDF's resource store (decoded images, fonts) is capped at its 256 MB default,
// which the `mupdf` crate does not expose. Fine for text PDFs; if scanned books push the
// PW5's 512 MB, shrink it through `mupdf-sys` (`fz_shrink_store`) after each page.
pub struct Pdf {
    doc: Document,
    pages: usize,
}

impl Pdf {
    pub fn open(path: &Path) -> Result<Pdf, PdfError> {
        Pdf::from_doc(Document::open(path)?)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Pdf, PdfError> {
        Pdf::from_doc(Document::from_bytes(bytes, "application/pdf")?)
    }

    fn from_doc(doc: Document) -> Result<Pdf, PdfError> {
        let pages = usize::try_from(doc.page_count()?).unwrap_or(0);
        Ok(Pdf { doc, pages })
    }

    pub fn page_count(&self) -> usize {
        self.pages
    }

    /// Title from the document info; `None` when missing or blank.
    pub fn title(&self) -> Option<String> {
        self.info(MetadataName::Title)
    }

    pub fn author(&self) -> Option<String> {
        self.info(MetadataName::Author)
    }

    fn info(&self, name: MetadataName) -> Option<String> {
        let v = self.doc.metadata(name).ok()?;
        let v = v.trim();
        (!v.is_empty()).then(|| v.to_string())
    }

    /// The bookmarks, flattened depth first; entries without a page in this file (external
    /// links) are dropped, their children kept.
    pub fn outline(&self) -> Vec<OutlineEntry> {
        fn walk(items: &[Outline], depth: usize, out: &mut Vec<OutlineEntry>) {
            for o in items {
                let title = o.title.split_whitespace().collect::<Vec<_>>().join(" ");
                if let Some(d) = o.dest.as_ref().filter(|_| !title.is_empty()) {
                    out.push(OutlineEntry { title, depth, page: d.loc.page_number as usize });
                }
                walk(&o.down, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.doc.outlines().unwrap_or_default(), 0, &mut out);
        out
    }

    /// Page `n` fitted into `max_w`×`max_h`, aspect kept (the image is at most that big, one
    /// side exactly). With `crop`, the white margins around the content are cut first.
    pub fn render(&self, n: usize, max_w: u32, max_h: u32, crop: bool) -> Result<GrayImage, PdfError> {
        let page = self.doc.load_page(i32::try_from(n).unwrap_or(i32::MAX))?;
        let bounds = page.bounds()?;
        // Interpreted once: cropping rasterizes the page twice (preview, then the page).
        let list = page.to_display_list(false)?;
        let area = if crop { content_box(&list, bounds)? } else { bounds };
        let (aw, ah) = (area.x1 - area.x0, area.y1 - area.y0);
        if aw <= 0.0 || ah <= 0.0 || max_w == 0 || max_h == 0 {
            return Err(PdfError::InvalidArgument(format!("page {n} has no area")));
        }
        let scale = (max_w as f32 / aw).min(max_h as f32 / ah);
        let w = ((aw * scale).round() as u32).clamp(1, max_w);
        let h = ((ah * scale).round() as u32).clamp(1, max_h);
        draw(&list, &area, scale, w, h)
    }
}

/// Renders the `area` of a page's `list` (page space) at `scale` into a white `w`×`h` gray image.
fn draw(list: &DisplayList, area: &Rect, scale: f32, w: u32, h: u32) -> Result<GrayImage, PdfError> {
    let (wi, hi) = (i32::try_from(w).unwrap_or(i32::MAX), i32::try_from(h).unwrap_or(i32::MAX));
    let mut pix = Pixmap::new_with_w_h(&Colorspace::device_gray(), wi, hi, false)?;
    pix.clear_with(255)?;
    {
        let dev = Device::from_pixmap(&pix)?;
        let ctm = Matrix::new(scale, 0.0, 0.0, scale, -area.x0 * scale, -area.y0 * scale);
        list.run(&dev, &ctm, Rect::INF)?;
        // The device closes on drop, before the samples are read.
    }
    let stride = usize::try_from(pix.stride()).unwrap_or(w as usize);
    let samples = pix.samples();
    let pixels = if stride == w as usize {
        samples.get(..w as usize * h as usize).unwrap_or(samples).to_vec()
    } else {
        let mut pixels = Vec::with_capacity(w as usize * h as usize);
        for row in samples.chunks(stride).take(h as usize) {
            pixels.extend_from_slice(&row[..(w as usize).min(row.len())]);
        }
        pixels
    };
    Ok(GrayImage { width: w, height: h, pixels })
}

/// The part of `bounds` with ink on it, found on a small preview, grown to [`CROP_MIN`] of the
/// page per axis. The whole page when it is blank.
// ponytail: per page, so the zoom may change between pages of uneven fill; a whole-book (or
// odd/even) box from a few sampled pages would steady it.
fn content_box(list: &DisplayList, bounds: Rect) -> Result<Rect, PdfError> {
    let (pw, ph) = (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0);
    let scale = CROP_PREVIEW / pw.max(ph).max(1.0);
    let (w, h) = (((pw * scale).ceil() as u32).max(1), ((ph * scale).ceil() as u32).max(1));
    let img = draw(list, &bounds, scale, w, h)?;
    let Some((x0, y0, x1, y1)) = ink_box(&img) else { return Ok(bounds) };
    // Slack each side: the preview's antialiasing rounds off edges, and ink flush with the
    // screen edge looks clipped.
    let to_page = |v: u32, origin: f32| origin + v as f32 / scale;
    let span = |a: f32, b: f32, lo: f32, hi: f32| {
        let pad = 1.0 / scale + (hi - lo) * CROP_PAD;
        let (a, b) = ((a - pad).max(lo), (b + pad).min(hi));
        let min = (hi - lo) * CROP_MIN;
        if b - a >= min {
            return (a, b);
        }
        let start = ((a + b - min) / 2.0).clamp(lo, hi - min);
        (start, start + min)
    };
    let (x0, x1) = span(to_page(x0, bounds.x0), to_page(x1 + 1, bounds.x0), bounds.x0, bounds.x1);
    let (y0, y1) = span(to_page(y0, bounds.y0), to_page(y1 + 1, bounds.y0), bounds.y0, bounds.y1);
    Ok(Rect { x0, y0, x1, y1 })
}

/// Bounding box (inclusive pixel coordinates) of the samples darker than [`CROP_INK`].
fn ink_box(img: &GrayImage) -> Option<(u32, u32, u32, u32)> {
    let w = img.width as usize;
    let mut found: Option<(u32, u32, u32, u32)> = None;
    for (y, row) in img.pixels.chunks_exact(w.max(1)).enumerate() {
        let (Some(l), Some(r)) = (row.iter().position(|&p| p < CROP_INK), row.iter().rposition(|&p| p < CROP_INK))
        else {
            continue;
        };
        let (l, r, y) = (l as u32, r as u32, y as u32);
        found = Some(match found {
            None => (l, y, r, y),
            Some((x0, y0, x1, _)) => (x0.min(l), y0, x1.max(r), y),
        });
    }
    found
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A two-page US Letter PDF with an outline: page 1 has a black square in the middle
    /// (wide white margins), page 2 is blank. Offsets in the xref table are computed.
    pub(crate) fn sample_pdf() -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>",
            "<< /Length 36 >>\nstream\n0 g 206 296 200 200 re f\nendstream",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
            "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >>",
            "<< /Title (One) /Parent 6 0 R /Next 8 0 R /Dest [3 0 R /Fit] >>",
            "<< /Title (Two) /Parent 6 0 R /Prev 7 0 R /Dest [5 0 R /Fit] >>",
            "<< /Title (Sample) /Author (Ann Author) >>",
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        let trailer = format!("trailer\n<< /Size {} /Root 1 0 R /Info 9 0 R >>\n", objects.len() + 1);
        out.extend_from_slice(format!("{trailer}startxref\n{xref}\n%%EOF\n").as_bytes());
        out
    }

    #[test]
    fn reads_metadata_and_outline() {
        let pdf = Pdf::from_bytes(&sample_pdf()).expect("open");
        assert_eq!(pdf.page_count(), 2);
        assert_eq!(pdf.title().as_deref(), Some("Sample"));
        assert_eq!(pdf.author().as_deref(), Some("Ann Author"));
        let titles: Vec<_> = pdf.outline().into_iter().map(|e| (e.title, e.depth, e.page)).collect();
        assert_eq!(titles, [("One".to_string(), 0, 0), ("Two".to_string(), 0, 1)]);
    }

    #[test]
    fn renders_fitted_and_cropped() {
        let pdf = Pdf::from_bytes(&sample_pdf()).expect("open");
        let full = pdf.render(0, 612, 1000, false).expect("render");
        assert_eq!((full.width, full.height), (612, 792));
        // The square is 206..406 × 296..496 (from the bottom): black there, white in the margin.
        assert_eq!(full.pixels[(396 * 612 + 306) as usize], 0);
        assert_eq!(full.pixels[(20 * 612 + 20) as usize], 255);

        let cropped = pdf.render(0, 612, 1000, true).expect("render");
        // Cropped to the minimum share of the page around the square: ink reaches further out.
        let ink = cropped.pixels.iter().filter(|&&p| p < 128).count() as f32;
        let full_ink = full.pixels.iter().filter(|&&p| p < 128).count() as f32;
        assert!(ink / (cropped.pixels.len() as f32) > 2.0 * full_ink / (full.pixels.len() as f32));

        let blank = pdf.render(1, 100, 100, true).expect("blank page renders whole");
        assert!(blank.pixels.iter().all(|&p| p == 255));
        assert_eq!((blank.width, blank.height), (77, 100));
    }
}
