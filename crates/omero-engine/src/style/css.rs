//! Stylesheet parsing (cssparser) into typed declarations for the properties books use.

use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserState, QualifiedRuleParser,
    RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
};

use super::select::{self, SelectorList};

/// A CSS length before resolution against font size / containing width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    /// Absolute, in CSS px (1/96 in); scaled with the user's text size when resolved.
    Px(f32),
    Em(f32),
    Rem(f32),
    Percent(f32),
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    Block,
    Inline,
    ListItem,
    InlineBlock,
    Table,
    TableRow,
    TableCell,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Start,
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHeight {
    Normal,
    /// Multiple of the font size.
    Factor(f32),
    Length(Length),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteSpace {
    Normal,
    Pre,
    NoWrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlign {
    Baseline,
    Sub,
    Super,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStyle {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextTransform {
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontSize {
    Length(Length),
    /// `small`, `large`… as a factor of the user's base size.
    Keyword(f32),
    Larger,
    Smaller,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Property {
    Display(Display),
    FontFamily(Vec<String>),
    FontSize(FontSize),
    Italic(bool),
    FontWeight(u16),
    SmallCaps(bool),
    LineHeight(LineHeight),
    TextAlign(TextAlign),
    TextIndent(Length),
    Margin(Side, Length),
    Padding(Side, Length),
    BreakBefore(bool),
    BreakAfter(bool),
    BreakInsideAvoid(bool),
    WhiteSpace(WhiteSpace),
    VerticalAlign(VerticalAlign),
    ListStyle(ListStyle),
    TextTransform(TextTransform),
    LetterSpacing(Length),
    Hyphens(bool),
    Width(Length),
    Height(Length),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub property: Property,
    pub important: bool,
}

#[derive(Debug)]
pub struct Rule {
    pub selectors: SelectorList,
    pub declarations: Vec<Declaration>,
}

/// Parses a stylesheet; invalid rules and unsupported properties are skipped, as CSS requires.
pub fn parse_stylesheet(css: &str) -> Vec<Rule> {
    let mut input = Parser::new(css);
    let mut rules = Vec::new();
    let mut sheet = SheetParser;
    for item in StyleSheetParser::new(&mut input, &mut sheet).flatten() {
        rules.extend(item);
    }
    rules
}

/// Parses a `style="…"` attribute.
pub fn parse_declarations(css: &str) -> Vec<Declaration> {
    parse_body(&mut Parser::new(css))
}

struct SheetParser;

impl<'i> QualifiedRuleParser<'i> for SheetParser {
    type Prelude = SelectorList;
    type QualifiedRule = Vec<Rule>;
    type Error = ();

    fn parse_prelude(&mut self, input: &mut Parser<'i>) -> Result<SelectorList, ParseError<()>> {
        select::parse_from(input).ok_or_else(|| ParseError::custom(()))
    }

    fn parse_block(
        &mut self,
        selectors: SelectorList,
        _start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<Vec<Rule>, ParseError<()>> {
        Ok(vec![Rule { selectors, declarations: parse_body(input) }])
    }
}

impl<'i> AtRuleParser<'i> for SheetParser {
    /// Whether the rules inside apply to us.
    type Prelude = bool;
    type AtRule = Vec<Rule>;
    type Error = ();

    fn parse_prelude(&mut self, name: CowRcStr<'i>, input: &mut Parser<'i>) -> Result<bool, ParseError<()>> {
        let start = input.position();
        while input.next().is_ok() {}
        let prelude = input.slice_from(start).to_ascii_lowercase();
        // ponytail: @font-face and @import are ignored (publisher fonts come later);
        // @supports is assumed true unless negated, without evaluating the condition.
        if name.eq_ignore_ascii_case("media") {
            Ok(media_applies(&prelude))
        } else if name.eq_ignore_ascii_case("supports") {
            Ok(!prelude.trim_start().starts_with("not"))
        } else {
            Err(ParseError::custom(()))
        }
    }

    fn parse_block(
        &mut self,
        applies: bool,
        _start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<Vec<Rule>, ParseError<()>> {
        let mut rules = Vec::new();
        if !applies {
            while input.next().is_ok() {} // skip without parsing rules we would discard
            return Ok(rules);
        }
        for item in StyleSheetParser::new(input, &mut SheetParser).flatten() {
            rules.extend(item);
        }
        Ok(rules)
    }
}

/// Media queries (lowercased) for this device: a monochrome screen that reads KF8.
/// Media types match exactly; `(color…)` features are false, other features true.
fn media_applies(query: &str) -> bool {
    query.split(',').any(|q| {
        let (mut negated, mut result, mut seen_type) = (false, true, false);
        let mut rest = q.trim();
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('(') {
                let (feature, tail) = after.split_once(')').unwrap_or((after, ""));
                let name = feature.split(':').next().unwrap_or("").trim();
                result &= !matches!(name, "color" | "min-color" | "color-index" | "min-color-index");
                rest = tail.trim_start();
                continue;
            }
            let end = rest.find(|c: char| c.is_whitespace() || c == '(').unwrap_or(rest.len());
            let (word, tail) = rest.split_at(end);
            match word {
                "not" if !seen_type => negated = true,
                "only" | "and" | "" => {}
                t => {
                    seen_type = true;
                    result &= matches!(t, "all" | "screen" | "amzn-kf8");
                }
            }
            rest = tail.trim_start();
        }
        result != negated
    })
}

fn parse_body(input: &mut Parser) -> Vec<Declaration> {
    let mut decls = Vec::new();
    for item in RuleBodyParser::new(input, &mut DeclParser).flatten() {
        decls.extend(item);
    }
    decls
}

struct DeclParser;

impl<'i> DeclarationParser<'i> for DeclParser {
    type Declaration = Vec<Declaration>;
    type Error = ();

    fn parse_value(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i>,
        _start: &ParserState,
    ) -> Result<Vec<Declaration>, ParseError<()>> {
        // ponytail: CSS-wide keywords (inherit/initial/unset) drop the declaration instead of
        // resolving it; lower-priority rules and normal inheritance then apply.
        let wide = input.try_parse(|i| {
            let k = i.expect_ident_cloned().map_err(|_| ())?;
            i.expect_exhausted().map_err(|_| ())?;
            // Only consume the value when it really is a CSS-wide keyword.
            ["inherit", "initial", "unset", "revert"].iter().any(|w| k.eq_ignore_ascii_case(w)).then_some(()).ok_or(())
        });
        if wide.is_ok() {
            return Ok(Vec::new());
        }
        let props = property(&name.to_ascii_lowercase(), input).ok_or_else(|| ParseError::custom(()))?;
        let important = input.try_parse(cssparser::parse_important).is_ok();
        if !input.is_exhausted() {
            return Err(ParseError::custom(()));
        }
        Ok(props.into_iter().map(|property| Declaration { property, important }).collect())
    }
}

impl<'i> AtRuleParser<'i> for DeclParser {
    type Prelude = ();
    type AtRule = Vec<Declaration>;
    type Error = ();
}

impl<'i> QualifiedRuleParser<'i> for DeclParser {
    type Prelude = ();
    type QualifiedRule = Vec<Declaration>;
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, Vec<Declaration>, ()> for DeclParser {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        false
    }
}

fn ident(input: &mut Parser) -> Option<String> {
    input.try_parse(|i| i.expect_ident_cloned().map(|s| s.to_ascii_lowercase())).ok()
}

fn length(input: &mut Parser) -> Option<Length> {
    input
        .try_parse(|i| -> Result<Length, ParseError<()>> {
            let token = i.next()?.clone();
            let len = match token {
                Token::Number { value: 0.0, .. } => Length::Px(0.0),
                Token::Percentage { unit_value, .. } => Length::Percent(unit_value * 100.0),
                Token::Dimension { value, ref unit, .. } => match unit.to_ascii_lowercase().as_str() {
                    "px" => Length::Px(value),
                    "pt" => Length::Px(value * 96.0 / 72.0),
                    "pc" => Length::Px(value * 16.0),
                    "in" => Length::Px(value * 96.0),
                    "cm" => Length::Px(value * 96.0 / 2.54),
                    "mm" => Length::Px(value * 96.0 / 25.4),
                    "em" => Length::Em(value),
                    "rem" => Length::Rem(value),
                    "ex" | "ch" => Length::Em(value * 0.5),
                    _ => return Err(ParseError::custom(())),
                },
                Token::Ident(ref s) if s.eq_ignore_ascii_case("auto") => Length::Auto,
                _ => return Err(ParseError::custom(())),
            };
            Ok(len)
        })
        .ok()
}

fn number(input: &mut Parser) -> Option<f32> {
    input.try_parse(|i| i.expect_number()).ok()
}

/// 1–4 values → top, right, bottom, left.
fn box_sides(input: &mut Parser, make: fn(Side, Length) -> Property) -> Option<Vec<Property>> {
    let mut v = Vec::new();
    while let Some(l) = length(input) {
        v.push(l);
    }
    let [t, r, b, l] = match v[..] {
        [a] => [a, a, a, a],
        [a, b] => [a, b, a, b],
        [a, b, c] => [a, b, c, b],
        [a, b, c, d] => [a, b, c, d],
        _ => return None,
    };
    Some(vec![make(Side::Top, t), make(Side::Right, r), make(Side::Bottom, b), make(Side::Left, l)])
}

fn font_size(input: &mut Parser) -> Option<FontSize> {
    if let Some(l) = length(input) {
        return Some(FontSize::Length(l));
    }
    Some(match ident(input)?.as_str() {
        "xx-small" => FontSize::Keyword(0.6),
        "x-small" => FontSize::Keyword(0.75),
        "small" => FontSize::Keyword(0.89),
        "medium" => FontSize::Keyword(1.0),
        "large" => FontSize::Keyword(1.2),
        "x-large" => FontSize::Keyword(1.5),
        "xx-large" => FontSize::Keyword(2.0),
        "larger" => FontSize::Larger,
        "smaller" => FontSize::Smaller,
        _ => return None,
    })
}

fn font_weight(input: &mut Parser) -> Option<u16> {
    if let Some(n) = number(input) {
        return Some(n.clamp(1.0, 1000.0) as u16);
    }
    Some(match ident(input)?.as_str() {
        "normal" => 400,
        "bold" | "bolder" => 700,
        "lighter" => 300,
        _ => return None,
    })
}

fn font_family(input: &mut Parser) -> Option<Vec<String>> {
    let mut families = Vec::new();
    loop {
        let name = input.try_parse(|i| i.expect_string_cloned().map(|s| s.to_string())).ok().or_else(|| {
            let mut words = Vec::new();
            while let Ok(w) = input.try_parse(|i| i.expect_ident_cloned()) {
                words.push(w.to_string());
            }
            (!words.is_empty()).then(|| words.join(" "))
        })?;
        families.push(name);
        if input.try_parse(|i| i.expect_comma()).is_err() {
            return Some(families);
        }
    }
}

fn line_height(input: &mut Parser) -> Option<LineHeight> {
    if let Some(n) = number(input) {
        return Some(LineHeight::Factor(n));
    }
    if let Some(l) = length(input) {
        return Some(LineHeight::Length(l));
    }
    (ident(input)? == "normal").then_some(LineHeight::Normal)
}

fn page_break(input: &mut Parser) -> Option<bool> {
    Some(matches!(ident(input)?.as_str(), "always" | "page" | "left" | "right" | "recto" | "verso"))
}

/// One CSS property (or shorthand) into typed properties; `None` = unsupported or invalid.
fn property(name: &str, input: &mut Parser) -> Option<Vec<Property>> {
    use Property as P;
    let one = |p: Property| Some(vec![p]);
    match name {
        "display" => one(P::Display(match ident(input)?.as_str() {
            "block" | "flow-root" | "flex" | "grid" => Display::Block,
            "inline" => Display::Inline,
            "list-item" => Display::ListItem,
            "inline-block" | "inline-flex" => Display::InlineBlock,
            "table" | "inline-table" => Display::Table,
            // ponytail: row groups and captions flow as blocks; columns carry no content.
            "table-row-group" | "table-header-group" | "table-footer-group" | "table-caption" => Display::Block,
            "table-column" | "table-column-group" => Display::None,
            "table-row" => Display::TableRow,
            "table-cell" => Display::TableCell,
            "none" => Display::None,
            _ => return None,
        })),
        "font-family" => one(P::FontFamily(font_family(input)?)),
        "font-size" => one(P::FontSize(font_size(input)?)),
        "font-style" => one(P::Italic(matches!(ident(input)?.as_str(), "italic" | "oblique"))),
        "font-weight" => one(P::FontWeight(font_weight(input)?)),
        "font-variant" | "font-variant-caps" => one(P::SmallCaps(ident(input)?.contains("small-caps"))),
        "font" => {
            // [style] [variant] [weight] size[/line-height] family
            let mut out = vec![P::Italic(false), P::SmallCaps(false), P::FontWeight(400)];
            loop {
                // Size keywords end the prefix: only consume idents that are style/variant/weight.
                let prefix = input.try_parse(|i| match ident(i).as_deref() {
                    Some("italic" | "oblique") => Ok(P::Italic(true)),
                    Some("small-caps") => Ok(P::SmallCaps(true)),
                    Some("normal") => Ok(P::Italic(false)),
                    _ => Err(()),
                });
                match prefix {
                    Ok(p @ P::SmallCaps(_)) => out[1] = p,
                    Ok(p) => out[0] = p,
                    Err(()) => match input.try_parse(|i| font_weight(i).ok_or(())) {
                        Ok(w) => out[2] = P::FontWeight(w),
                        Err(()) => break,
                    },
                }
            }
            out.push(P::FontSize(font_size(input)?));
            // The shorthand resets line-height when it doesn't set it.
            let lh =
                if input.try_parse(|i| i.expect_delim('/')).is_ok() { line_height(input)? } else { LineHeight::Normal };
            out.push(P::LineHeight(lh));
            out.push(P::FontFamily(font_family(input)?));
            Some(out)
        }
        "line-height" => one(P::LineHeight(line_height(input)?)),
        _ if name.starts_with("margin-") || name.starts_with("padding-") => {
            let (kind, suffix) = name.split_once('-')?;
            let side = match suffix {
                "top" => Side::Top,
                "right" => Side::Right,
                "bottom" => Side::Bottom,
                "left" => Side::Left,
                _ => return None,
            };
            let len = length(input)?;
            one(if kind == "margin" { P::Margin(side, len) } else { P::Padding(side, len) })
        }
        "text-align" => one(P::TextAlign(match ident(input)?.as_str() {
            "left" => TextAlign::Left,
            "right" | "end" => TextAlign::Right,
            "center" => TextAlign::Center,
            "justify" => TextAlign::Justify,
            "start" => TextAlign::Start,
            _ => return None,
        })),
        "text-indent" => one(P::TextIndent(length(input)?)),
        "margin" => box_sides(input, P::Margin),
        "padding" => box_sides(input, P::Padding),
        "page-break-before" | "break-before" => one(P::BreakBefore(page_break(input)?)),
        "page-break-after" | "break-after" => one(P::BreakAfter(page_break(input)?)),
        "page-break-inside" | "break-inside" => one(P::BreakInsideAvoid(ident(input)?.starts_with("avoid"))),
        "white-space" => one(P::WhiteSpace(match ident(input)?.as_str() {
            "pre" | "pre-wrap" | "pre-line" | "break-spaces" => WhiteSpace::Pre,
            "nowrap" => WhiteSpace::NoWrap,
            _ => WhiteSpace::Normal,
        })),
        "vertical-align" => one(P::VerticalAlign(match ident(input).as_deref() {
            Some("sub") => VerticalAlign::Sub,
            Some("super") => VerticalAlign::Super,
            _ => VerticalAlign::Baseline,
        })),
        "list-style" => {
            // Any order; the first ident that names a marker type wins (position/image ignored).
            let mut found = None;
            while let Some(w) = ident(input) {
                found = found.or(list_style(&w));
            }
            input.try_parse(|i| i.next().map(|_| ()).map_err(|_| ())).ok();
            one(P::ListStyle(found?))
        }
        "list-style-type" => one(P::ListStyle(list_style(&ident(input)?)?)),
        "text-transform" => one(P::TextTransform(match ident(input)?.as_str() {
            "uppercase" => TextTransform::Uppercase,
            "lowercase" => TextTransform::Lowercase,
            "capitalize" => TextTransform::Capitalize,
            _ => TextTransform::None,
        })),
        "letter-spacing" => {
            one(P::LetterSpacing(length(input).or_else(|| (ident(input)? == "normal").then_some(Length::Px(0.0)))?))
        }
        "hyphens" | "-webkit-hyphens" | "adobe-hyphenate" => {
            one(P::Hyphens(!matches!(ident(input)?.as_str(), "none" | "manual")))
        }
        "width" => one(P::Width(length(input)?)),
        "height" => one(P::Height(length(input)?)),
        _ => None,
    }
}

fn list_style(name: &str) -> Option<ListStyle> {
    Some(match name {
        "none" => ListStyle::None,
        "disc" => ListStyle::Disc,
        "circle" => ListStyle::Circle,
        "square" => ListStyle::Square,
        "decimal" | "decimal-leading-zero" => ListStyle::Decimal,
        "lower-alpha" | "lower-latin" => ListStyle::LowerAlpha,
        "upper-alpha" | "upper-latin" => ListStyle::UpperAlpha,
        "lower-roman" => ListStyle::LowerRoman,
        "upper-roman" => ListStyle::UpperRoman,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rules_shorthands_and_importance() {
        let rules = parse_stylesheet(
            r#"
            @charset "utf-8";
            p, li { margin: 0 1em; text-indent: 1.5em; font: italic bold 12pt/1.4 "Palatino Linotype", serif }
            h1 { text-align: center !important; page-break-before: always; color: red }
            .bad { margin: ; } .ok { font-size: smaller }
            @media print { p { display: none } }
            @media amzn-kf8, screen { blockquote { margin-left: 2em } }
            @font-face { font-family: X; src: url(x.ttf) }
            "#,
        );
        assert_eq!(rules.len(), 5, "{rules:#?}");
        let p = &rules[0].declarations;
        assert!(p.contains(&Declaration { property: Property::Margin(Side::Right, Length::Em(1.0)), important: false }));
        assert!(p.contains(&Declaration { property: Property::TextIndent(Length::Em(1.5)), important: false }));
        assert!(p.iter().any(|d| d.property == Property::Italic(true)));
        assert!(p.iter().any(|d| d.property == Property::FontWeight(700)));
        assert!(p.iter().any(|d| d.property == Property::FontSize(FontSize::Length(Length::Px(16.0)))));
        assert!(p.iter().any(|d| d.property == Property::LineHeight(LineHeight::Factor(1.4))));
        assert!(p.iter().any(|d| d.property == Property::FontFamily(vec!["Palatino Linotype".into(), "serif".into()])));
        let h1 = &rules[1].declarations;
        assert_eq!(h1.len(), 2, "unsupported `color` dropped");
        assert!(h1[0].important);
        assert!(rules[2].declarations.is_empty(), "invalid declaration skipped, rule kept");
        assert_eq!(rules[4].declarations[0].property, Property::Margin(Side::Left, Length::Em(2.0)));
    }

    #[test]
    fn media_queries_for_a_monochrome_kf8_screen() {
        for (q, expected) in [
            ("", true),
            ("all", true),
            ("screen", true),
            ("amzn-kf8", true),
            ("print", false),
            ("amzn-mobi", false),
            ("not amzn-mobi", true),
            ("not print", true),
            ("not all and (color)", true),
            ("screen and (monochrome)", true),
            ("only screen and (min-width: 600px)", true),
            ("(color)", false),
            ("print, amzn-kf8", true),
        ] {
            assert_eq!(media_applies(q), expected, "{q:?}");
        }
    }

    #[test]
    fn font_shorthand_keywords_and_css_wide_values() {
        let d = parse_declarations("font: bold x-large Georgia, serif; font-style: inherit; list-style: square inside");
        assert!(d.contains(&Declaration { property: Property::FontWeight(700), important: false }));
        assert!(d.iter().any(|d| d.property == Property::FontSize(FontSize::Keyword(1.5))));
        assert!(d.iter().any(|d| d.property == Property::LineHeight(LineHeight::Normal)), "shorthand resets it");
        assert!(!d.iter().any(|d| d.property == Property::Italic(false) && d.important), "inherit dropped");
        assert!(d.iter().any(|d| d.property == Property::ListStyle(ListStyle::Square)));
    }

    #[test]
    fn parses_style_attribute() {
        let d = parse_declarations("font-style: italic; margin-top: 10%; bogus: 1");
        assert_eq!(d.len(), 2);
        assert_eq!(d[1].property, Property::Margin(Side::Top, Length::Percent(10.0)));
    }
}
