//! Interface languages: Russian, English, Polish, Ukrainian ("auto" follows Windows).
//! Strings are compiled in; switching is instant (one atomic).

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Ru = 0,
    En = 1,
    Pl = 2,
    Uk = 3,
}

pub const LANGS: [Lang; 4] = [Lang::Ru, Lang::En, Lang::Pl, Lang::Uk];

impl Lang {
    pub fn code(self) -> &'static str {
        ["ru", "en", "pl", "uk"][self as usize]
    }
    /// Each language's own name, for menus.
    pub fn native_name(self) -> &'static str {
        ["Русский", "English", "Polski", "Українська"][self as usize]
    }
    pub fn from_code(c: &str) -> Option<Lang> {
        LANGS.into_iter().find(|l| l.code().eq_ignore_ascii_case(c))
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::En as u8);

pub fn get() -> Lang {
    LANGS[CURRENT.load(Ordering::Relaxed) as usize % 4]
}

pub fn set(l: Lang) {
    CURRENT.store(l as u8, Ordering::Relaxed);
}

/// "auto" → the system display language (Belarusian and other Cyrillic → Russian,
/// anything unknown → English); otherwise a language code.
pub fn resolve(setting: &str) -> Lang {
    if let Some(l) = Lang::from_code(setting) {
        return l;
    }
    system_lang()
}

#[cfg_attr(windows, allow(dead_code))]
fn from_tag(tag: &str) -> Lang {
    let primary = tag.split(['-', '_', '.', '@']).next().unwrap_or("").to_ascii_lowercase();
    match primary.as_str() {
        "ru" | "be" | "kk" | "ky" => Lang::Ru,
        "pl" => Lang::Pl,
        "uk" => Lang::Uk,
        _ => Lang::En,
    }
}

#[cfg(windows)]
fn system_lang() -> Lang {
    let primary = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3FF;
    match primary {
        0x19 | 0x23 | 0x3F | 0x40 => Lang::Ru, // ru, be, kk, ky
        0x15 => Lang::Pl,
        0x22 => Lang::Uk,
        _ => Lang::En,
    }
}

/// The first language in System Settings > General > Language & Region.
#[cfg(target_os = "macos")]
fn system_lang() -> Lang {
    use std::ffi::c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> *const c_void;
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFStringGetCString(s: *const c_void, buf: *mut u8, len: isize, enc: u32) -> bool;
        fn CFRelease(p: *const c_void);
    }
    const UTF8: u32 = 0x0800_0100;
    unsafe {
        let arr = CFLocaleCopyPreferredLanguages();
        if arr.is_null() {
            return Lang::En;
        }
        let mut buf = [0u8; 64];
        let tag = if CFArrayGetCount(arr) > 0
            && CFStringGetCString(CFArrayGetValueAtIndex(arr, 0), buf.as_mut_ptr(), buf.len() as isize, UTF8)
        {
            let n = buf.iter().position(|&b| b == 0).unwrap_or(0);
            String::from_utf8_lossy(&buf[..n]).into_owned()
        } else {
            String::new()
        };
        CFRelease(arr);
        from_tag(&tag)
    }
}

/// LANGUAGE / LC_ALL / LC_MESSAGES / LANG, as gettext reads them.
#[cfg(all(unix, not(target_os = "macos")))]
fn system_lang() -> Lang {
    for var in ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var)
            && let Some(first) = v.split(':').next().filter(|f| !f.is_empty() && *f != "C" && *f != "POSIX")
        {
            return from_tag(first);
        }
    }
    Lang::En
}

macro_rules! texts {
    ($($key:ident => [$ru:expr, $en:expr, $pl:expr, $uk:expr],)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum T { $($key,)* }

        pub fn tr(key: T, lang: Lang) -> &'static str {
            match key {
                $(T::$key => [$ru, $en, $pl, $uk][lang as usize],)*
            }
        }

        #[cfg(test)]
        pub const ALL: &[T] = &[$(T::$key,)*];
    };
}

/// Text in the current interface language.
pub fn t(key: T) -> &'static str {
    tr(key, get())
}

texts! {
    AppName => ["Диктовка", "Dictation", "Dyktowanie", "Диктування"],
    // tray tooltip / status (prefixed with AppName — )
    StListening => ["слушаю…", "listening…", "słucham…", "слухаю…"],
    StBusy => ["распознаю…", "recognizing…", "rozpoznaję…", "розпізнаю…"],
    StLoading => ["загружаю модель…", "loading the model…", "ładuję model…", "завантажую модель…"],
    StError => ["ошибка", "error", "błąd", "помилка"],
    StIdleHold => ["держи {key} и говори", "hold {key} and speak", "przytrzymaj {key} i mów", "тримай {key} і говори"],
    StIdleToggle => ["нажми {key} и говори", "press {key} and speak", "naciśnij {key} i mów", "натисни {key} і говори"],
    // tray menu
    MenuModel => ["Модель", "Model", "Model", "Модель"],
    MenuLast => ["Последняя", "Last", "Ostatnie", "Остання"],
    MenuSettings => ["Настройки…", "Settings…", "Ustawienia…", "Налаштування…"],
    MenuUiLang => ["Язык интерфейса", "Interface language", "Język interfejsu", "Мова інтерфейсу"],
    MenuSystemLang => ["Как в системе", "System default", "Jak w systemie", "Як у системі"],
    MenuAutostart => ["Запускать с Windows", "Start with Windows", "Uruchamiaj z Windows", "Запускати з Windows"],
    MenuAutostartLogin => ["Запускать при входе", "Start at login", "Uruchamiaj po zalogowaniu", "Запускати під час входу"],
    MenuLog => ["Журнал", "Log", "Dziennik", "Журнал"],
    MenuRestart => ["Перезапустить", "Restart", "Uruchom ponownie", "Перезапустити"],
    MenuQuit => ["Выход", "Quit", "Zakończ", "Вихід"],
    LastStat => ["{s} с речи → {ms} мс, {n} симв.", "{s} s of speech → {ms} ms, {n} chars", "{s} s mowy → {ms} ms, {n} zn.", "{s} с мовлення → {ms} мс, {n} симв."],
    NoSpeech => ["речь не распознана", "no speech recognized", "nie rozpoznano mowy", "мовлення не розпізнано"],
    // message boxes / balloons
    AlreadyRunning => ["Диктовка уже запущена (значок в трее).", "Dictation is already running (tray icon).", "Dyktowanie już działa (ikona w zasobniku).", "Диктування вже запущене (значок у треї)."],
    ConfigError => ["Ошибка в настройках", "Settings error", "Błąd w ustawieniach", "Помилка в налаштуваннях"],
    ModelFailed => ["Модель не загрузилась", "The model failed to load", "Nie udało się wczytać modelu", "Модель не завантажилася"],
    SaveFailed => ["Не удалось сохранить", "Could not save", "Nie udało się zapisać", "Не вдалося зберегти"],
    // settings window
    SettingsTitle => ["Настройки диктовки", "Dictation Settings", "Ustawienia dyktowania", "Налаштування диктування"],
    SecControls => ["Управление", "Controls", "Sterowanie", "Керування"],
    SecRecognition => ["Распознавание", "Recognition", "Rozpoznawanie", "Розпізнавання"],
    SecInterface => ["Интерфейс", "Interface", "Interfejs", "Інтерфейс"],
    Hotkey => ["Клавиша", "Shortcut key", "Klawisz", "Клавіша"],
    Microphone => ["Микрофон", "Microphone", "Mikrofon", "Мікрофон"],
    MicrophoneSub => ["Если он отключён, берётся системный", "If it's unplugged, the system default is used", "Gdy jest odłączony, używany jest domyślny", "Якщо його відключено, береться системний"],
    MicDefault => ["Системный по умолчанию", "System default", "Domyślny systemowy", "Системний за замовчуванням"],
    MicMissing => ["не подключён", "not connected", "niepodłączony", "не підключено"],
    HotkeySub => ["Нажмите, чтобы назначить другую", "Click to assign another key", "Kliknij, aby przypisać inny", "Натисніть, щоб призначити іншу"],
    HotkeyPress => ["Нажмите клавишу…", "Press a key…", "Naciśnij klawisz…", "Натисніть клавішу…"],
    HotkeyEsc => ["Esc — отмена", "Esc to cancel", "Esc — anuluj", "Esc — скасувати"],
    HotkeyBad => ["Эта клавиша печатает текст или открывает меню", "This key types text or opens menus", "Ten klawisz wpisuje tekst lub otwiera menu", "Ця клавіша друкує текст або відкриває меню"],
    Mode => ["Режим", "Mode", "Tryb", "Режим"],
    ModeHold => ["Удерживать", "Hold", "Przytrzymaj", "Утримувати"],
    ModeToggle => ["Переключать", "Toggle", "Przełączaj", "Перемикати"],
    ModeHoldSub => ["Держите клавишу, пока говорите", "Hold the key while you speak", "Trzymaj klawisz, gdy mówisz", "Тримайте клавішу, поки говорите"],
    ModeToggleSub => ["Нажмите, говорите, нажмите снова", "Press, speak, press again", "Naciśnij, mów, naciśnij znowu", "Натисніть, говоріть, натисніть знову"],
    SpeechLang => ["Язык речи", "Speech language", "Język mowy", "Мова мовлення"],
    SpeechAuto => ["Определять сам", "Detect", "Wykrywaj", "Визначати"],
    Prompt => ["Подсказка для модели", "Hint for the model", "Podpowiedź dla modelu", "Підказка для моделі"],
    PromptSub => ["Термины, которые часто говорите: модель напишет их так же", "Terms you often say: the model will spell them the same way", "Terminy, których często używasz: model zapisze je tak samo", "Терміни, які часто кажете: модель напише їх так само"],
    Space => ["Пробел после фразы", "Space after each phrase", "Spacja po każdej frazie", "Пробіл після фрази"],
    Bubble => ["Облачко", "Bubble", "Dymek", "Хмаринка"],
    BubblePos => ["Положение", "Position", "Położenie", "Положення"],
    SecSpeech => ["Речь", "Speech", "Mowa", "Мовлення"],
    BubbleBottom => ["Снизу", "Bottom", "Na dole", "Знизу"],
    BubbleCenter => ["В центре", "Center", "Na środku", "По центру"],
    BubbleOff => ["Выкл.", "Off", "Wył.", "Вимк."],
    Live => ["Текст во время диктовки", "Live text while speaking", "Tekst na żywo", "Текст під час диктування"],
    LiveSub => ["Показывать распознаваемый текст у облачка", "Show recognized text next to the bubble", "Pokazuj rozpoznawany tekst przy dymku", "Показувати розпізнаваний текст біля хмаринки"],
    UiLang => ["Язык интерфейса", "Interface language", "Język interfejsu", "Мова інтерфейсу"],
    Autostart => ["Запускать при входе в Windows", "Start when Windows starts", "Uruchamiaj przy starcie Windows", "Запускати під час входу в Windows"],
    // settings sidebar
    PageGeneral => ["Основные", "General", "Ogólne", "Загальні"],
    PageApp => ["Приложение", "Application", "Aplikacja", "Застосунок"],
    PageHistory => ["История", "History", "Historia", "Історія"],
    SecOutput => ["Вывод текста", "Text output", "Wstawianie tekstu", "Виведення тексту"],
    Clipboard => ["Копировать в буфер обмена", "Copy to clipboard", "Kopiuj do schowka", "Копіювати в буфер обміну"],
    ClipboardSub => ["Текст ещё и копируется, его можно вставить", "Each dictation is also copied, ready to paste", "Tekst jest też kopiowany, gotowy do wklejenia", "Текст ще й копіюється, його можна вставити"],
    HistoryMode => ["Хранить историю", "Keep history", "Przechowuj historię", "Зберігати історію"],
    HistorySession => ["До выхода", "Until exit", "Do zamknięcia", "До виходу"],
    HistoryDisk => ["На диске", "On disk", "Na dysku", "На диску"],
    HistoryOffSub => ["Надиктованный текст нигде не хранится", "Dictated text is not kept anywhere", "Podyktowany tekst nie jest nigdzie przechowywany", "Надиктований текст ніде не зберігається"],
    HistorySessionSub => ["Только в памяти, стирается при выходе", "In memory only, erased when the app quits", "Tylko w pamięci, znika po zamknięciu", "Лише в пам'яті, стирається після виходу"],
    HistoryDiskSub => ["Файл в %LOCALAPPDATA%\\dictate, без шифрования", "A file in %LOCALAPPDATA%\\dictate, not encrypted", "Plik w %LOCALAPPDATA%\\dictate, bez szyfrowania", "Файл у %LOCALAPPDATA%\\dictate, без шифрування"],
    // history page
    HistorySearch => ["Поиск", "Search", "Szukaj", "Пошук"],
    HistoryClear => ["Очистить", "Clear", "Wyczyść", "Очистити"],
    HistoryClearAsk => ["Удалить все диктовки из истории?", "Delete all dictations from the history?", "Usunąć wszystkie dyktowania z historii?", "Видалити всі диктування з історії?"],
    HistoryEmpty => ["Пока нет диктовок", "No dictations yet", "Brak dyktowań", "Поки немає диктувань"],
    HistoryEmptySub => ["Здесь появится всё, что вы надиктуете", "Everything you dictate will appear here", "Tutaj pojawi się wszystko, co podyktujesz", "Тут з'явиться все, що ви надиктуєте"],
    HistoryNoMatch => ["Ничего не найдено", "Nothing found", "Nic nie znaleziono", "Нічого не знайдено"],
    HistoryIsOff => ["История выключена", "History is turned off", "Historia jest wyłączona", "Історію вимкнено"],
    HistoryIsOffSub => ["Её можно включить в разделе «Приложение»", "You can turn it on under Application", "Możesz ją włączyć w sekcji Aplikacja", "Її можна увімкнути в розділі «Застосунок»"],
    Copy => ["Копировать", "Copy", "Kopiuj", "Копіювати"],
    Copied => ["Скопировано", "Copied", "Skopiowano", "Скопійовано"],
    Today => ["Сегодня", "Today", "Dzisiaj", "Сьогодні"],
    Yesterday => ["Вчера", "Yesterday", "Wczoraj", "Учора"],
    MenuHistory => ["История…", "History…", "Historia…", "Історія…"],
    SettingsFile => ["Файл настроек…", "Settings file…", "Plik ustawień…", "Файл налаштувань…"],
    Cancel => ["Отмена", "Cancel", "Anuluj", "Скасувати"],
    Save => ["Сохранить", "Save", "Zapisz", "Зберегти"],
    // key names (Right/Left + key)
    KeyRight => ["Правый", "Right", "Prawy", "Правий"],
    KeyLeft => ["Левый", "Left", "Lewy", "Лівий"],
    KeyMenu => ["Клавиша меню", "Menu key", "Klawisz menu", "Клавіша меню"],
    KeyMouseBack => ["Боковая кнопка мыши (назад)", "Mouse side button (back)", "Boczny przycisk myszy (wstecz)", "Бічна кнопка миші (назад)"],
    KeyMouseFwd => ["Боковая кнопка мыши (вперёд)", "Mouse side button (forward)", "Boczny przycisk myszy (dalej)", "Бічна кнопка миші (вперед)"],
    KeyWheel => ["Колёсико мыши", "Mouse wheel button", "Kółko myszy", "Коліщатко миші"],
}

/// `{name}` placeholders → values.
pub fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut s = template.to_string();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// Localized name of a hotkey.
pub fn key_label(vk: i32) -> String {
    const ALT: &str = if cfg!(target_os = "macos") { "Option" } else { "Alt" };
    const SUPER: &str = if cfg!(target_os = "macos") {
        "Command"
    } else if cfg!(windows) {
        "Win"
    } else {
        "Super"
    };
    let side = |right: bool, key: &str| format!("{} {key}", t(if right { T::KeyRight } else { T::KeyLeft }));
    match vk {
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0xA5 => side(true, ALT),
        0xA4 => side(false, ALT),
        0xA3 => side(true, "Ctrl"),
        0xA2 => side(false, "Ctrl"),
        0xA1 => side(true, "Shift"),
        0xA0 => side(false, "Shift"),
        0x5C => side(true, SUPER),
        0x5B => side(false, SUPER),
        0x5D => t(T::KeyMenu).into(),
        0x13 => "Pause".into(),
        0x91 => "Scroll Lock".into(),
        0x2D => "Insert".into(),
        0x14 => "Caps Lock".into(),
        0x05 => t(T::KeyMouseBack).into(),
        0x06 => t(T::KeyMouseFwd).into(),
        0x04 => t(T::KeyWheel).into(),
        _ => format!("0x{vk:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_tags() {
        assert_eq!(from_tag("ru_RU.UTF-8"), Lang::Ru);
        assert_eq!(from_tag("be-BY"), Lang::Ru);
        assert_eq!(from_tag("pl"), Lang::Pl);
        assert_eq!(from_tag("uk_UA@euro"), Lang::Uk);
        assert_eq!(from_tag("en-GB"), Lang::En);
        assert_eq!(from_tag(""), Lang::En);
    }

    #[test]
    fn every_text_exists_in_every_language() {
        for &k in ALL {
            for l in LANGS {
                assert!(!tr(k, l).is_empty(), "{k:?} empty in {l:?}");
            }
        }
    }

    #[test]
    fn placeholders_survive_translation() {
        for l in LANGS {
            assert!(tr(T::StIdleHold, l).contains("{key}"));
            assert!(tr(T::StIdleToggle, l).contains("{key}"));
            let s = tr(T::LastStat, l);
            assert!(s.contains("{s}") && s.contains("{ms}") && s.contains("{n}"), "{l:?}");
        }
        assert_eq!(fill("a {x} b", &[("x", "1")]), "a 1 b");
    }

    #[test]
    fn codes_roundtrip() {
        for l in LANGS {
            assert_eq!(Lang::from_code(l.code()), Some(l));
            assert_eq!(resolve(l.code()), l);
        }
    }
}
