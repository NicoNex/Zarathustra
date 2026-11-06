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
    pub bookmarks: &'static str,
    pub bookmark: &'static str,
    pub no_bookmarks: &'static str,
    pub no_bookmarks_hint: &'static str,
    /// Reader menu: the scrubber's label at rest.
    pub position: &'static str,
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
    /// Footnote popup title, followed by the note's marker.
    pub note: &'static str,
    /// Definition card (long press on a word).
    pub no_definition: &'static str,
    pub no_dictionaries: &'static str,
    pub sleeping: &'static str,
    pub untitled: &'static str,
    /// Book menu (long press on a book) and the book's details screen.
    pub resume: &'static str,
    pub start_reading: &'static str,
    pub about_book: &'static str,
    pub about: &'static str,
    pub mark_read: &'static str,
    pub mark_unread: &'static str,
    pub delete_book: &'static str,
    pub delete_confirm: &'static str,
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
    /// Library sort orders: title, author, recently read.
    pub sorts: [&'static str; 3],
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
    /// Weekday names, Monday first (the Italian week ends on Sunday).
    pub weekdays: [&'static str; 7],
    pub months: [&'static str; 12],
    /// In-book search: the search key, the screen title, the empty-field hint, status lines;
    /// the keyboard's space bar.
    pub search: &'static str,
    pub search_book: &'static str,
    pub search_hint: &'static str,
    pub search_library: &'static str,
    pub settings: &'static str,
    pub language: &'static str,
    /// The language's own name, shown as the current value.
    pub language_name: &'static str,
    pub full_refresh: &'static str,
    /// `{}` is the number of pages.
    pub every_pages: &'static str,
    pub searching: &'static str,
    pub no_results: &'static str,
    pub space: &'static str,
    /// Wireless transfer screen: title, how to use it, the list and the no-network state.
    pub send_books: &'static str,
    pub transfer_hint: &'static str,
    pub waiting_books: &'static str,
    pub received: &'static str,
    pub receiving: &'static str,
    pub failed: &'static str,
    pub no_wifi: &'static str,
    pub no_wifi_hint: &'static str,
    pub retry: &'static str,
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
    bookmarks: "Segnalibri",
    bookmark: "Segnalibro",
    no_bookmarks: "Ancora nessun segnalibro.",
    no_bookmarks_hint: "Tocca l'angolo in alto a destra di una pagina per aggiungerne uno.",
    position: "Posizione nel libro",
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
    note: "Nota",
    no_definition: "Nessuna definizione trovata.",
    no_dictionaries: "Nessun dizionario installato: copia dizionari StarDict in /mnt/us/dictionaries.",
    sleeping: "In sospensione · premi il tasto di accensione",
    untitled: "Senza titolo",
    resume: "Continua a leggere",
    start_reading: "Inizia a leggere",
    about_book: "Informazioni sul libro",
    about: "Informazioni",
    mark_read: "Segna come letto",
    mark_unread: "Segna come da leggere",
    delete_book: "Elimina dal dispositivo",
    delete_confirm: "Tocca di nuovo per eliminare",
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
    not_read_yet: "Non ancora letto con Zarathustra.",
    sorts: ["Titolo", "Autore", "Recenti"],
    weekday_initials: ["L", "M", "M", "G", "V", "S", "D"],
    tiles: [["Calore", "automatico"], ["Animazione", "pagina"], ["Orologio", "in lettura"], ["Sospendi", ""]],
    on: "Sì",
    off: "No",
    charging: "In carica",
    brightness: "Luminosità",
    warmth: "Calore",
    auto: "AUTO",
    weekdays: ["Lunedì", "Martedì", "Mercoledì", "Giovedì", "Venerdì", "Sabato", "Domenica"],
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
    search: "Cerca",
    search_book: "Cerca nel libro",
    search_hint: "Maiuscole e accenti non contano",
    search_library: "Cerca titolo o autore",
    settings: "Impostazioni",
    language: "Lingua",
    language_name: "Italiano",
    full_refresh: "Pulizia completa dello schermo",
    every_pages: "ogni {} pagine",
    searching: "Ricerca in corso…",
    no_results: "Nessun risultato",
    space: "spazio",
    send_books: "Trasferisci libri",
    transfer_hint:
        "Inquadra il codice con il telefono, o apri l'indirizzo nel browser di un computer sulla stessa rete Wi-Fi.",
    waiting_books: "In attesa dei libri…",
    received: "Ricevuti",
    receiving: "in arrivo…",
    failed: "non riuscito",
    no_wifi: "Wi-Fi non connesso",
    no_wifi_hint: "Collega il Kindle a una rete Wi-Fi (anche l'hotspot del telefono), poi tocca Riprova.",
    retry: "Riprova",
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
    bookmarks: "Bookmarks",
    bookmark: "Bookmark",
    no_bookmarks: "No bookmarks yet.",
    no_bookmarks_hint: "Tap the top-right corner of a page to add one.",
    position: "Position in book",
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
    note: "Note",
    no_definition: "No definition found.",
    no_dictionaries: "No dictionaries installed: copy StarDict dictionaries to /mnt/us/dictionaries.",
    sleeping: "Sleeping · press power to wake",
    untitled: "Untitled",
    resume: "Continue reading",
    start_reading: "Start reading",
    about_book: "About this book",
    about: "About",
    mark_read: "Mark as read",
    mark_unread: "Mark as unread",
    delete_book: "Delete from device",
    delete_confirm: "Tap again to delete",
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
    not_read_yet: "Not read with Zarathustra yet.",
    sorts: ["Title", "Author", "Recent"],
    weekday_initials: ["M", "T", "W", "T", "F", "S", "S"],
    tiles: [["Auto", "warmth"], ["Page", "animation"], ["Clock in", "reader"], ["Sleep", ""]],
    on: "On",
    off: "Off",
    charging: "Charging",
    brightness: "Brightness",
    warmth: "Warmth",
    auto: "AUTO",
    weekdays: ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"],
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
    search: "Search",
    search_book: "Search this book",
    search_hint: "Case and accents don't matter",
    search_library: "Search title or author",
    settings: "Settings",
    language: "Language",
    language_name: "English",
    full_refresh: "Full screen refresh",
    every_pages: "every {} pages",
    searching: "Searching…",
    no_results: "No results",
    space: "space",
    send_books: "Send books",
    transfer_hint: "Scan the code with your phone, or open the address in a computer's browser on the same Wi-Fi.",
    waiting_books: "Waiting for books…",
    received: "Received",
    receiving: "receiving…",
    failed: "failed",
    no_wifi: "Wi-Fi not connected",
    no_wifi_hint: "Connect the Kindle to a Wi-Fi network (your phone's hotspot works too), then tap Retry.",
    retry: "Retry",
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

    /// The pill offering the way back after a jump: "‹  Torna a p. 12".
    pub fn back_to_page(self, page: usize) -> String {
        match self {
            Lang::It => format!("‹  Torna a p. {page}"),
            Lang::En => format!("‹  Back to p. {page}"),
        }
    }

    /// Search status: "1 risultato", "12 risultati"; `capped` when the search stopped there.
    pub fn results(self, n: usize, capped: bool) -> String {
        match (self, n, capped) {
            (Lang::It, n, true) => format!("Primi {n} risultati"),
            (Lang::It, 1, false) => "1 risultato".into(),
            (Lang::It, n, false) => format!("{n} risultati"),
            (Lang::En, n, true) => format!("First {n} results"),
            (Lang::En, 1, false) => "1 result".into(),
            (Lang::En, n, false) => format!("{n} results"),
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
        assert_eq!(Lang::It.results(1, false), "1 risultato");
        assert_eq!(Lang::En.results(200, true), "First 200 results");
    }
}
