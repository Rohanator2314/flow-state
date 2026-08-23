//! Offline spell checking backed by Hunspell-format dictionaries.
//!
//! This module owns dictionary discovery, prose tokenization, and byte-accurate
//! issue spans. It deliberately has no UI or application-state dependencies.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::{fs::OpenOptions, io::Write};

use spellbook::Dictionary;
use unicode_segmentation::UnicodeSegmentation;

use crate::core::text::Pos;

#[derive(Debug, Clone)]
pub struct LoadedDictionary {
    pub dictionary: Arc<RwLock<Dictionary>>,
    pub aff_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellIssue {
    pub start: Pos,
    pub end: Pos,
    pub word: String,
}

/// Load a configured dictionary, or discover `<language>.aff/.dic` in the
/// usual application and system Hunspell directories.
pub fn load_dictionary(language: &str, configured: &str) -> Result<LoadedDictionary, String> {
    let explicit = !configured.trim().is_empty();
    let candidates = dictionary_candidates(language, configured);
    let mut attempted = Vec::new();

    for (aff_path, dic_path) in candidates {
        attempted.push(aff_path.display().to_string());
        if !aff_path.is_file() || !dic_path.is_file() {
            continue;
        }
        let aff = std::fs::read_to_string(&aff_path)
            .map_err(|e| format!("could not read {}: {e}", aff_path.display()))?;
        let dic = std::fs::read_to_string(&dic_path)
            .map_err(|e| format!("could not read {}: {e}", dic_path.display()))?;
        let mut dictionary = Dictionary::new(&aff, &dic)
            .map_err(|e| format!("could not parse {}: {e}", aff_path.display()))?;
        load_personal_words(&mut dictionary, language)?;
        return Ok(LoadedDictionary {
            dictionary: Arc::new(RwLock::new(dictionary)),
            aff_path,
        });
    }

    let qualifier = if explicit { "configured" } else { language };
    Err(format!(
        "no {qualifier} Hunspell dictionary found (tried {})",
        attempted.join(", ")
    ))
}

/// Per-language word list maintained by flow-state. Unlike a Hunspell `.dic`,
/// this is one plain word per line with no count header or affix flags.
pub fn personal_dictionary_path(language: &str) -> Result<PathBuf, String> {
    let mut components = Path::new(language).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err("spell language must be one plain locale name".to_string());
    }
    let dir = crate::core::config::config_dir().ok_or("no config directory")?;
    Ok(dir
        .join("dictionaries")
        .join(format!("{language}.personal")))
}

/// Persist `word` once in a personal dictionary. The caller adds it to the
/// live `Dictionary` only after this succeeds, so a disk failure cannot create
/// a session-only exception that silently disappears at restart.
pub fn save_personal_word(path: &Path, word: &str) -> Result<(), String> {
    let word = word.trim();
    if !is_candidate(word) {
        return Err("personal dictionary words must contain at least two letters".to_string());
    }
    let existing = match std::fs::read_to_string(path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    if existing.lines().any(|line| line.trim() == word) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        file.write_all(b"\n")
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    }
    writeln!(file, "{word}").map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn load_personal_words(dictionary: &mut Dictionary, language: &str) -> Result<(), String> {
    let Ok(path) = personal_dictionary_path(language) else {
        return Ok(());
    };
    let words = match std::fs::read_to_string(&path) {
        Ok(words) => words,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    for (index, word) in words.lines().map(str::trim).enumerate() {
        if word.is_empty() {
            continue;
        }
        dictionary.add(word).map_err(|error| {
            format!(
                "invalid personal dictionary entry at {}:{}: {error}",
                path.display(),
                index + 1
            )
        })?;
    }
    Ok(())
}

fn dictionary_candidates(language: &str, configured: &str) -> Vec<(PathBuf, PathBuf)> {
    if !configured.trim().is_empty() {
        let path = PathBuf::from(configured.trim());
        let base = if path.is_dir() {
            path.join(language)
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("aff" | "dic")
        ) {
            path.with_extension("")
        } else {
            path
        };
        return vec![(base.with_extension("aff"), base.with_extension("dic"))];
    }

    let mut bases = Vec::new();
    if let Some(dir) = crate::core::config::config_dir() {
        bases.push(dir.join("dictionaries").join(language));
    }
    bases.extend(
        [
            "/usr/share/hunspell",
            "/usr/share/myspell",
            "/usr/share/myspell/dicts",
        ]
        .into_iter()
        .map(|dir| Path::new(dir).join(language)),
    );
    bases
        .into_iter()
        .map(|base| (base.with_extension("aff"), base.with_extension("dic")))
        .collect()
}

/// Return every misspelled prose word as a `(line, byte-column)` span.
pub fn check_text(dictionary: &Dictionary, input: &str) -> Vec<SpellIssue> {
    input
        .split('\n')
        .enumerate()
        .flat_map(|(line_index, line)| {
            line.unicode_word_indices()
                .filter_map(move |(start, word)| {
                    let end = start + word.len();
                    (is_candidate(word) && !is_ignored_context(line, start, end))
                        .then(|| {
                            (!dictionary.check(word)).then(|| SpellIssue {
                                start: (line_index, start),
                                end: (line_index, end),
                                word: word.to_string(),
                            })
                        })
                        .flatten()
                })
        })
        .collect()
}

/// Find a spelling issue under the caret, or immediately adjacent to it.
pub fn issue_near(issues: &[SpellIssue], cursor: Pos) -> Option<&SpellIssue> {
    issues.iter().find(|issue| {
        issue.start.0 == cursor.0 && issue.start.1 <= cursor.1 && cursor.1 <= issue.end.1
    })
}

/// The first issue after `cursor`, wrapping to the document's first issue.
pub fn next_issue(issues: &[SpellIssue], cursor: Pos) -> Option<&SpellIssue> {
    issues
        .iter()
        .find(|issue| issue.start > cursor)
        .or_else(|| issues.first())
}

fn is_candidate(word: &str) -> bool {
    let letters: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
    letters.len() > 1
        && word
            .chars()
            .all(|c| c.is_alphabetic() || matches!(c, '\'' | '’'))
        && !letters.iter().all(|c| c.is_uppercase())
}

fn is_ignored_context(line: &str, start: usize, end: usize) -> bool {
    let left = line[..start]
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(index, c)| index + c.len_utf8());
    let right = line[end..]
        .char_indices()
        .find(|(_, c)| c.is_whitespace())
        .map_or(line.len(), |(index, _)| end + index);
    let token = &line[left..right];
    token.contains("://")
        || token.contains('@')
        || token.contains('/')
        || token.starts_with('\\')
        || token.starts_with('`')
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn dictionary() -> Dictionary {
        Dictionary::new(
            "SET UTF-8\n",
            "8\nhello\nworld\nand\ncafé\nemail\nNASA\npath\nexample\n",
        )
        .unwrap()
    }

    #[test]
    fn reports_byte_accurate_unicode_spans_and_ignores_non_prose() {
        let issues = check_text(
            &dictionary(),
            "hello wurld café\nNASA https://bad.example/path me@example.com",
        );
        assert_eq!(
            issues,
            vec![SpellIssue {
                start: (0, 6),
                end: (0, 11),
                word: "wurld".to_string(),
            }]
        );
    }

    #[test]
    fn finds_issue_at_or_next_to_the_caret() {
        let issues = check_text(&dictionary(), "hello wurld");
        assert_eq!(issue_near(&issues, (0, 8)).unwrap().word, "wurld");
        assert_eq!(issue_near(&issues, (0, 11)).unwrap().word, "wurld");
        assert!(issue_near(&issues, (0, 2)).is_none());
    }

    #[test]
    fn next_issue_advances_and_wraps() {
        let issues = check_text(&dictionary(), "wurld hello\nhello wurld");
        assert_eq!(next_issue(&issues, (0, 2)).unwrap().start, (1, 6));
        assert_eq!(next_issue(&issues, (1, 9)).unwrap().start, (0, 0));
        assert!(next_issue(&[], (0, 0)).is_none());
    }

    #[test]
    fn loads_an_explicit_dictionary_basename() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("flow-state-spell-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("test_US");
        std::fs::write(base.with_extension("aff"), "SET UTF-8\n").unwrap();
        std::fs::write(base.with_extension("dic"), "2\nhello\nworld\n").unwrap();

        let loaded = load_dictionary("ignored", base.to_str().unwrap()).unwrap();
        let dictionary = loaded.dictionary.read().unwrap();
        assert!(dictionary.check("hello"));
        assert!(!dictionary.check("wurld"));
        drop(dictionary);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn personal_words_are_persisted_once() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("flow-state-personal-{unique}"));
        let path = dir.join("test.personal");

        save_personal_word(&path, "Flowstate").unwrap();
        save_personal_word(&path, "Flowstate").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Flowstate\n");
        assert!(save_personal_word(&path, "a").is_err());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_personal_word_updates_the_live_dictionary() {
        let mut dictionary = dictionary();
        assert!(!dictionary.check("Flowstate"));
        dictionary.add("Flowstate").unwrap();
        assert!(dictionary.check("Flowstate"));
    }

    #[test]
    fn personal_dictionary_locale_cannot_escape_the_config_directory() {
        assert!(personal_dictionary_path("../other").is_err());
        assert!(personal_dictionary_path("").is_err());
    }
}
