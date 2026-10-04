//! Platform-independent console monitor logic: profile mapping, filters, dedupe, ancestry walk.

use std::cell::Cell;
use std::time::{Duration, Instant};

use harness_services::console_monitor::dedupe_cache::{
    DedupeCache, EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND, MAX_ENTRIES,
};
use harness_services::console_monitor::process_ancestry::{
    walk_ancestry, ProcessEntry, ProcessTable, MAX_ANCESTRY_DEPTH,
};
use harness_services::console_monitor::profile_mapping::profile_from_path;
use harness_services::console_monitor::window_filter::{
    is_console_class, is_console_process, is_console_window, is_window_object_event,
    truncate_window_class,
};

fn table(rows: &[(u32, &str, u32)]) -> ProcessTable {
    rows.iter()
        .map(|(pid, name, parent)| {
            (
                *pid,
                ProcessEntry {
                    name: (*name).to_owned(),
                    parent_pid: *parent,
                },
            )
        })
        .collect()
}

#[test]
fn profile_mapping_matches_python() {
    assert_eq!(
        profile_from_path(r"C:\Users\x\.codex-cy\bin\codex.exe"),
        "cy"
    );
    assert_eq!(
        profile_from_path(r"C:\Users\x\.CODEX-FEU\bin\codex.exe"),
        "feu"
    );
    assert_eq!(
        profile_from_path("C:/Users/u/.codex/bin/codex.exe"),
        "personal"
    );
    assert_eq!(
        profile_from_path(r"C:\Program Files\codex\codex.exe"),
        "unknown"
    );
    assert_eq!(profile_from_path(""), "unknown");
}

#[test]
fn console_classification_matches_python() {
    assert!(is_console_window("PwSh.exe", "Whatever"));
    assert!(is_console_window("explorer.exe", "ConsoleWindowClass"));
    assert!(!is_console_window("explorer.exe", "CabinetWClass"));
    assert!(is_console_class("PseudoConsoleWindow"));
    assert!(!is_console_class("Chrome_WidgetWin_1"));
    assert!(is_console_process("OpenConsole.exe"));
    assert!(!is_console_process("unknown"));
}

#[test]
fn window_object_filter_requires_whole_window() {
    assert!(is_window_object_event(false, 0, 0));
    assert!(!is_window_object_event(true, 0, 0));
    assert!(!is_window_object_event(false, -4, 0));
    assert!(!is_window_object_event(false, 0, 3));
}

#[test]
fn window_class_truncates_to_80_characters() {
    let long: String = "é".repeat(100);
    assert_eq!(truncate_window_class(&long).chars().count(), 80);
    assert_eq!(truncate_window_class("Short"), "Short");
}

#[test]
fn dedupe_drops_repeats_within_two_seconds() {
    let mut cache = DedupeCache::new();
    let start = Instant::now();
    assert!(cache.admit((1, 10), EVENT_OBJECT_SHOW, start));
    assert!(!cache.admit(
        (1, 10),
        EVENT_OBJECT_SHOW,
        start + Duration::from_millis(1_999)
    ));
    assert!(cache.admit(
        (1, 10),
        EVENT_OBJECT_SHOW,
        start + Duration::from_millis(2_000)
    ));
    assert!(cache.admit((2, 10), EVENT_OBJECT_SHOW, start));
}

#[test]
fn foreground_for_seen_window_is_always_dropped() {
    let mut cache = DedupeCache::new();
    let start = Instant::now();
    assert!(cache.admit((7, 1), EVENT_SYSTEM_FOREGROUND, start));
    assert!(!cache.admit(
        (7, 1),
        EVENT_SYSTEM_FOREGROUND,
        start + Duration::from_secs(60)
    ));
    assert!(cache.admit((7, 1), EVENT_OBJECT_SHOW, start + Duration::from_secs(60)));
}

#[test]
fn cache_clears_after_exceeding_capacity() {
    let mut cache = DedupeCache::new();
    let now = Instant::now();
    for hwnd in 0..MAX_ENTRIES {
        assert!(cache.admit((hwnd, 1), EVENT_OBJECT_SHOW, now));
    }
    assert_eq!(cache.len(), MAX_ENTRIES);
    assert!(cache.admit((MAX_ENTRIES, 1), EVENT_OBJECT_SHOW, now));
    assert!(cache.is_empty());
    assert!(cache.admit((0, 1), EVENT_OBJECT_SHOW, now));
}

#[test]
fn ancestry_finds_codex_and_maps_profile() {
    let processes = table(&[
        (40, "cmd.exe", 30),
        (30, "node.exe", 20),
        (20, "Codex.exe", 10),
        (10, "explorer.exe", 0),
    ]);
    let lookups = Cell::new(0);
    let ancestry = walk_ancestry(40, &processes, |pid| {
        lookups.set(lookups.get() + 1);
        assert_eq!(pid, 20);
        r"C:\Users\x\.codex-feu\bin\codex.exe".to_owned()
    });
    assert_eq!(ancestry.codex_pid, Some(20));
    assert_eq!(ancestry.profile, "feu");
    assert_eq!(ancestry.chain.len(), 3);
    assert_eq!(ancestry.chain[0].parent_pid, 30);
    assert_eq!(ancestry.chain[2].process, "Codex.exe");
    assert_eq!(lookups.get(), 1);
}

#[test]
fn ancestry_without_codex_reports_unknown() {
    let processes = table(&[(5, "pwsh.exe", 99)]);
    let ancestry = walk_ancestry(5, &processes, |_| unreachable!("no codex ancestor"));
    assert_eq!(ancestry.codex_pid, None);
    assert_eq!(ancestry.profile, "unknown");
    assert_eq!(ancestry.chain.len(), 2);
    assert_eq!(ancestry.chain[1].process, "unknown");
    assert_eq!(ancestry.chain[1].parent_pid, 0);
}

#[test]
fn ancestry_stops_on_cycles_zero_and_depth() {
    let cyclic = table(&[(1, "a.exe", 2), (2, "b.exe", 1)]);
    assert_eq!(walk_ancestry(1, &cyclic, |_| String::new()).chain.len(), 2);
    assert!(walk_ancestry(0, &cyclic, |_| String::new())
        .chain
        .is_empty());

    let rows: Vec<(u32, &str, u32)> = (1..=30).map(|pid| (pid, "x.exe", pid + 1)).collect();
    let deep = table(&rows);
    assert_eq!(
        walk_ancestry(1, &deep, |_| String::new()).chain.len(),
        MAX_ANCESTRY_DEPTH
    );
}
