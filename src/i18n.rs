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

/// "auto" → the Windows display language (Belarusian and other Cyrillic → Russian,
/// anything unknown → English); otherwise a language code.
pub fn resolve(setting: &str) -> Lang {
    if let Some(l) = Lang::from_code(setting) {
        return l;
    }
    let primary = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3FF;
    match primary {
        0x19 | 0x23 | 0x3F | 0x40 => Lang::Ru, // ru, be, kk, ky
        0x15 => Lang::Pl,
        0x22 => Lang::Uk,
        _ => Lang::En,
    }
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
    BubbleBottom => ["Снизу", "Bottom", "Na dole", "Знизу"],
    BubbleCenter => ["В центре", "Center", "Na środku", "По центру"],
    BubbleOff => ["Выкл.", "Off", "Wył.", "Вимк."],
    Live => ["Текст во время диктовки", "Live text while speaking", "Tekst na żywo", "Текст під час диктування"],
    LiveSub => ["Показывать распознаваемый текст у облачка", "Show recognized text next to the bubble", "Pokazuj rozpoznawany tekst przy dymku", "Показувати розпізнаваний текст біля хмаринки"],
    UiLang => ["Язык интерфейса", "Interface language", "Język interfejsu", "Мова інтерфейсу"],
    Autostart => ["Запускать при входе в Windows", "Start when Windows starts", "Uruchamiaj przy starcie Windows", "Запускати під час входу в Windows"],
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
    let side = |right: bool, key: &str| format!("{} {key}", t(if right { T::KeyRight } else { T::KeyLeft }));
    match vk {
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0xA5 => side(true, "Alt"),
        0xA4 => side(false, "Alt"),
        0xA3 => side(true, "Ctrl"),
        0xA2 => side(false, "Ctrl"),
        0xA1 => side(true, "Shift"),
        0xA0 => side(false, "Shift"),
        0x5C => side(true, "Win"),
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
