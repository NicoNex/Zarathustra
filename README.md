<div align="center">

# Zarathustra

### *Also sprach der Leser.*

**The Über-Reader for e-ink. Man is something that shall be overcome, and so is the stock Kindle UI.**

<img src="docs/screenshots/home.png" width="270" alt="The library"> <img src="docs/screenshots/reading.png" width="270" alt="A page with the dictionary open"> <img src="docs/screenshots/home-night.png" width="270" alt="The library in night mode">

</div>

---

## Prologue

When Zarathustra was thirty years old, he left his home and went into the mountains, and there he read. For ten years he did not tire of it. But one morning he rose with the dawn, stepped before his Kindle, and spoke to it thus:

> *"You great device! What would your screen be, had you not the one who reads upon it? You have carried the same tired menus for years: the carousel of recommendations, the store that pokes its head into every page, the font menu designed by a committee of last men. I am weary of it, as a bee that has gathered too much honey. I must go down."*

So he went down, and wrote Zarathustra in Rust.

Zarathustra runs on a jailbroken Kindle Paperwhite 5, in place of the Amazon interface. One static binary is the whole interface. It has no store, no account, no cloud and no recommendations: you, your books, and a screen that does what you ask.

## Behold, I teach you the Über-Reader

The old readers are *décadents*. They justify text the way the herd walks, word by word, never looking at the paragraph. They hyphenate timidly or not at all. They reflow your page the moment you breathe on a setting. They serve a storefront and call it a home screen.

Zarathustra was built against all of that.

### Typography beyond good and evil

- **Knuth–Plass line breaking**: the whole paragraph is weighed before a single line is set, the way TeX does it. No rivers, no gaping justified lines.
- **Real hyphenation and margin protrusion**: punctuation hangs into the margin so the edge of the text looks straight to the eye, not just to the ruler.
- **Widows and orphans** are not left behind. The pagination looks ahead.
- **Weight you can choose.** How bold should the type be? Four steps, set live, and the page does not reflow.
- **The page stays visible while you set it.** The typography sheet is compact: size, weight, font, spacing, margins, alignment, each a tap away, with the result right above it.

<div align="center">
<img src="docs/screenshots/menu.png" width="300" alt="Typography sheet over the page"> <img src="docs/screenshots/toc.png" width="300" alt="Contents">
</div>

### What does not kill your reading makes it stronger

- **Tap a word** to get its dictionary entry in a card over the page (Wiktionary, StarDict).
- **Footnotes open where you are.** No jump to the end of the book and no hunt for the way back.
- **Highlights and bookmarks** are kept in a SQLite store and gathered in a notebook per book.
- **Search** within a book or across the whole library.
- **The first page of your library quotes you back to yourself**: a highlight from the book you are reading, or your reading figures if you prefer them.

### The eternal return of your library

<div align="center">
<img src="docs/screenshots/bookmenu.png" width="270" alt="The book menu, floating beside the cover"> <img src="docs/screenshots/quick.png" width="270" alt="The quick panel"> <img src="docs/screenshots/wifi.png" width="270" alt="Wi-Fi networks">
</div>

- **Hold a book** and a menu opens beside its cover: open it, see its contents, bookmarks, highlights and details, search inside it, mark it read or unread, or delete it.
- **Swipe down from the top** for the quick panel: brightness, warmth (it can follow the clock), Wi-Fi, sending books, a clean refresh, suspend, page-turn animation, the clock while reading, and night mode.
- **Night mode** turns the page white on black. Covers and pictures keep their own tones.
- The library notices new books by itself, and you go back to it from a book in an instant.

### Send your books across the abyss

<div align="center">
<img src="docs/screenshots/transfer-qr.png" width="300" alt="QR code for wireless transfer">
</div>

Scan the code with your phone, or open the address in any browser on the same network. Choose your books and they fly into `library/`. No cable, no email-to-device, no account.

### Formats

EPUB · KEPUB · AZW3 · MOBI · PDF · TXT

## Thus spoke the engine

Zarathustra is written in Rust, with C only where C is clearly best (SQLite, MuPDF).

```
crates/zarathustra-device   the Kindle (MTK e-ink, touch, light, power, Wi-Fi), an emulator
crates/zarathustra-engine   fonts, DOM, CSS, layout, pagination, EPUB / MOBI / AZW3 / PDF / TXT
crates/zarathustra-store    SQLite: annotations, reading days
crates/zarathustra          the shell: home, reader, panels, keyboard
```

It cross-compiles to one static `armv7-unknown-linux-musleabihf` binary. A KUAL extension stops the Amazon framework while Zarathustra runs and brings it back when you leave.

## Become who you are: install it

You need a **jailbroken Kindle** with KUAL and SSH (KOReader's server on port 2222 works). It has been built for the Paperwhite 5 and the code is written to be portable.

```sh
# the cross toolchain: arm-linux-musleabihf-cross-bin (AUR) for the C dependencies
make install                     # build and install over SSH (KINDLE_HOST=…)
make install KINDLE_HOST=        # or over USB, with the Kindle mounted
```

Put your books in `/mnt/us/library` (by cable, or with the transfer screen), then start Zarathustra from KUAL.

On a desktop, without a Kindle:

```sh
cargo run --release -p zarathustra --bin zarathustra -- book.epub --pgm out/   # render every screen as images
cargo run -p zarathustra --features emulator --bin zarathustra-probe          # the emulator
```

## The last man asks: *"What is a page? What is a font? What is night?"*, and blinks.

The Übermensch reads.

---

<div align="center">

*"One must still have chaos in oneself to be able to give birth to a dancing star."*
<br>Zarathustra has the chaos in its issue tracker. The dancing star is on your screen.

**GPL-3.0-or-later** · not affiliated with Amazon · no book was harmed

</div>
