//! Book images: decode JPEG/PNG/GIF to 8-bit grayscale, fit them to a box and blit them.
//!
//! Resampling happens in linear light (sRGB decoded to linear, filtered, re-encoded), so
//! downscaled line art and halftones keep their apparent brightness. Alpha is composited on
//! white, the page colour.

use std::num::NonZeroU64;
use std::sync::OnceLock;

use omero_device::Canvas;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_core::result::DecodingResult;

/// Largest accepted width or height of a source image.
pub const MAX_DIM: u32 = 8192;
/// Largest accepted pixel count of a source image.
// ponytail: 32 Mpx keeps a worst-case RGBA decode at 128 MiB on a 512 MiB device. A tiled or
// streaming decoder would lift it, but no book ships images anywhere near this.
const MAX_PIXELS: u64 = 1 << 25;
/// Never enlarge a small image more than this, or it turns to mush.
const MAX_UPSCALE: f32 = 2.0;

/// 8-bit grayscale image, row-major, no padding (0 = black, 255 = white).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrayImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Image file formats we recognise by their magic bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Png,
    Gif,
    /// Recognised (for file names) but not decoded.
    Bmp,
}

impl Kind {
    pub fn extension(self) -> &'static str {
        match self {
            Kind::Jpeg => "jpg",
            Kind::Png => "png",
            Kind::Gif => "gif",
            Kind::Bmp => "bmp",
        }
    }
}

/// The image format of `bytes`, by magic: the one sniff [`decode`] and [`dimensions`] agree on.
pub fn kind(bytes: &[u8]) -> Option<Kind> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Kind::Jpeg)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Kind::Png)
    } else if bytes.starts_with(b"GIF8") {
        Some(Kind::Gif)
    } else if bytes.starts_with(b"BM") {
        Some(Kind::Bmp)
    } else {
        None
    }
}

/// Decodes a JPEG, PNG or GIF (first frame) and fits it inside `max_w`×`max_h`, keeping the
/// aspect ratio and enlarging at most 2×. Returns `None` for unsupported formats (WebP, SVG),
/// corrupt data, or images larger than [`MAX_DIM`].
pub fn decode(bytes: &[u8], max_w: u32, max_h: u32) -> Option<GrayImage> {
    if max_w == 0 || max_h == 0 {
        return None;
    }
    let src = match kind(bytes)? {
        Kind::Jpeg => decode_jpeg(bytes)?,
        Kind::Png => decode_png(bytes)?,
        Kind::Gif => decode_gif(bytes)?,
        Kind::Bmp => return None,
    };
    let (w, h) = fit(src.width, src.height, max_w, max_h);
    Some(resample(src, w, h))
}

/// Pixel size of a JPEG, PNG or GIF read from its header, without decoding. `None` for other
/// formats, truncated headers, or sizes [`decode`] would refuse.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| Some(u32::from(u16::from_be_bytes(bytes.get(i..i + 2)?.try_into().ok()?)));
    let (w, h) = match kind(bytes)? {
        Kind::Png if bytes.get(12..16) == Some(b"IHDR") => {
            let be32 = |i: usize| Some(u32::from_be_bytes(bytes.get(i..i + 4)?.try_into().ok()?));
            (be32(16)?, be32(20)?)
        }
        Kind::Gif => {
            let le16 = |i: usize| Some(u32::from(u16::from_le_bytes(bytes.get(i..i + 2)?.try_into().ok()?)));
            (le16(6)?, le16(8)?)
        }
        Kind::Jpeg => jpeg_frame(bytes, be16)?,
        Kind::Png | Kind::Bmp => return None,
    };
    within_limits(w as usize, h as usize).then_some((w, h))
}

/// Width and height from a JPEG's start-of-frame segment.
fn jpeg_frame(bytes: &[u8], be16: impl Fn(usize) -> Option<u32>) -> Option<(u32, u32)> {
    let mut i = 2;
    loop {
        // Markers may be preceded by any number of 0xFF fill bytes.
        while *bytes.get(i)? == 0xFF && *bytes.get(i + 1)? == 0xFF {
            i += 1;
        }
        if *bytes.get(i)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(i + 1)?;
        match marker {
            // Standalone markers: TEM, RSTn.
            0x01 | 0xD0..=0xD7 => i += 2,
            // SOF0–SOF15, except DHT (C4), JPG (C8) and DAC (CC): length, precision, height, width.
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => return Some((be16(i + 7)?, be16(i + 5)?)),
            // End of image or start of scan before any frame header.
            0xD9 | 0xDA => return None,
            _ => i += 2 + usize::try_from(be16(i + 2)?).ok()?,
        }
    }
}

/// Output size of a `w`×`h` image fitted inside `max_w`×`max_h`.
fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let scale = (max_w as f32 / w as f32).min(max_h as f32 / h as f32).min(MAX_UPSCALE);
    let side = |v: u32, max: u32| ((v as f32 * scale).round() as u32).clamp(1, max);
    (side(w, max_w), side(h, max_h))
}

/// A decoded source: interleaved 8-bit samples, 1 (L), 2 (LA), 3 (RGB) or 4 (RGBA) channels.
struct Raw {
    width: u32,
    height: u32,
    channels: usize,
    data: Vec<u8>,
}

fn within_limits(w: usize, h: usize) -> bool {
    w > 0 && h > 0 && w <= MAX_DIM as usize && h <= MAX_DIM as usize && (w as u64) * (h as u64) <= MAX_PIXELS
}

fn decoder_options() -> DecoderOptions {
    DecoderOptions::default().set_max_width(MAX_DIM as usize).set_max_height(MAX_DIM as usize)
}

fn decode_jpeg(bytes: &[u8]) -> Option<Raw> {
    // Luma output skips chroma upsampling and colour conversion entirely.
    // ponytail: zune-jpeg has no DCT-domain downscaling; a 1600×2400 cover decodes at full size
    // then gets filtered. If covers prove slow on the device, add 1/2–1/8 IDCT scaling upstream.
    let options = decoder_options().jpeg_set_out_colorspace(ColorSpace::Luma);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    dec.decode_headers().ok()?;
    let info = dec.info()?;
    let (w, h) = (usize::from(info.width), usize::from(info.height));
    if !within_limits(w, h) {
        return None;
    }
    let data = dec.decode().ok()?;
    raw(w, h, 1, data)
}

fn decode_png(bytes: &[u8]) -> Option<Raw> {
    let options = decoder_options().png_set_strip_to_8bit(true);
    let mut dec = zune_png::PngDecoder::new_with_options(ZCursor::new(bytes), options);
    dec.decode_headers().ok()?;
    let (w, h) = dec.dimensions()?;
    if !within_limits(w, h) {
        return None;
    }
    let channels = dec.colorspace()?.num_components();
    match dec.decode().ok()? {
        DecodingResult::U8(data) => raw(w, h, channels, data),
        _ => None,
    }
}

fn decode_gif(bytes: &[u8]) -> Option<Raw> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    options.set_memory_limit(gif::MemoryLimit::Bytes(NonZeroU64::new(MAX_PIXELS * 4)?));
    let mut dec = options.read_info(bytes).ok()?;
    let (w, h) = (usize::from(dec.width()), usize::from(dec.height()));
    if !within_limits(w, h) {
        return None;
    }
    let frame = dec.read_next_frame().ok()??;
    // The first frame may cover only part of the logical screen; the rest stays transparent.
    let mut data = vec![0u8; w * h * 4];
    let (fx, fy, fw) = (usize::from(frame.left), usize::from(frame.top), usize::from(frame.width));
    let n = fw.min(w.saturating_sub(fx)) * 4;
    if n == 0 {
        return raw(w, h, 4, data);
    }
    for (row, src) in frame.buffer.chunks_exact(fw * 4).enumerate().take(h.saturating_sub(fy)) {
        let start = ((fy + row) * w + fx) * 4;
        data[start..start + n].copy_from_slice(&src[..n]);
    }
    raw(w, h, 4, data)
}

fn raw(w: usize, h: usize, channels: usize, data: Vec<u8>) -> Option<Raw> {
    (data.len() >= w * h * channels && (1..=4).contains(&channels)).then_some(Raw {
        width: w as u32,
        height: h as u32,
        channels,
        data,
    })
}

/// sRGB byte → linear light.
fn to_linear() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}

const ENCODE_STEPS: usize = 4096;

/// Linear light (quantized to 12 bits) → sRGB byte.
fn to_srgb(v: f32) -> u8 {
    static LUT: OnceLock<Vec<u8>> = OnceLock::new();
    let lut = LUT.get_or_init(|| {
        (0..ENCODE_STEPS)
            .map(|i| {
                let l = i as f32 / (ENCODE_STEPS - 1) as f32;
                let c = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
                (c * 255.0).round().clamp(0.0, 255.0) as u8
            })
            .collect()
    });
    let i = (v.clamp(0.0, 1.0) * (ENCODE_STEPS - 1) as f32).round() as usize;
    lut[i.min(ENCODE_STEPS - 1)]
}

/// One source row as linear-light gray, alpha composited on white.
fn linear_row(src: &[u8], channels: usize, out: &mut [f32]) {
    let lin = to_linear();
    let gray = |p: &[u8]| 0.2126 * lin[p[0] as usize] + 0.7152 * lin[p[1] as usize] + 0.0722 * lin[p[2] as usize];
    let over_white = |v: f32, a: u8| {
        let a = f32::from(a) / 255.0;
        v * a + (1.0 - a)
    };
    for (o, p) in out.iter_mut().zip(src.chunks_exact(channels)) {
        *o = match channels {
            1 => lin[p[0] as usize],
            2 => over_white(lin[p[0] as usize], p[1]),
            3 => gray(p),
            _ => over_white(gray(p), p[3]),
        };
    }
}

/// Catmull-Rom cubic: sharp, with mild overshoot (clamped on output).
fn cubic(x: f32) -> f32 {
    let x = x.abs();
    if x < 1.0 {
        1.5 * x * x * x - 2.5 * x * x + 1.0
    } else if x < 2.0 {
        -0.5 * x * x * x + 2.5 * x * x - 4.0 * x + 2.0
    } else {
        0.0
    }
}

/// Filter taps mapping `src_len` samples onto `dst_len`: for each output sample, the first
/// source index and its normalized weights. When shrinking, the kernel widens by the scale
/// factor so every source pixel contributes (area averaging).
fn taps(src_len: u32, dst_len: u32) -> Vec<(usize, Vec<f32>)> {
    let ratio = src_len as f32 / dst_len as f32;
    let widen = ratio.max(1.0);
    let support = 2.0 * widen;
    (0..dst_len)
        .map(|i| {
            let center = (i as f32 + 0.5) * ratio;
            let lo = ((center - support).floor().max(0.0)) as usize;
            let hi = ((center + support).ceil() as usize).min(src_len as usize);
            let mut w: Vec<f32> = (lo..hi).map(|j| cubic((j as f32 + 0.5 - center) / widen)).collect();
            let sum: f32 = w.iter().sum();
            if sum.abs() > f32::EPSILON {
                w.iter_mut().for_each(|v| *v /= sum);
            }
            (lo, w)
        })
        .collect()
}

/// Separable resample in linear light: horizontal pass row by row (never materializing the
/// full-size float image), then vertical.
fn resample(src: Raw, dst_w: u32, dst_h: u32) -> GrayImage {
    let (sw, sh, ch) = (src.width as usize, src.height as usize, src.channels);
    if ch == 1 && (dst_w, dst_h) == (src.width, src.height) {
        let mut pixels = src.data;
        pixels.truncate(sw * sh);
        return GrayImage { width: dst_w, height: dst_h, pixels };
    }
    let (dw, dh) = (dst_w as usize, dst_h as usize);
    let htaps = taps(src.width, dst_w);
    let mut row = vec![0f32; sw];
    let mut mid = vec![0f32; dw * sh];
    for (y, src_row) in src.data.chunks_exact(sw * ch).take(sh).enumerate() {
        linear_row(src_row, ch, &mut row);
        for (out, (lo, w)) in mid[y * dw..(y + 1) * dw].iter_mut().zip(&htaps) {
            *out = w.iter().zip(&row[*lo..]).map(|(w, v)| w * v).sum();
        }
    }
    let mut pixels = Vec::with_capacity(dw * dh);
    let mut acc = vec![0f32; dw];
    for (lo, w) in taps(src.height, dst_h) {
        acc.fill(0.0);
        for (k, wk) in w.iter().enumerate() {
            let line = &mid[(lo + k) * dw..(lo + k + 1) * dw];
            acc.iter_mut().zip(line).for_each(|(a, v)| *a += wk * v);
        }
        pixels.extend(acc.iter().map(|&v| to_srgb(v)));
    }
    GrayImage { width: dst_w, height: dst_h, pixels }
}

/// Floyd–Steinberg dither down to the 16 gray levels the e-ink panel shows (multiples of 17),
/// so gradients band less than plain rounding.
pub fn quantize16(img: &mut GrayImage) {
    let w = img.width as usize;
    if w == 0 {
        return;
    }
    // Error carried into the current and next row, offset by one so x-1 never underflows.
    let mut cur = vec![0i16; w + 2];
    let mut next = vec![0i16; w + 2];
    for row in img.pixels.chunks_exact_mut(w) {
        for (x, p) in row.iter_mut().enumerate() {
            let v = (i16::from(*p) + cur[x + 1] / 16).clamp(0, 255);
            let q = (v + 8) / 17 * 17;
            *p = q as u8;
            let e = v - q;
            cur[x + 2] += e * 7;
            next[x] += e * 3;
            next[x + 1] += e * 5;
            next[x + 2] += e;
        }
        std::mem::swap(&mut cur, &mut next);
        next.fill(0);
    }
}

/// Copies `img` onto `canvas` with its top-left corner at (`x`, `y`), clipped to the canvas.
pub fn draw(canvas: &mut Canvas, img: &GrayImage, x: i32, y: i32) {
    let (iw, ih) = (img.width as i64, img.height as i64);
    let (x, y) = (i64::from(x), i64::from(y));
    let x0 = x.max(0);
    let x1 = (x + iw).min(i64::from(canvas.width));
    let y0 = y.max(0);
    let y1 = (y + ih).min(i64::from(canvas.height));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let n = (x1 - x0) as usize;
    for cy in y0..y1 {
        let src = ((cy - y) * iw + (x0 - x)) as usize;
        let dst = cy as usize * canvas.stride + x0 as usize;
        canvas.pixels[dst..dst + n].copy_from_slice(&img.pixels[src..src + n]);
    }
}

/// Decoded, dithered images by (archive path, size), most recent last; holds the few the
/// screen around the current page needs.
#[derive(Debug, Default)]
pub struct ImageCache {
    entries: Vec<(String, u32, u32, Option<GrayImage>)>,
}

impl ImageCache {
    /// How many decoded images are kept: a page rarely shows more than two.
    const CAPACITY: usize = 4;

    /// The image `src` fitted to `w`×`h` and dithered to 16 grays, decoding the bytes from
    /// `read` on a miss. Failures are cached too, so a broken image is read only once.
    pub fn get(&mut self, src: &str, w: u32, h: u32, read: impl FnOnce() -> Option<Vec<u8>>) -> Option<&GrayImage> {
        let hit = self.entries.iter().position(|(s, ew, eh, _)| s == src && *ew == w && *eh == h);
        let entry = match hit {
            Some(i) => self.entries.remove(i),
            None => {
                let img = read().and_then(|bytes| decode(&bytes, w, h)).map(|mut img| {
                    quantize16(&mut img);
                    img
                });
                if self.entries.len() >= Self::CAPACITY {
                    self.entries.remove(0);
                }
                (src.to_string(), w, h, img)
            }
        };
        self.entries.push(entry);
        self.entries.last().and_then(|e| e.3.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u16, h: u16, rgb: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let enc = jpeg_encoder::Encoder::new(&mut out, 95);
        enc.encode(rgb, w, h, jpeg_encoder::ColorType::Rgb).expect("encode jpeg");
        out
    }

    fn png(w: usize, h: usize, cs: ColorSpace, data: &[u8]) -> Vec<u8> {
        let opts = zune_core::options::EncoderOptions::new(w, h, cs, zune_core::bit_depth::BitDepth::Eight);
        let mut out = Vec::new();
        zune_png::PngEncoder::new(data, opts).encode(&mut out).expect("encode png");
        out
    }

    #[test]
    fn fit_keeps_aspect_and_caps_upscale() {
        assert_eq!(fit(1600, 2400, 300, 450), (300, 450));
        assert_eq!(fit(1600, 2400, 1000, 1000), (667, 1000));
        assert_eq!(fit(10, 20, 1000, 1000), (20, 40)); // 2× at most
        assert_eq!(fit(10_000, 1, 100, 100), (100, 1)); // never collapses to 0
    }

    #[test]
    fn jpeg_decodes_to_luma_and_scales() {
        // Left half black, right half white.
        let (w, h) = (64u16, 32u16);
        let rgb: Vec<u8> = (0..w as usize * h as usize).flat_map(|i| [if i % 64 < 32 { 0 } else { 255 }; 3]).collect();
        let img = decode(&jpeg(w, h, &rgb), 16, 16).expect("decode");
        assert_eq!((img.width, img.height), (16, 8));
        assert_eq!(img.pixels.len(), 16 * 8);
        assert!(img.pixels[0] < 20, "left is dark: {}", img.pixels[0]);
        assert!(img.pixels[15] > 235, "right is light: {}", img.pixels[15]);
    }

    #[test]
    fn downscale_is_gamma_correct() {
        // 1-px black/white checkerboard: in linear light the average is 50% luminance,
        // which is sRGB ~188, not the naive 128.
        let (w, h) = (64usize, 64usize);
        let data: Vec<u8> = (0..w * h).map(|i| if (i % w + i / w) % 2 == 0 { 0 } else { 255 }).collect();
        let img = decode(&png(w, h, ColorSpace::Luma, &data), 8, 8).expect("decode");
        let mean = img.pixels.iter().map(|&p| u32::from(p)).sum::<u32>() / img.pixels.len() as u32;
        assert!((182..=194).contains(&mean), "mean {mean}");
    }

    #[test]
    fn png_alpha_composites_on_white() {
        // Fully transparent black → white; opaque black stays black.
        let data = [0, 0, 0, 0, 0, 0, 0, 255];
        let img = decode(&png(2, 1, ColorSpace::RGBA, &data), 2, 1).expect("decode");
        assert_eq!(img.pixels, vec![255, 0]);
        let la = [0, 0, 0, 255];
        let img = decode(&png(2, 1, ColorSpace::LumaA, &la), 2, 1).expect("decode");
        assert_eq!(img.pixels, vec![255, 0]);
    }

    #[test]
    fn png_rgb_uses_luminance_weights() {
        let data = [255, 0, 0, 0, 255, 0];
        let img = decode(&png(2, 1, ColorSpace::RGB, &data), 2, 1).expect("decode");
        // Pure green is much brighter than pure red.
        assert!(img.pixels[1] > img.pixels[0] + 50, "{:?}", img.pixels);
    }

    #[test]
    fn gif_first_frame_partial_on_white() {
        let mut out = Vec::new();
        {
            let palette = [0, 0, 0, 255, 255, 255];
            let mut enc = gif::Encoder::new(&mut out, 4, 2, &palette).expect("encoder");
            let mut frame = gif::Frame::from_palette_pixels(2, 1, vec![0, 0], palette.to_vec(), None);
            frame.left = 1;
            frame.top = 1;
            enc.write_frame(&frame).expect("frame");
        }
        let img = decode(&out, 4, 2).expect("decode");
        assert_eq!(img.pixels, vec![255, 255, 255, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn dimensions_from_headers() {
        assert_eq!(dimensions(&jpeg(37, 21, &[128; 37 * 21 * 3])), Some((37, 21)));
        assert_eq!(dimensions(&png(5, 9, ColorSpace::RGB, &[0; 5 * 9 * 3])), Some((5, 9)));
        let mut gif = Vec::new();
        gif::Encoder::new(&mut gif, 300, 2, &[0, 0, 0]).expect("encoder");
        assert_eq!(dimensions(&gif), Some((300, 2)));
        assert_eq!(dimensions(b"<svg/>"), None);
        let full = jpeg(16, 16, &[128; 16 * 16 * 3]);
        for n in 0..full.len() {
            let _ = dimensions(&full[..n]);
        }
    }

    #[test]
    fn cache_keeps_recent_and_remembers_failures() {
        let bytes = png(4, 4, ColorSpace::Luma, &[0; 16]);
        let mut cache = ImageCache::default();
        assert_eq!(cache.get("a", 2, 2, || Some(bytes.clone())).map(|i| i.width), Some(2));
        assert!(cache.get("a", 2, 2, || panic!("cached")).is_some());
        assert!(cache.get("bad", 2, 2, || Some(b"junk".to_vec())).is_none());
        assert!(cache.get("bad", 2, 2, || panic!("failure cached")).is_none());
        for i in 0..ImageCache::CAPACITY {
            cache.get(&i.to_string(), 2, 2, || None);
        }
        assert!(cache.get("a", 2, 2, || None).is_none(), "evicted");
    }

    #[test]
    fn rejects_garbage_and_unsupported() {
        assert!(decode(b"", 10, 10).is_none());
        assert!(decode(b"RIFF\0\0\0\0WEBPVP8 ", 10, 10).is_none());
        assert!(decode(b"<svg xmlns='http://www.w3.org/2000/svg'/>", 10, 10).is_none());
        assert!(decode(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10], 10, 10).is_none());
        assert!(decode(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", 10, 10).is_none());
        let ok = png(1, 1, ColorSpace::Luma, &[0]);
        assert!(decode(&ok, 0, 10).is_none());
        // Truncations of a valid file never panic.
        let full = jpeg(16, 16, &[128; 16 * 16 * 3]);
        for n in 0..full.len() {
            let _ = decode(&full[..n], 8, 8);
        }
    }

    #[test]
    fn rejects_oversized_header() {
        // IHDR claiming 9000×9000: refused before any allocation.
        let mut ihdr = Vec::from(&b"IHDR"[..]);
        ihdr.extend_from_slice(&9000u32.to_be_bytes());
        ihdr.extend_from_slice(&9000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        let mut bytes = Vec::from(&b"\x89PNG\r\n\x1a\n"[..]);
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(&ihdr);
        bytes.extend_from_slice(&crc32(&ihdr).to_be_bytes());
        assert!(decode(&bytes, 100, 100).is_none());
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    #[test]
    fn quantize16_hits_panel_levels_and_preserves_mean() {
        let mut img = GrayImage { width: 64, height: 64, pixels: vec![100; 64 * 64] };
        quantize16(&mut img);
        assert!(img.pixels.iter().all(|p| p % 17 == 0));
        let mean = img.pixels.iter().map(|&p| u32::from(p)).sum::<u32>() as f32 / 4096.0;
        assert!((mean - 100.0).abs() < 1.5, "mean {mean}");
        let mut extremes = GrayImage { width: 2, height: 1, pixels: vec![0, 255] };
        quantize16(&mut extremes);
        assert_eq!(extremes.pixels, vec![0, 255]);
    }

    #[test]
    fn draw_clips_on_every_side() {
        let img = GrayImage { width: 3, height: 3, pixels: (1..=9).collect() };
        let mut buf = vec![0u8; 5 * 4];
        let mut c = Canvas { width: 4, height: 4, stride: 5, pixels: &mut buf };
        draw(&mut c, &img, -1, -1);
        draw(&mut c, &img, 3, 3);
        draw(&mut c, &img, 100, 0);
        draw(&mut c, &img, i32::MIN, i32::MAX);
        assert_eq!(&buf[0..5], &[5, 6, 0, 0, 0]);
        assert_eq!(&buf[5..10], &[8, 9, 0, 0, 0]);
        assert_eq!(&buf[15..20], &[0, 0, 0, 1, 0]); // stride padding untouched
    }

    /// Timing of a 1600×2400 cover: `cargo test --release -p omero-engine bench_cover -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_cover() {
        let (w, h) = (1600u16, 2400u16);
        let rgb: Vec<u8> = (0..u32::from(w) * u32::from(h))
            .flat_map(|i| {
                let (x, y) = (i % 1600, i / 1600);
                let n = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) % 64;
                [((x / 7 + n) % 256) as u8, ((y / 9 + n) % 256) as u8, ((x + y) / 16 % 256) as u8]
            })
            .collect();
        let bytes = jpeg(w, h, &rgb);
        for (mw, mh) in [(1236, 1648), (300, 450)] {
            let t = std::time::Instant::now();
            let runs = 10;
            for _ in 0..runs {
                std::hint::black_box(decode(&bytes, mw, mh));
            }
            eprintln!("{w}x{h} jpeg ({} KiB) -> fit {mw}x{mh}: {:?}", bytes.len() / 1024, t.elapsed() / runs);
        }
        let t = std::time::Instant::now();
        std::hint::black_box(decode_jpeg(&bytes));
        eprintln!("luma decode alone: {:?}", t.elapsed());
    }

    /// Decodes the cover of every local EPUB in `dev-books/` (not in the repo).
    #[test]
    fn dev_books_covers() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev-books");
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for path in entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "epub")) {
            let file = std::fs::File::open(&path).expect("open");
            let mut epub = crate::epub::Epub::open(std::io::BufReader::new(file)).expect("epub");
            let Some(cover) = epub.cover.clone() else { continue };
            let bytes = epub.read(&cover).expect("read cover");
            let t = std::time::Instant::now();
            let img = decode(&bytes, 1236, 1648).unwrap_or_else(|| panic!("{}: {cover}", path.display()));
            eprintln!("{}: {cover} -> {}x{} in {:?}", path.display(), img.width, img.height, t.elapsed());
            assert!(img.width <= 1236 && img.height <= 1648);
        }
    }
}
