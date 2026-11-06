# EPUB engine (sub-project 2, phases 2–3) Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Steps use `- [ ]`.

**Goal:** turn an EPUB into paginated display lists (positioned glyphs + images, each carrying its source position), typeset to book quality, fast enough that the reader never waits.

**Architecture:** a pipeline of small modules in `zarathustra-engine`, each pure and unit-tested:
`epub` (container) → `dom` (arena tree) → `style` (cascade) → `boxes` (block/inline tree) → `inline` (shaping + Knuth–Plass) → `paginate` → `display` (paint). Plato (reference only, never copied) fuses style, layout and pagination into one recursive function; separating them is what makes the improvements below possible.

**Where we beat Plato:**

| Plato | Zarathustra |
|---|---|
| Hand-written XML parser, fails on malformed content | quick-xml for XHTML (fast); html5ever fallback when XML parsing fails |
| Regex-assisted CSS subset, own matcher | `cssparser` + Servo `selectors` (full selector grammar, correct specificity), spec inheritance, `em`/`rem`/`%`/`pt` units |
| Integer pixel advances | Fractional advances (f32) end to end, quarter-pixel glyph placement |
| Knuth–Plass only | Knuth–Plass + Liang hyphenation (`hyphenation` crate, en/it embedded) + **margin protrusion** (hanging punctuation, à la microtype) + looseness fallback for pathological paragraphs |
| Page-local positions | Stable `(spine, node path, char offset)` positions, EPUB CFI serialization: the basis for highlights that survive re-layout |
| Synchronous full re-layout on every setting change | Chapter-at-a-time layout on a worker thread; open shows the saved page after laying out just its chapter; page count fills in in the background |
| Publisher CSS fights user settings | Readium-CSS-style policy: user font, size, line height, margins, justification override publisher values; publisher keeps structure (indents, italics, alignment of centered/right blocks) |

---

### Phase 2a — `epub`: container, metadata, spine, TOC, cover
- `zip` (deflate via miniz_oxide, pure Rust). `META-INF/container.xml` → OPF.
- OPF: title, creators, language, identifiers, series (`calibre:series` meta and EPUB3 `belongs-to-collection`), manifest, spine (linear flag), cover (EPUB3 `cover-image` property, EPUB2 `meta name=cover`, then a heuristic on the guide).
- TOC: EPUB3 nav (`epub:type="toc"`), else NCX. Hrefs resolved and percent-decoded relative to the OPF.
- Tests: built-in tiny EPUBs made with `zip` in tests (EPUB2 + EPUB3); local smoke test against the user's real books when present (not committed).

### Phase 2b — `dom`: arena tree with source offsets
- `Vec<Node>` arena, `NodeId(u32)`. Elements store an interned local name, attributes (`id`, `class` and the rest). Text nodes store text + byte offset in the chapter file.
- Parser: quick-xml (entities decoded, namespaces stripped to local names); on error, html5ever builds the same tree.
- Tests: XHTML with self-closing tags, entities, malformed fallback.

### Phase 2c — `style`: CSS cascade
- User-agent sheet (book-tuned defaults: paragraphs, headings, lists, blockquote, hr, img) + publisher sheets (`<link>` + `<style>`) + user overrides.
- Properties supported: display, font-family/size/style/weight/variant(small-caps), line-height, text-align, text-indent, margins/padding (block axis + inline), page-break-before/after/inside, white-space, vertical-align (sub/sup), list-style-type, letter-spacing, word-spacing, text-transform, hyphens, width/height on images.
- Tests: specificity order, inheritance, units, user override policy.

### Phase 3a — `inline`: paragraph builder
- Runs → shaped words (one harfrust call per style run, not per word); break opportunities from `icu_segmenter` line breaker; hyphenation points from Liang patterns (language from `xml:lang`/OPF).
- Knuth–Plass (own implementation, active-node list, fitness classes, flagged penalties for consecutive hyphens), protrusion via per-glyph left/right protrusion factors, fallback to first-fit above N items.
- Output: lines of glyph runs with fractional x, each glyph with source `(node, offset)`.
- Tests: known-good breaks for a fixture paragraph, no overfull lines, hyphenation applied, protrusion shifts punctuation.

### Phase 3b — `paginate` + `display`
- Blocks and lines flow into pages: margins collapse, widows/orphans (2/2), headings kept with the next block, `page-break-*`, images scaled to fit.
- Page = display list (glyph runs, images, rules) + source range. `paint(page, canvas)` uses the glyph cache.
- Tests: golden PNGs for fixture chapters; position → page lookup round-trip.

### Phase 3c — on device
- `zarathustra-epubprobe BOOK.epub [page]`: opens, paginates the chapter, shows a page, taps turn pages with the swipe animation. Measured: open time, per-chapter layout time, page turn latency.
