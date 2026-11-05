//! The shell's user-visible strings, in Italian (the default) and English.
//!
//! One `Strings` table per language: a missing translation is a compile error, and drawing
//! code reads `&'static str`s with no lookup or allocation.
// ponytail: a compiled-in table for two languages whose plurals are both one/other. Move to
// Fluent (`fluent-bundle`, .ftl files) when a third language, a language with other plural
// rules, or translations edited without a rebuild show up.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    It,
    En,
}

impl Lang {
    /// The `settings.txt` code.
    pub fn code(self) -> &'static str {
        match self {
            Lang::It => "it",
            Lang::En => "en",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        match code {
            "it" => Some(Lang::It),
            "en" => Some(Lang::En),
            _ => None,
        }
    }
}

pub struct Strings {
    pub library: &'static str,
    pub exit: &'static str,
    pub continue_reading: &'static str,
    /// Home filter chips: all, unread, reading, finished.
    pub filters: [&'static str; 4],
    /// List status and grid badge of a finished book.
    pub finished: &'static str,
    pub finished_badge: &'static str,
    pub no_books: &'static str,
    pub back_library: &'static str,
    pub back: &'static str,
    pub previous: &'static str,
    pub next: &'static str,
    pub highlights: &'static str,
    pub no_highlights: &'static str,
    pub no_highlights_hint: &'static str,
    pub contents: &'static str,
    pub no_contents: &'static str,
    /// Typography menu: section headers, row labels, choices.
    pub text: &'static str,
    pub layout: &'static str,
    pub size: &'static str,
    pub font: &'static str,
    pub spacing: &'static str,
    pub margins: &'static str,
    pub alignment: &'static str,
    pub original_font: &'static str,
    pub spacings: [&'static str; 3],
    pub margin_names: [&'static str; 3],
    pub aligns: [&'static str; 3],
    /// Selection popup.
    pub highlight: &'static str,
    pub delete: &'static str,
    pub cancel: &'static str,
    pub sleeping: &'static str,
    pub untitled: &'static str,
    /// Book menu (long press on a book) and the book's details screen.
    pub resume: &'static str,
    pub start_reading: &'static str,
    pub about_book: &'static str,
    pub about: &'static str,
    pub mark_read: &'static str,
    pub mark_unread: &'static str,
    pub description: &'static str,
    pub no_description: &'static str,
    pub your_reading: &'static str,
    /// Reading statistics: screen title, figure labels, chart caption.
    pub stats: &'static str,
    pub time_read: &'static str,
    pub pages_read: &'static str,
    pub days_read: &'static str,
    pub last_read: &'static str,
    pub today: &'static str,
    pub this_week: &'static str,
    pub streak: &'static str,
    pub books_finished: &'static str,
    pub last_days: &'static str,
    pub not_read_yet: &'static str,
    /// Weekday initials, Monday first.
    pub weekday_initials: [&'static str; 7],
    /// Quick panel tiles, two lines each: auto warmth, page animation, clock, sleep.
    pub tiles: [[&'static str; 2]; 4],
    pub on: &'static str,
    pub off: &'static str,
    pub charging: &'static str,
    pub brightness: &'static str,
    pub warmth: &'static str,
    pub auto: &'static str,
    /// Sunday first.
    pub weekdays: [&'static str; 7],
    pub months: [&'static str; 12],
}

const IT: Strings = Strings {
    library: "Libreria",
    exit: "Esci",
    continue_reading: "CONTINUA A LEGGERE",
    filters: ["Tutti", "Da leggere", "In lettura", "Letti"],
    finished: "Letto",
    finished_badge: "LETTO",
    no_books: "Nessun libro qui",
    back_library: "‹  Libreria",
    back: "‹  Indietro",
    previous: "‹  Precedente",
    next: "Successiva  ›",
    highlights: "Evidenziazioni",
    no_highlights: "Ancora nessuna evidenziazione.",
    no_highlights_hint: "Tieni premuta una parola durante la lettura per iniziare.",
    contents: "Indice",
    no_contents: "Nessun indice",
    text: "Testo",
    layout: "Impaginazione",
    size: "Dimensione",
    font: "Carattere",
    spacing: "Interlinea",
    margins: "Margini",
    alignment: "Allineamento",
    original_font: "Originale",
    spacings: ["Stretta", "Normale", "Ampia"],
    margin_names: ["Stretti", "Normali", "Ampi"],
    aligns: ["Giustificato", "Sinistra", "Centrato"],
    highlight: "Evidenzia",
    delete: "Elimina",
    cancel: "Annulla",
    sleeping: "In sospensione · premi il tasto di accensione",
    untitled: "Senza titolo",
    resume: "Continua a leggere",
    start_reading: "Inizia a leggere",
    about_book: "Informazioni sul libro",
    about: "Informazioni",
    mark_read: "Segna come letto",
    mark_unread: "Segna come da leggere",
    description: "DESCRIZIONE",
    no_description: "Nessuna descrizione nel libro.",
    your_reading: "LA TUA LETTURA",
    stats: "Statistiche",
    time_read: "tempo di lettura",
    pages_read: "pagine lette",
    days_read: "giorni di lettura",
    last_read: "Ultima lettura",
    today: "oggi",
    this_week: "ultimi 7 giorni",
    streak: "giorni di fila",
    books_finished: "libri finiti",
    last_days: "ULTIMI 14 GIORNI",
    not_read_yet: "Non ancora letto con Omero.",
    weekday_initials: ["L", "M", "M", "G", "V", "S", "D"],
    tiles: [["Calore", "automatico"], ["Animazione", "pagina"], ["Orologio", "in lettura"], ["Sospendi", ""]],
    on: "Sì",
    off: "No",
    charging: "In carica",
    brightness: "Luminosità",
    warmth: "Calore",
    auto: "AUTO",
    weekdays: ["Domenica", "Lunedì", "Martedì", "Mercoledì", "Giovedì", "Venerdì", "Sabato"],
    months: [
        "gennaio",
        "febbraio",
        "marzo",
        "aprile",
        "maggio",
        "giugno",
        "luglio",
        "agosto",
        "settembre",
        "ottobre",
        "novembre",
        "dicembre",
    ],
};

const EN: Strings = Strings {
    library: "Library",
    exit: "Exit",
    continue_reading: "CONTINUE READING",
    filters: ["All", "Unread", "Reading", "Read"],
    finished: "Read",
    finished_badge: "READ",
    no_books: "No books here",
    back_library: "‹  Library",
    back: "‹  Back",
    previous: "‹  Previous",
    next: "Next  ›",
    highlights: "Highlights",
    no_highlights: "No highlights yet.",
    no_highlights_hint: "Press and hold a word while reading to start one.",
    contents: "Contents",
    no_contents: "No table of contents",
    text: "Text",
    layout: "Layout",
    size: "Size",
    font: "Font",
    spacing: "Spacing",
    margins: "Margins",
    alignment: "Alignment",
    original_font: "Original",
    spacings: ["Tight", "Normal", "Loose"],
    margin_names: ["Narrow", "Normal", "Wide"],
    aligns: ["Justified", "Left", "Center"],
    highlight: "Highlight",
    delete: "Delete",
    cancel: "Cancel",
    sleeping: "Sleeping · press power to wake",
    untitled: "Untitled",
    resume: "Continue reading",
    start_reading: "Start reading",
    about_book: "About this book",
    about: "About",
    mark_read: "Mark as read",
    mark_unread: "Mark as unread",
    description: "DESCRIPTION",
    no_description: "The book has no description.",
    your_reading: "YOUR READING",
    stats: "Statistics",
    time_read: "time read",
    pages_read: "pages read",
    days_read: "days read",
    last_read: "Last read",
    today: "today",
    this_week: "last 7 days",
    streak: "day streak",
    books_finished: "books finished",
    last_days: "LAST 14 DAYS",
    not_read_yet: "Not read with Omero yet.",
    weekday_initials: ["M", "T", "W", "T", "F", "S", "S"],
    tiles: [["Auto", "warmth"], ["Page", "animation"], ["Clock in", "reader"], ["Sleep", ""]],
    on: "On",
    off: "Off",
    charging: "Charging",
    brightness: "Brightness",
    warmth: "Warmth",
    auto: "AUTO",
    weekdays: ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"],
    months: [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
};

/// The UI language for the whole process; set once from the settings at startup.
static LANG: AtomicU8 = AtomicU8::new(Lang::It as u8);

pub fn set_lang(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if LANG.load(Ordering::Relaxed) == Lang::En as u8 {
        Lang::En
    } else {
        Lang::It
    }
}

/// The strings of the current language.
pub fn t() -> &'static Strings {
    strings(lang())
}

pub fn strings(lang: Lang) -> &'static Strings {
    match lang {
        Lang::It => &IT,
        Lang::En => &EN,
    }
}

impl Lang {
    /// "1 evidenziazione", "3 evidenziazioni".
    pub fn highlights(self, n: usize) -> String {
        match (self, n) {
            (Lang::It, 1) => "1 evidenziazione".into(),
            (Lang::It, n) => format!("{n} evidenziazioni"),
            (Lang::En, 1) => "1 highlight".into(),
            (Lang::En, n) => format!("{n} highlights"),
        }
    }

    /// Reader footer: minutes left in the chapter.
    pub fn minutes_left(self, n: u32) -> String {
        match (self, n) {
            (Lang::It, 1) => "1 min rimanente nel capitolo".into(),
            (Lang::It, n) => format!("{n} min rimanenti nel capitolo"),
            (Lang::En, n) => format!("{n} min left in chapter"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_plurals_agree() {
        for l in [Lang::It, Lang::En] {
            assert_eq!(Lang::from_code(l.code()), Some(l));
        }
        assert_eq!(Lang::from_code("fr"), None);
        assert_eq!(Lang::It.highlights(1), "1 evidenziazione");
        assert_eq!(Lang::It.highlights(0), "0 evidenziazioni");
        assert_eq!(Lang::En.highlights(3), "3 highlights");
        assert_eq!(Lang::It.minutes_left(1), "1 min rimanente nel capitolo");
        assert_eq!(Lang::It.minutes_left(5), "5 min rimanenti nel capitolo");
    }
}
