//! Translations of the user interface.
//!
//! Every text shown to the user goes through `tr("English text")` (or `trf` with `{placeholders}`).
//! A language is a plain UTF-8 text file, one translation per line:
//!
//! ```text
//! # comment
//! @code = pl
//! @name = Polski
//! @decimal = ,
//! Refresh devices = Odśwież urządzenia
//! {name}: battery at {level}% = {name}: bateria {level}%
//! ```
//!
//! The left side is the English text exactly as in the program (`\n` stands for a line break), the
//! right side its translation. Placeholders in braces are filled in by the program and stay as
//! they are. A missing or empty translation falls back to English. `@decimal` is the language's
//! decimal separator, `.` (3.9, the default) or `,` (3,9), used by `decimal()`.
//!
//! The languages in `lang/` are built into the exe. Users can add their own (or fix a built-in
//! one): any `*.txt` in `%LOCALAPPDATA%\BatteryHub\Languages` is listed in the Language menu, and
//! one with the same `@code` as a built-in language replaces that language's texts it contains.
//! `lang/en.txt` lists every text of the program and is the template for a new language.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Marks an English text for translation where it is kept in a table and translated later with
/// `tr(text)` (the test that checks `lang/en.txt` finds these too).
pub const fn n_(english: &'static str) -> &'static str {
    english
}

/// The built-in languages (code, file).
pub const BUILT_IN: &[(&str, &str)] = &[
    ("en", include_str!("../lang/en.txt")),
    ("cs", include_str!("../lang/cs.txt")),
    ("de", include_str!("../lang/de.txt")),
    ("es", include_str!("../lang/es.txt")),
    ("fr", include_str!("../lang/fr.txt")),
    ("hu", include_str!("../lang/hu.txt")),
    ("id", include_str!("../lang/id.txt")),
    ("it", include_str!("../lang/it.txt")),
    ("ja", include_str!("../lang/ja.txt")),
    ("ko", include_str!("../lang/ko.txt")),
    ("nl", include_str!("../lang/nl.txt")),
    ("pl", include_str!("../lang/pl.txt")),
    ("pt-BR", include_str!("../lang/pt-BR.txt")),
    ("ro", include_str!("../lang/ro.txt")),
    ("ru", include_str!("../lang/ru.txt")),
    ("sv", include_str!("../lang/sv.txt")),
    ("th", include_str!("../lang/th.txt")),
    ("tr", include_str!("../lang/tr.txt")),
    ("uk", include_str!("../lang/uk.txt")),
    ("vi", include_str!("../lang/vi.txt")),
    ("zh-CN", include_str!("../lang/zh-CN.txt")),
    ("zh-TW", include_str!("../lang/zh-TW.txt")),
];

/// One parsed language file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Language {
    pub code: String,
    /// Name in the language itself ("Polski").
    pub name: String,
    /// Decimal separator from `@decimal` ("," or "."); empty when the file has none.
    pub decimal: String,
    pub texts: HashMap<String, String>,
    /// Where it came from: None for a built-in language.
    pub file: Option<PathBuf>,
}

fn unescape(s: &str) -> String {
    s.replace("\\n", "\n")
}

/// Parses a language file. Lines that are not `text = translation` are ignored.
pub fn parse(src: &str) -> Language {
    let mut l = Language::default();
    for line in src.lines() {
        let line = line.trim_start_matches('\u{FEFF}').trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(" = ").or_else(|| line.strip_suffix(" =").map(|k| (k, ""))) else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "@code" => l.code = v.to_string(),
            "@name" => l.name = v.to_string(),
            "@decimal" => l.decimal = v.to_string(),
            _ if k.starts_with('@') => {}
            _ if !v.is_empty() => {
                l.texts.insert(unescape(k), unescape(v));
            }
            _ => {}
        }
    }
    l
}

/// `%LOCALAPPDATA%\BatteryHub\Languages`: the user's own language files.
pub fn user_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("BatteryHub").join("Languages")
}

fn user_files(dir: &Path) -> Vec<Language> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Language> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("txt")))
        .filter_map(|p| {
            let text = std::fs::read(&p).ok()?;
            let mut l = parse(&String::from_utf8_lossy(&text));
            if l.code.is_empty() {
                l.code = p.file_stem()?.to_string_lossy().into_owned();
            }
            if l.name.is_empty() {
                l.name = l.code.clone();
            }
            l.file = Some(p);
            Some(l)
        })
        .collect();
    out.sort_by(|a, b| a.code.cmp(&b.code));
    out
}

/// Every language offered in the menu (built-in and the user's), as (code, name), sorted by name.
/// A user file with a built-in code is merged into that language.
pub fn available() -> Vec<(String, String)> {
    available_in(&user_dir())
}

fn available_in(dir: &Path) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = BUILT_IN.iter().map(|(c, src)| (c.to_string(), parse(src).name)).collect();
    for u in user_files(dir) {
        if !list.iter().any(|(c, _)| c.eq_ignore_ascii_case(&u.code)) {
            list.push((u.code, u.name));
        }
    }
    list.sort_by_key(|(_, n)| n.to_lowercase());
    list
}

/// The texts of one language: built-in file first, then the user's files with that code on top.
pub fn load(code: &str) -> HashMap<String, String> {
    load_in(code, &user_dir())
}

fn load_in(code: &str, dir: &Path) -> HashMap<String, String> {
    let mut texts = HashMap::new();
    if code.eq_ignore_ascii_case("en") {
        return texts; // English is the program's own text
    }
    if let Some((_, src)) = BUILT_IN.iter().find(|(c, _)| c.eq_ignore_ascii_case(code)) {
        texts = parse(src).texts;
    }
    for u in user_files(dir).into_iter().filter(|u| u.code.eq_ignore_ascii_case(code)) {
        texts.extend(u.texts);
    }
    texts
}

/// The decimal separator of a language: the user's file with that code if it sets one, else the
/// built-in file's, else ".".
fn decimal_in(code: &str, dir: &Path) -> char {
    let built_in = BUILT_IN.iter().find(|(c, _)| c.eq_ignore_ascii_case(code)).map(|(_, src)| parse(src).decimal);
    let user = user_files(dir).into_iter().filter(|u| u.code.eq_ignore_ascii_case(code) && !u.decimal.is_empty()).map(|u| u.decimal).next_back();
    user.or(built_in).and_then(|d| d.chars().next()).unwrap_or('.')
}

/// The best language for a Windows UI language name ("pl-PL", "pt-BR", "zh-Hant-TW"): an exact
/// match, then the same language, then English.
pub fn pick(windows: &str, codes: &[String]) -> String {
    let w = windows.to_ascii_lowercase();
    if let Some(c) = codes.iter().find(|c| c.to_ascii_lowercase() == w) {
        return c.clone();
    }
    // Chinese: Traditional for Taiwan / Hong Kong / Macau and Hant, Simplified otherwise.
    if w.starts_with("zh") {
        let trad = ["-tw", "-hk", "-mo", "-hant"].iter().any(|s| w.contains(s));
        let want = if trad { "zh-tw" } else { "zh-cn" };
        if let Some(c) = codes.iter().find(|c| c.to_ascii_lowercase() == want) {
            return c.clone();
        }
    }
    let primary = w.split('-').next().unwrap_or("");
    codes
        .iter()
        .find(|c| c.to_ascii_lowercase() == primary)
        .or_else(|| codes.iter().find(|c| c.to_ascii_lowercase().split('-').next() == Some(primary)))
        .cloned()
        .unwrap_or_else(|| "en".into())
}

/// Windows' display language ("pl-PL").
pub fn windows_language() -> String {
    use windows::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};
    let mut buf = [0u16; 85];
    let n = unsafe { LCIDToLocaleName(GetUserDefaultUILanguage() as u32, Some(&mut buf), 0) };
    if n <= 1 { "en-US".into() } else { String::from_utf16_lossy(&buf[..n as usize - 1]) }
}

struct Current {
    code: String,
    texts: HashMap<String, String>,
    decimal: char,
}

static CURRENT: RwLock<Option<Current>> = RwLock::new(None);

/// Switches the program to a language: a code from `available()`, or "" for Windows' language.
/// Returns the code in use.
pub fn set_language(choice: &str) -> String {
    let code = if choice.is_empty() {
        let codes: Vec<String> = available().into_iter().map(|(c, _)| c).collect();
        pick(&windows_language(), &codes)
    } else {
        choice.to_string()
    };
    let texts = load(&code);
    let decimal = decimal_in(&code, &user_dir());
    if let Ok(mut c) = CURRENT.write() {
        *c = Some(Current { code: code.clone(), texts, decimal });
    }
    code
}

/// The language in use ("en" until `set_language` is called).
pub fn current() -> String {
    CURRENT.read().ok().and_then(|c| c.as_ref().map(|c| c.code.clone())).unwrap_or_else(|| "en".into())
}

/// The translation of an English text, or the text itself.
pub fn tr(english: &str) -> String {
    CURRENT.read().ok().and_then(|c| c.as_ref().and_then(|c| c.texts.get(english).cloned())).unwrap_or_else(|| english.to_string())
}

/// `tr` with `{placeholders}` filled in: `trf("{name}: {level}%", &[("name", &n), ("level", &l)])`.
pub fn trf(english: &str, args: &[(&str, &str)]) -> String {
    fill(&tr(english), args)
}

/// The decimal separator of the language in use ('.' until `set_language` is called).
pub fn decimal_separator() -> char {
    CURRENT.read().ok().and_then(|c| c.as_ref().map(|c| c.decimal)).unwrap_or('.')
}

/// A number with `digits` decimals and the language's separator: `decimal(3.94, 1)` is "3.9" in
/// English, "3,9" in Polish.
pub fn decimal(x: f64, digits: usize) -> String {
    with_separator(&format!("{x:.digits$}"), decimal_separator())
}

fn with_separator(number: &str, separator: char) -> String {
    if separator == '.' { number.to_string() } else { number.replacen('.', &separator.to_string(), 1) }
}

pub fn fill(text: &str, args: &[(&str, &str)]) -> String {
    let mut s = text.to_string();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// Writes a template for a new language (every text, with the current language's translation or
/// the English text to start from) into the user's folder, and returns its path. An existing file
/// of that name is kept.
pub fn create_template() -> std::io::Result<PathBuf> {
    let dir = user_dir();
    std::fs::create_dir_all(&dir)?;
    let mut path = dir.join("my-language.txt");
    let mut i = 2;
    while path.exists() {
        path = dir.join(format!("my-language-{i}.txt"));
        i += 1;
    }
    std::fs::write(&path, template(&load(&current())))?;
    Ok(path)
}

fn escape(s: &str) -> String {
    s.replace('\n', "\\n")
}

pub fn template(start: &HashMap<String, String>) -> String {
    let mut out = String::from(
        "# BatteryHub language file (UTF-8).\n\
         # Each line: English text = translation. Keep {placeholders} and \\n (line break) as they are.\n\
         # Change @code (e.g. \"eo\") and @name (the language's own name), save the file, then pick it in\n\
         # the right-click menu: Language. A line left empty, or removed, shows the English text.\n\
         # @decimal is the decimal separator: \".\" (3.9) or \",\" (3,9).\n\
         @code = custom\n@name = My language\n",
    );
    out.push_str(&format!("@decimal = {}\n\n", decimal_separator()));
    let en = parse(BUILT_IN[0].1);
    let mut keys: Vec<&String> = en.texts.keys().collect();
    keys.sort();
    for k in keys {
        let v = start.get(k).unwrap_or(k);
        out.push_str(&format!("{} = {}\n", escape(k), escape(v)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn english_keys() -> BTreeSet<String> {
        parse(BUILT_IN[0].1).texts.into_keys().collect()
    }

    /// Every `tr("...")` / `trf("...")` text in the sources (string literals, `\n` and `\"` read).
    fn source_keys() -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut files = vec![PathBuf::from("src/bin/taskbar.rs")];
        for e in std::fs::read_dir("src").unwrap().flatten() {
            if e.path().extension().is_some_and(|x| x == "rs") && !e.path().ends_with("i18n.rs") {
                files.push(e.path());
            }
        }
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            // The text may start on the next line (rustfmt breaks long calls).
            for pat in ["tr(", "trf(", "n_("] {
                let mut rest = src.as_str();
                while let Some(i) = rest.find(pat) {
                    let before = rest[..i].chars().last();
                    rest = &rest[i + pat.len()..];
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue; // e.g. "attr("
                    }
                    let Some(r) = rest.trim_start().strip_prefix('"') else { continue };
                    rest = r;
                    let mut s = String::new();
                    let mut chars = rest.chars();
                    while let Some(c) = chars.next() {
                        match c {
                            '"' => break,
                            '\\' => match chars.next() {
                                Some('n') => s.push('\n'),
                                Some(o) => s.push(o),
                                None => break,
                            },
                            c => s.push(c),
                        }
                    }
                    out.insert(s);
                }
            }
        }
        out
    }

    #[test]
    fn english_lists_exactly_the_texts_of_the_program() {
        let (src, en) = (source_keys(), english_keys());
        let missing: Vec<_> = src.difference(&en).collect();
        let unused: Vec<_> = en.difference(&src).collect();
        assert!(missing.is_empty(), "add to lang/en.txt: {missing:#?}");
        assert!(unused.is_empty(), "not used any more: {unused:#?}");
    }

    /// The connection texts drivers set (`via: "..."`, `.via = "..."`, also in `if` / `else` arms
    /// over several lines): drivers are not scanned for `tr(` / `n_(`, so these reach lang/en.txt
    /// through `device::CONNECTIONS`.
    fn driver_connection_texts() -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for e in std::fs::read_dir("src/drivers").unwrap().flatten() {
            let src = std::fs::read_to_string(e.path()).unwrap();
            for pat in ["via:", ".via ="] {
                for (i, _) in src.match_indices(pat) {
                    let (before, value) = (&src[..i], &src[i + pat.len()..]);
                    if (pat == "via:" && before.ends_with(|c: char| c == '"' || c == '_' || c.is_alphanumeric())) || value.starts_with('=') {
                        continue; // "via:{...}" in a key, "via_name", a `==` comparison
                    }
                    // String literals up to the end of the value: a `,` or `;` outside brackets.
                    let (mut depth, mut chars) = (0, value.chars());
                    while let Some(c) = chars.next() {
                        match c {
                            '"' => {
                                out.insert(chars.by_ref().take_while(|&c| c != '"').collect());
                            }
                            '(' | '{' => depth += 1,
                            ')' | '}' if depth == 0 => break,
                            ')' | '}' => depth -= 1,
                            ',' | ';' if depth == 0 => break,
                            _ => {}
                        }
                    }
                }
            }
        }
        out
    }

    #[test]
    fn driver_connections_are_translatable() {
        // Names of products, protocols and standards: the same in every language.
        const NAMES: [&str; 5] = ["Bluetooth", "HID", "Jabra Link 390", "Xbox Wireless", "Xbox Wireless / Bluetooth"];
        let listed: BTreeSet<&str> = crate::device::CONNECTIONS.iter().copied().chain(NAMES).collect();
        let found = driver_connection_texts();
        assert!(found.contains("Logitech receiver") && found.contains("USB cable"), "{found:#?}");
        let missing: Vec<_> = found.iter().filter(|t| !listed.contains(t.as_str())).collect();
        assert!(missing.is_empty(), "add to device::CONNECTIONS (and lang/*.txt), or to NAMES if it is a name: {missing:#?}");
    }

    #[test]
    fn every_language_is_complete_and_keeps_placeholders() {
        let en = english_keys();
        let ph = |s: &str| -> BTreeSet<String> {
            let mut v = BTreeSet::new();
            let mut r = s;
            while let Some(i) = r.find('{') {
                let Some(j) = r[i..].find('}') else { break };
                v.insert(r[i..i + j + 1].to_string());
                r = &r[i + j + 1..];
            }
            v
        };
        for (code, src) in BUILT_IN {
            let l = parse(src);
            assert_eq!(&l.code, code, "@code of lang/{code}.txt");
            assert!(!l.name.is_empty(), "@name of lang/{code}.txt");
            let keys: BTreeSet<String> = l.texts.keys().cloned().collect();
            let missing: Vec<_> = en.difference(&keys).collect();
            let extra: Vec<_> = keys.difference(&en).collect();
            assert!(missing.is_empty(), "lang/{code}.txt misses {missing:#?}");
            assert!(extra.is_empty(), "lang/{code}.txt has unknown texts {extra:#?}");
            for (k, v) in &l.texts {
                assert_eq!(ph(k), ph(v), "placeholders in lang/{code}.txt: {k}");
                assert_eq!(k.matches('\n').count() > 0, v.matches('\n').count() > 0, "line breaks in lang/{code}.txt: {k}");
            }
        }
    }

    #[test]
    fn parse_and_fallback() {
        let l = parse("\u{FEFF}# c\n@code = xx\n@name = Test\nRefresh = Odswiez\nEmpty =\nA\\nB = C\\nD\nnot a line\n");
        assert_eq!((l.code.as_str(), l.name.as_str()), ("xx", "Test"));
        assert_eq!(l.texts.get("Refresh").map(String::as_str), Some("Odswiez"));
        assert!(!l.texts.contains_key("Empty"));
        assert_eq!(l.texts.get("A\nB").map(String::as_str), Some("C\nD"));
        assert_eq!(fill("{name}: {level}%", &[("name", "Mouse"), ("level", "40")]), "Mouse: 40%");
        assert_eq!(tr("Text that has no translation"), "Text that has no translation");
    }

    #[test]
    fn windows_language_is_matched() {
        let codes: Vec<String> = BUILT_IN.iter().map(|(c, _)| c.to_string()).collect();
        assert_eq!(pick("pl-PL", &codes), "pl");
        assert_eq!(pick("pt-BR", &codes), "pt-BR");
        assert_eq!(pick("pt-PT", &codes), "pt-BR");
        assert_eq!(pick("zh-TW", &codes), "zh-TW");
        assert_eq!(pick("zh-Hant-HK", &codes), "zh-TW");
        assert_eq!(pick("zh-SG", &codes), "zh-CN");
        assert_eq!(pick("de-AT", &codes), "de");
        assert_eq!(pick("ro-RO", &codes), "ro");
        assert_eq!(pick("fi-FI", &codes), "en");
    }

    #[test]
    fn decimal_separator_per_language() {
        assert_eq!(parse("@code = xx\n@decimal = ,\nA = B\n").decimal, ",");
        assert_eq!(with_separator("3.9", ','), "3,9");
        assert_eq!(with_separator("12", ','), "12");
        assert_eq!(with_separator("3.9", '.'), "3.9");
        // Comma where it is the norm, the default dot elsewhere; every file says which.
        const COMMA: [&str; 16] = ["cs", "de", "es", "fr", "hu", "id", "it", "nl", "pl", "pt-BR", "ro", "ru", "sv", "tr", "uk", "vi"];
        for (code, src) in BUILT_IN {
            let want = if COMMA.contains(code) { "," } else { "." };
            assert_eq!(parse(src).decimal, want, "@decimal of lang/{code}.txt");
        }
        // A user file can set it, also for a built-in language.
        let dir = std::env::temp_dir().join(format!("bh-decimal-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("eo.txt"), "@code = eo\n@decimal = ,\n").unwrap();
        std::fs::write(dir.join("de-fix.txt"), "@code = de\n@decimal = .\n").unwrap();
        std::fs::write(dir.join("pl-fix.txt"), "@code = pl\nExit = Zakończ\n").unwrap();
        assert_eq!(decimal_in("eo", &dir), ',');
        assert_eq!(decimal_in("de", &dir), '.');
        assert_eq!(decimal_in("pl", &dir), ','); // no @decimal in the user file: the built-in one
        assert_eq!(decimal_in("en", &dir), '.');
        assert_eq!(decimal_in("xx", &dir), '.');
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn user_files_add_and_override_languages() {
        let dir = std::env::temp_dir().join(format!("bh-lang-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("eo.txt"), "@name = Esperanto\nExit = Eliri\n").unwrap();
        std::fs::write(dir.join("pl-fix.txt"), "@code = pl\nExit = Zakończ program\n").unwrap();
        let list = available_in(&dir);
        assert!(list.iter().any(|(c, n)| c == "eo" && n == "Esperanto"));
        assert_eq!(list.iter().filter(|(c, _)| c == "pl").count(), 1);
        assert_eq!(load_in("eo", &dir).get("Exit").map(String::as_str), Some("Eliri"));
        let pl = load_in("pl", &dir);
        assert_eq!(pl.get("Exit").map(String::as_str), Some("Zakończ program"));
        assert!(pl.contains_key("Refresh devices")); // the rest of the built-in file stays
        let t = template(&pl);
        assert!(t.contains("Exit = Zakończ program\n") && t.contains("@code = custom") && t.contains("\n@decimal = .\n"));
        assert_eq!(parse(&t).texts.len(), english_keys().len());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
