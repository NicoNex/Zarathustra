//! Cascade: computed style for every element of a chapter.
//!
//! Order: user-agent < author < author `!important` < user-agent `!important`, then
//! specificity, then source order; inline `style` beats author rules. Inheritance follows
//! CSS. Last, the reader's preferences override typography the way Readium CSS does:
//! the user picks font, size, line height and justification; the publisher keeps structure.

use super::css::{
    self, Declaration, Display, FontSize, Length, LineHeight, ListStyle, Property, TextAlign, TextTransform,
    VerticalAlign, WhiteSpace,
};
use super::select::{self, El};
use crate::dom::{Dom, NodeId};
use std::rc::Rc;
use std::sync::LazyLock;

/// Browser-like defaults tuned for books.
const USER_AGENT_CSS: &str = r#"
html, body, div, p, section, article, aside, header, footer, nav, main, blockquote, figure, figcaption,
h1, h2, h3, h4, h5, h6, ul, ol, dl, dt, dd, hr, pre, address, center, hgroup, details, summary { display: block }
li { display: list-item }
table { display: table } tr { display: table-row } td, th { display: table-cell }
head, script, style, title, meta, link, template, noscript { display: none }
aside[epub|type~=footnote], aside[epub|type~=endnote], aside[epub|type~=rearnote] { display: none }
p { margin: 1em 0 }
h1 { font-size: 2em; margin: .67em 0 } h2 { font-size: 1.5em; margin: .83em 0 } h3 { font-size: 1.17em; margin: 1em 0 }
h4 { margin: 1.33em 0 } h5 { font-size: .83em; margin: 1.67em 0 } h6 { font-size: .67em; margin: 2.33em 0 }
h1, h2, h3, h4, h5, h6, b, strong, th, dt { font-weight: bold }
h1, h2, h3, h4, h5, h6 { page-break-inside: avoid; hyphens: manual }
i, em, cite, var, dfn, address { font-style: italic }
small { font-size: smaller } big { font-size: larger }
sub { vertical-align: sub; font-size: smaller } sup { vertical-align: super; font-size: smaller }
code, kbd, samp, tt, pre { font-family: monospace }
pre { white-space: pre; margin: 1em 0 }
blockquote { margin: 1em 2em } figure { margin: 1em 2em } dd { margin-left: 2em }
ul, ol { margin: 1em 0; padding-left: 2em } ul { list-style-type: disc } ol { list-style-type: decimal }
ul ul, ol ol, ul ol, ol ul { margin: 0 }
hr { margin: .5em 0 } center, th { text-align: center } td, th { padding: 0 .25em }
"#;

/// CSS `line-height: normal`.
const NORMAL_LINE_HEIGHT: f32 = 1.2;
/// Line height when neither the publisher nor the reader sets one.
const DEFAULT_LINE_HEIGHT: f32 = 1.4;

/// The reader's choices, applied over publisher styles.
#[derive(Debug, Clone, PartialEq)]
pub struct Preferences {
    /// Base text size in device pixels.
    pub font_size: f32,
    /// Reading font family; `None` keeps the publisher's.
    pub font_family: Option<String>,
    /// Line height as a multiple of the font size; `None` keeps the publisher's.
    pub line_height: Option<f32>,
    /// Alignment of body text (paragraphs set start, left or justified); `None` as published.
    /// Text the publisher centred or set right keeps its alignment.
    pub align: Option<TextAlign>,
    pub hyphenate: bool,
    /// Width of the text area in device pixels.
    pub page_width: f32,
}

/// Specified line height as CSS inherits it: factors scale with each element's font size,
/// lengths stay absolute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineSpec {
    Factor(f32),
    Px(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputedStyle {
    pub display: Display,
    /// Shared: a refcount bump per element, not a copy.
    pub font_family: Rc<[String]>,
    /// The family is monospace (code), which the reader font doesn't replace.
    pub monospace: bool,
    /// Device pixels.
    pub font_size: f32,
    pub italic: bool,
    pub font_weight: u16,
    pub small_caps: bool,
    pub line_spec: LineSpec,
    /// Device pixels, resolved from `line_spec`.
    pub line_height: f32,
    pub text_align: TextAlign,
    pub text_indent: f32,
    /// Top, right, bottom, left in device pixels; `None` = auto.
    pub margin: [Option<f32>; 4],
    pub padding: [f32; 4],
    pub break_before: bool,
    pub break_after: bool,
    pub break_inside_avoid: bool,
    pub white_space: WhiteSpace,
    pub vertical_align: VerticalAlign,
    pub list_style: ListStyle,
    pub text_transform: TextTransform,
    pub letter_spacing: f32,
    pub hyphens: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// Width available to this element's children (the containing block for percentages).
    pub content_width: f32,
}

impl ComputedStyle {
    fn initial(p: &Preferences) -> ComputedStyle {
        let line_spec = LineSpec::Factor(p.line_height.unwrap_or(DEFAULT_LINE_HEIGHT));
        ComputedStyle {
            display: Display::Block,
            font_family: Rc::from(["serif".to_string()]),
            monospace: false,
            font_size: p.font_size,
            italic: false,
            font_weight: 400,
            small_caps: false,
            line_spec,
            line_height: 0.0,
            text_align: TextAlign::Start,
            text_indent: 0.0,
            margin: [Some(0.0); 4],
            padding: [0.0; 4],
            break_before: false,
            break_after: false,
            break_inside_avoid: false,
            white_space: WhiteSpace::Normal,
            vertical_align: VerticalAlign::Baseline,
            list_style: ListStyle::Disc,
            text_transform: TextTransform::None,
            letter_spacing: 0.0,
            hyphens: true,
            width: None,
            height: None,
            content_width: p.page_width,
        }
    }

    /// Inherited properties carry over; the rest reset to initial values.
    fn inherit(&self) -> ComputedStyle {
        ComputedStyle {
            display: Display::Inline,
            font_family: Rc::clone(&self.font_family),
            margin: [Some(0.0); 4],
            padding: [0.0; 4],
            break_before: false,
            break_after: false,
            break_inside_avoid: false,
            vertical_align: VerticalAlign::Baseline,
            width: None,
            height: None,
            ..*self
        }
    }
}

/// Code fonts: `monospace`, Courier, and families with a whole word `Mono` (not "Monotype").
fn is_monospace(families: &[String]) -> bool {
    families.iter().any(|f| {
        f.eq_ignore_ascii_case("monospace")
            || f.to_ascii_lowercase().contains("courier")
            || f.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w.eq_ignore_ascii_case("mono"))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Origin {
    UserAgent,
    Author,
}

/// The user-agent sheet is constant: parsed once per process.
static USER_AGENT: LazyLock<Vec<css::Rule>> = LazyLock::new(|| css::parse_stylesheet(USER_AGENT_CSS));

/// A chapter's author stylesheets, in document order (the UA sheet is implied).
#[derive(Default)]
pub struct Stylesheets {
    author: Vec<Vec<css::Rule>>,
}

impl Stylesheets {
    pub fn add_author(&mut self, css_text: &str) {
        self.author.push(css::parse_stylesheet(css_text));
    }

    fn sheets(&self) -> impl Iterator<Item = (Origin, &[css::Rule])> {
        std::iter::once((Origin::UserAgent, USER_AGENT.as_slice()))
            .chain(self.author.iter().map(|r| (Origin::Author, r.as_slice())))
    }
}

/// Sort key: (font-size last?, level, specificity, source order). Font-size declarations
/// sort first so `em` elsewhere refers to the element's own size; within each group,
/// higher keys apply later and win.
type Key = (bool, u8, u32, u32);

fn level(origin: Origin, important: bool) -> u8 {
    match (origin, important) {
        (Origin::UserAgent, false) => 0,
        (Origin::Author, false) => 1,
        (Origin::Author, true) => 3,
        (Origin::UserAgent, true) => 4,
    }
}

struct Ctx<'a> {
    prefs: &'a Preferences,
    /// The root element's font size: what `rem` refers to.
    root_font_size: f32,
}

/// Computes styles for every element of `dom` (`None` for text and the document node).
// ponytail: every rule is tested against every element (O(elements × rules), ~30 ms per chapter
// on the Kindle with real books); bucket rules by rightmost id/class/tag if big stylesheets lag.
pub fn compute(dom: &Dom, sheets: &Stylesheets, prefs: &Preferences) -> Vec<Option<ComputedStyle>> {
    let mut styles: Vec<Option<ComputedStyle>> = vec![None; dom.len()];
    let initial = ComputedStyle::initial(prefs);
    let mut ctx = Ctx { prefs, root_font_size: prefs.font_size };
    let mut caches = Default::default();
    let mut seen_root = false;

    for id in dom.descendants(Dom::ROOT) {
        if dom.name(id).is_none() {
            continue;
        }
        let parent = parent_style(dom, &styles, id).unwrap_or(&initial);
        let mut style = parent.inherit();

        // Borrows both the sheets and this element's inline declarations, so it lives per element.
        let inline = dom.attr(id, "style").map(css::parse_declarations).unwrap_or_default();
        let mut matched: Vec<(Key, &Declaration)> = Vec::new();
        let mut order = 0u32;
        for (origin, rules) in sheets.sheets() {
            for rule in rules {
                order += 1;
                if let Some(spec) = select::matches(&rule.selectors, El { dom, id }, &mut caches) {
                    for d in &rule.declarations {
                        let other = !matches!(d.property, Property::FontSize(_));
                        matched.push(((other, level(origin, d.important), spec, order), d));
                    }
                }
            }
        }
        for d in &inline {
            // Inline style: author level, above any selector specificity.
            let other = !matches!(d.property, Property::FontSize(_));
            matched.push(((other, level(Origin::Author, d.important), u32::MAX, u32::MAX), d));
        }
        matched.sort_by_key(|(k, _)| *k);
        for (_, d) in &matched {
            apply(&mut style, parent, &d.property, &ctx);
        }
        apply_preferences(&mut style, prefs);
        style.line_height = match style.line_spec {
            LineSpec::Factor(f) => f * style.font_size,
            LineSpec::Px(px) => px,
        };
        if !matches!(style.display, Display::Inline) {
            let [_, r, _, l] = style.margin;
            let inset = r.unwrap_or(0.0) + l.unwrap_or(0.0) + style.padding[1] + style.padding[3];
            style.content_width = style.width.unwrap_or(parent.content_width - inset).max(0.0);
        }
        if !seen_root {
            seen_root = true;
            ctx.root_font_size = style.font_size;
        }
        styles[id.index()] = Some(style);
    }
    styles
}

fn parent_style<'a>(dom: &Dom, styles: &'a [Option<ComputedStyle>], id: NodeId) -> Option<&'a ComputedStyle> {
    styles[dom.parent(id)?.index()].as_ref()
}

/// Resolves a length in device pixels. Absolute CSS px scale with the reader's text size
/// (16 CSS px = the base size), so a publisher's fixed sizes follow the size slider.
/// `percent_of` is `None` where a percentage has no definite basis (it then computes to auto).
fn resolve(len: Length, font_size: f32, ctx: &Ctx, percent_of: Option<f32>) -> Option<f32> {
    Some(match len {
        Length::Px(v) => v * ctx.prefs.font_size / 16.0,
        Length::Em(v) => v * font_size,
        Length::Rem(v) => v * ctx.root_font_size,
        Length::Percent(v) => v / 100.0 * percent_of?,
        Length::Auto => return None,
    })
}

fn apply(s: &mut ComputedStyle, parent: &ComputedStyle, p: &Property, ctx: &Ctx) {
    let cb = Some(parent.content_width);
    match p {
        Property::Display(d) => s.display = *d,
        Property::FontFamily(f) => {
            s.monospace = is_monospace(f);
            s.font_family = Rc::from(f.as_slice());
        }
        Property::FontSize(fs) => {
            let pf = parent.font_size;
            s.font_size = match *fs {
                FontSize::Length(Length::Em(v)) => v * pf,
                FontSize::Length(Length::Percent(v)) => v / 100.0 * pf,
                FontSize::Length(l) => resolve(l, pf, ctx, Some(pf)).unwrap_or(pf),
                FontSize::Keyword(k) => k * ctx.prefs.font_size,
                FontSize::Larger => pf * 1.2,
                FontSize::Smaller => pf / 1.2,
            };
        }
        Property::Italic(v) => s.italic = *v,
        Property::FontWeight(w) => s.font_weight = *w,
        Property::SmallCaps(v) => s.small_caps = *v,
        Property::LineHeight(lh) => {
            s.line_spec = match *lh {
                LineHeight::Normal => LineSpec::Factor(NORMAL_LINE_HEIGHT),
                LineHeight::Factor(f) => LineSpec::Factor(f),
                // em/% compute to an absolute length, which descendants inherit as is.
                LineHeight::Length(l) => match resolve(l, s.font_size, ctx, Some(s.font_size)) {
                    Some(px) => LineSpec::Px(px),
                    None => s.line_spec,
                },
            }
        }
        Property::TextAlign(a) => s.text_align = *a,
        Property::TextIndent(l) => s.text_indent = resolve(*l, s.font_size, ctx, cb).unwrap_or(0.0),
        Property::Margin(sd, l) => s.margin[*sd as usize] = resolve(*l, s.font_size, ctx, cb),
        Property::Padding(sd, l) => s.padding[*sd as usize] = resolve(*l, s.font_size, ctx, cb).unwrap_or(0.0),
        Property::BreakBefore(v) => s.break_before = *v,
        Property::BreakAfter(v) => s.break_after = *v,
        Property::BreakInsideAvoid(v) => s.break_inside_avoid = *v,
        Property::WhiteSpace(w) => s.white_space = *w,
        Property::VerticalAlign(v) => s.vertical_align = *v,
        Property::ListStyle(l) => s.list_style = *l,
        Property::TextTransform(t) => s.text_transform = *t,
        Property::LetterSpacing(l) => s.letter_spacing = resolve(*l, s.font_size, ctx, None).unwrap_or(0.0),
        Property::Hyphens(h) => s.hyphens = *h,
        Property::Width(l) => s.width = resolve(*l, s.font_size, ctx, cb),
        // ponytail: percentage heights have no definite containing height in reflowable text → auto.
        Property::Height(l) => s.height = resolve(*l, s.font_size, ctx, None),
    }
}

/// The reader wins on typography; structure (indents, emphasis, centred blocks) stays the publisher's.
fn apply_preferences(s: &mut ComputedStyle, prefs: &Preferences) {
    if let Some(family) = &prefs.font_family {
        if !s.monospace {
            s.font_family = Rc::from([family.clone()]);
        }
    }
    if let Some(factor) = prefs.line_height {
        s.line_spec = LineSpec::Factor(factor);
    }
    if let Some(align) = prefs.align {
        if matches!(s.text_align, TextAlign::Start | TextAlign::Left | TextAlign::Justify) {
            s.text_align = align;
        }
    }
    s.hyphens &= prefs.hyphenate;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs() -> Preferences {
        Preferences {
            font_size: 40.0,
            font_family: Some("Bookerly".into()),
            line_height: None,
            align: Some(TextAlign::Justify),
            hyphenate: true,
            page_width: 1000.0,
        }
    }

    fn style_of<'a>(dom: &Dom, styles: &'a [Option<ComputedStyle>], id_attr: &str) -> &'a ComputedStyle {
        let id = dom.descendants(Dom::ROOT).find(|&n| dom.attr(n, "id") == Some(id_attr)).expect("element");
        styles[id.index()].as_ref().expect("styled")
    }

    #[test]
    fn cascade_specificity_importance_inheritance_and_units() {
        let dom = Dom::parse_xml(
            r#"<html><body><div id="d" class="c">
            <p id="p1">a <em id="e">b</em></p>
            <p id="p2" class="note" style="margin-top: 10%">c</p>
            <h1 id="h">T</h1><pre id="code">x</pre></div></body></html>"#,
        )
        .expect("parses");
        let mut sheets = Stylesheets::default();
        sheets.add_author(
            "p { margin: 0; text-indent: 1.5em; font-size: 12px }
             .c p { text-align: left } .note { text-align: center !important; font-size: 150% }
             div p.note { text-align: right }
             h1 { font-family: 'Fancy Display'; font-size: 24pt; margin-bottom: 1rem }",
        );
        let styles = compute(&dom, &sheets, &prefs());

        let p1 = style_of(&dom, &styles, "p1");
        assert_eq!(p1.font_size, 30.0, "12 CSS px scales with the 40px base: 12/16·40");
        assert_eq!(p1.text_indent, 45.0, "1.5em of 30px");
        assert_eq!(p1.margin[0], Some(0.0));
        assert_eq!(p1.text_align, TextAlign::Justify, "left → justified by the reader's preference");
        assert_eq!(p1.font_family.as_ref(), ["Bookerly"]);

        let e = style_of(&dom, &styles, "e");
        assert!(e.italic, "UA em");
        assert_eq!(e.font_size, 30.0, "inherited");
        assert_eq!(e.text_indent, 45.0, "text-indent inherits");

        let p2 = style_of(&dom, &styles, "p2");
        assert_eq!(p2.text_align, TextAlign::Center, "!important beats higher specificity; centred stays");
        assert_eq!(p2.font_size, 60.0, "150% of the body's 40px (author p rule is overridden by .note)");
        assert_eq!(p2.margin[0], Some(100.0), "inline style, 10% of page width");

        let h = style_of(&dom, &styles, "h");
        assert_eq!(h.font_weight, 700);
        assert_eq!(h.font_size, 80.0, "24pt = 32 CSS px → 32/16·40");
        assert_eq!(h.margin[2], Some(40.0), "1rem = base size");
        assert_eq!(h.font_family.as_ref(), ["Bookerly"], "reader font wins over the publisher's");
        assert!(!h.hyphens, "headings don't hyphenate");

        let code = style_of(&dom, &styles, "code");
        assert!(code.monospace, "monospace survives the reader font");
        assert_eq!(code.font_family.as_ref(), ["monospace"]);
        assert_eq!(code.white_space, WhiteSpace::Pre);
    }

    #[test]
    fn rem_zero_sizes_line_height_lengths_and_containing_blocks() {
        let dom = Dom::parse_xml(
            r#"<html id="root"><body id="b"><div id="z"><span id="s">x</span></div>
            <p id="lh">a <small id="sm">b</small></p>
            <blockquote id="q"><p id="qp">c</p></blockquote>
            <p id="mono">m</p><img id="img"/></body></html>"#,
        )
        .expect("parses");
        let mut sheets = Stylesheets::default();
        sheets.add_author(
            "html { font-size: 62.5% } body { font-size: 1.6rem }
             #z { font-size: 0 } #s { font-size: 16px }
             #lh { line-height: 20px } small { font-size: .5em }
             blockquote { margin: 0 100px } #qp { margin-left: 10% }
             #mono { font-family: 'Monotype Baskerville', serif } img { height: 100% }",
        );
        let p = Preferences { line_height: None, ..prefs() };
        let styles = compute(&dom, &sheets, &p);
        let get = |id| style_of(&dom, &styles, id);
        assert_eq!(get("b").font_size, 40.0, "62.5% · 1.6rem = the base size");
        assert!(get("s").line_height.is_finite(), "font-size: 0 parent must not poison children");
        assert_eq!(get("sm").line_height, get("lh").line_height, "length line-height inherits absolute");
        let q = get("q").content_width;
        assert_eq!(get("qp").margin[3], Some(q * 0.1), "percent of the blockquote, not the page");
        assert_eq!(get("mono").font_family.as_ref(), ["Bookerly"], "Monotype is not monospace");
        assert_eq!(get("img").height, None, "percentage height → auto");
    }

    #[test]
    fn hidden_elements_and_line_height_preference() {
        let dom = Dom::parse_xml(
            r#"<html xmlns:epub="http://www.idpf.org/2007/ops"><head id="hd"><title>t</title></head>
            <body><p id="p">x</p><aside id="fn" epub:type="footnote">n</aside></body></html>"#,
        )
        .expect("parses");
        let p = Preferences { line_height: Some(1.5), ..prefs() };
        let styles = compute(&dom, &Stylesheets::default(), &p);
        assert_eq!(style_of(&dom, &styles, "hd").display, Display::None);
        assert_eq!(style_of(&dom, &styles, "fn").display, Display::None, "footnotes leave the flow");
        assert_eq!(style_of(&dom, &styles, "p").line_height, 60.0);
    }
}

#[cfg(test)]
mod real_books {
    use super::*;
    use crate::epub::{resolve, Epub};

    /// Styles every chapter of the local books in `dev-books/` (not in the repo) with their own CSS.
    #[test]
    fn styles_local_books() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev-books");
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let prefs = Preferences {
            font_size: 38.0,
            font_family: Some("Bookerly".into()),
            line_height: Some(1.5),
            align: Some(TextAlign::Justify),
            hyphenate: true,
            page_width: 1056.0,
        };
        for path in
            entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "epub"))
        {
            let mut book = Epub::open(std::fs::File::open(&path).expect("file")).expect("opens");
            let start = std::time::Instant::now();
            let (mut elements, mut rules) = (0, 0);
            for item in book.spine.clone() {
                let dom = book.read_dom(&item.path).expect("chapter");
                let mut sheets = Stylesheets::default();
                for link in dom.find_all(Dom::ROOT, "link") {
                    if let Some(href) = dom.attr(link, "href").filter(|h| h.ends_with(".css")) {
                        if let Ok(css) = book.read(&resolve(&item.path, href)) {
                            sheets.add_author(&String::from_utf8_lossy(&css));
                        }
                    }
                }
                for style in dom.find_all(Dom::ROOT, "style") {
                    sheets.add_author(&dom.text(style));
                }
                rules = sheets.sheets().map(|(_, r)| r.len()).sum();
                elements += compute(&dom, &sheets, &prefs).iter().flatten().count();
            }
            eprintln!(
                "{:?}: {elements} elements styled ({rules} rules/chapter) in {:?}",
                path.file_name().unwrap_or_default(),
                start.elapsed()
            );
        }
    }
}
