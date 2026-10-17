//! Both locales must define the same GUI message ids; a missing one would
//! silently show English (or a `[missing: ...]` placeholder) in the other.

use std::collections::BTreeSet;

const EN_US: &str = include_str!("../../locales/en-US/gui.ftl");
const JA_JP: &str = include_str!("../../locales/ja-JP/gui.ftl");

/// Message ids are lines of the form `^[a-z][a-z0-9-]* =`.
fn message_ids(ftl: &str) -> BTreeSet<&str> {
    ftl.lines()
        .filter_map(|line| {
            let (id, _) = line.split_once(" =")?;
            let mut chars = id.chars();
            let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
            let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            (first_ok && rest_ok).then_some(id)
        })
        .collect()
}

#[test]
fn ftl_parity() {
    let en = message_ids(EN_US);
    let ja = message_ids(JA_JP);
    assert!(!en.is_empty(), "no message ids parsed from en-US/gui.ftl");
    let only_en: Vec<_> = en.difference(&ja).collect();
    let only_ja: Vec<_> = ja.difference(&en).collect();
    assert!(
        only_en.is_empty() && only_ja.is_empty(),
        "gui.ftl id mismatch: only in en-US {only_en:?}, only in ja-JP {only_ja:?}"
    );
}
