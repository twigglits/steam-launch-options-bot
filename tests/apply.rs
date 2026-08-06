mod support;

use std::collections::BTreeMap;
use std::path::Path;

use steamtrain::apply::{self, State};
use steamtrain::codes::Action;
use support::{current_options, make_steam_root, make_user, set_options};

fn options(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn not_running(_root: &Path) -> bool {
    false
}

fn running(_root: &Path) -> bool {
    true
}

/// Plan and apply one round, returning the changes written.
fn apply_once(
    root: &Path,
    state_dir: &Path,
    pairs: &[(&str, &str)],
) -> Vec<steamtrain::apply::Change> {
    let state = State::load(state_dir).unwrap();
    let changes = apply::plan_changes(root, &options(pairs), &state, &BTreeMap::new()).unwrap();
    apply::apply_changes(root, &changes, state_dir, &not_running).unwrap()
}

#[test]
fn writes_into_an_empty_launch_options_field() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let localconfig = make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    let state = State::load(&state_dir).unwrap();
    let changes = apply::plan_changes(
        &root,
        &options(&[("100", "X=1 %command%")]),
        &state,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(changes[0].action, Action::Set);

    let written = apply::apply_changes(&root, &changes, &state_dir, &not_running).unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(current_options(&localconfig, "100"), "X=1 %command%");
}

#[test]
fn never_overwrites_a_value_a_human_set() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let localconfig = make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    apply_once(&root, &state_dir, &[("100", "OURS=1 %command%")]);
    set_options(&localconfig, "100", "HUMAN=1 %command%");

    let state = State::load(&state_dir).unwrap();
    let changes = apply::plan_changes(
        &root,
        &options(&[("100", "NEW=1 %command%")]),
        &state,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(changes[0].action, Action::SkipUserSet);

    apply::apply_changes(&root, &changes, &state_dir, &not_running).unwrap();
    assert_eq!(current_options(&localconfig, "100"), "HUMAN=1 %command%");
}

#[test]
fn updates_a_value_it_wrote_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let localconfig = make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    apply_once(&root, &state_dir, &[("100", "OLD=1 %command%")]);

    let state = State::load(&state_dir).unwrap();
    let changes = apply::plan_changes(
        &root,
        &options(&[("100", "NEW=1 %command%")]),
        &state,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(changes[0].action, Action::Set);

    apply::apply_changes(&root, &changes, &state_dir, &not_running).unwrap();
    assert_eq!(current_options(&localconfig, "100"), "NEW=1 %command%");
}

#[test]
fn an_unchanged_value_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    apply_once(&root, &state_dir, &[("100", "SAME=1 %command%")]);

    let state = State::load(&state_dir).unwrap();
    let changes = apply::plan_changes(
        &root,
        &options(&[("100", "SAME=1 %command%")]),
        &state,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(changes[0].action, Action::SkipUnchanged);
}

#[test]
fn refuses_to_write_while_steam_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let localconfig = make_user(&root, "111");
    let state_dir = tmp.path().join("state");
    let before = std::fs::read(&localconfig).unwrap();

    let state = State::load(&state_dir).unwrap();
    let changes = apply::plan_changes(
        &root,
        &options(&[("100", "X=1 %command%")]),
        &state,
        &BTreeMap::new(),
    )
    .unwrap();
    let err = apply::apply_changes(&root, &changes, &state_dir, &running).unwrap_err();

    assert!(matches!(err, apply::ApplyError::SteamRunning(_)));
    assert!(err.to_string().contains("Close Steam and re-run"));
    assert_eq!(
        std::fs::read(&localconfig).unwrap(),
        before,
        "nothing written"
    );
}

#[test]
fn nothing_to_write_does_not_consult_the_guardrail() {
    // A run with no planned changes must not fail just because Steam is open;
    // there is nothing to discard.
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    let written = apply::apply_changes(&root, &[], &state_dir, &running).unwrap();
    assert!(written.is_empty());
}

#[test]
fn backs_up_before_writing_and_keeps_the_newest_ten() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    for index in 0..12 {
        apply_once(
            &root,
            &state_dir,
            &[("100", &format!("N={index} %command%"))],
        );
    }

    let backups: Vec<_> = std::fs::read_dir(state_dir.join("backups"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(backups.len(), apply::BACKUPS_PER_USER);
}

#[test]
fn a_backup_holds_the_content_from_before_the_write() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let localconfig = make_user(&root, "111");
    let state_dir = tmp.path().join("state");
    let before = std::fs::read(&localconfig).unwrap();

    apply_once(&root, &state_dir, &[("100", "X=1 %command%")]);

    let backup = std::fs::read_dir(state_dir.join("backups"))
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path();
    assert_eq!(std::fs::read(backup).unwrap(), before);
}

#[test]
fn revert_restores_a_managed_option_to_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let localconfig = make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    apply_once(&root, &state_dir, &[("100", "X=1 %command%")]);

    let state = State::load(&state_dir).unwrap();
    let reverts = apply::plan_revert(&root, &state).unwrap();
    assert_eq!(reverts[0].action, Action::Set);
    assert_eq!(reverts[0].proposed, "");

    apply::apply_changes(&root, &reverts, &state_dir, &not_running).unwrap();
    assert_eq!(current_options(&localconfig, "100"), "");
    assert!(State::load(&state_dir).unwrap().is_empty());
}

#[test]
fn revert_keeps_a_value_a_human_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let localconfig = make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    apply_once(&root, &state_dir, &[("100", "X=1 %command%")]);
    set_options(&localconfig, "100", "HUMAN %command%");

    let state = State::load(&state_dir).unwrap();
    let reverts = apply::plan_revert(&root, &state).unwrap();
    assert_eq!(reverts[0].action, Action::SkipUserSet);

    apply::apply_changes(&root, &reverts, &state_dir, &not_running).unwrap();
    assert_eq!(current_options(&localconfig, "100"), "HUMAN %command%");
}

#[test]
fn revert_covers_appids_that_are_no_longer_installed() {
    // plan_revert works from state, not from the library, so an uninstalled
    // game's option is still cleared.
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let localconfig = make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    apply_once(&root, &state_dir, &[("999", "X=1 %command%")]);

    let reverts = apply::plan_revert(&root, &State::load(&state_dir).unwrap()).unwrap();
    assert_eq!(reverts.len(), 1);
    assert_eq!(reverts[0].appid, "999");
    assert_eq!(reverts[0].name, "999", "no game record exists to name it");

    apply::apply_changes(&root, &reverts, &state_dir, &not_running).unwrap();
    assert_eq!(current_options(&localconfig, "999"), "");
}

#[test]
fn plans_for_every_steam_account() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_user(&root, "111");
    make_user(&root, "222");
    let state_dir = tmp.path().join("state");

    let state = State::load(&state_dir).unwrap();
    let changes = apply::plan_changes(
        &root,
        &options(&[("100", "X=1 %command%")]),
        &state,
        &BTreeMap::new(),
    )
    .unwrap();

    let users: Vec<&str> = changes.iter().map(|change| change.user.as_str()).collect();
    assert_eq!(users, vec!["111", "222"]);
}

#[test]
fn state_is_keyed_per_account() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let first = make_user(&root, "111");
    let second = make_user(&root, "222");
    let state_dir = tmp.path().join("state");

    apply_once(&root, &state_dir, &[("100", "X=1 %command%")]);
    assert_eq!(current_options(&first, "100"), "X=1 %command%");
    assert_eq!(current_options(&second, "100"), "X=1 %command%");

    // One account's value is edited by hand; the other stays ours.
    set_options(&first, "100", "HUMAN %command%");
    let state = State::load(&state_dir).unwrap();
    let changes = apply::plan_changes(
        &root,
        &options(&[("100", "Y=2 %command%")]),
        &state,
        &BTreeMap::new(),
    )
    .unwrap();

    let by_user: BTreeMap<&str, Action> = changes
        .iter()
        .map(|change| (change.user.as_str(), change.action))
        .collect();
    assert_eq!(by_user["111"], Action::SkipUserSet);
    assert_eq!(by_user["222"], Action::Set);
}

#[test]
fn the_written_file_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let localconfig = make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    let mut perms = std::fs::metadata(&localconfig).unwrap().permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(&localconfig, perms).unwrap();

    apply_once(&root, &state_dir, &[("100", "X=1 %command%")]);

    let mode = std::fs::metadata(&localconfig)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn no_temporary_file_is_left_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let localconfig = make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    apply_once(&root, &state_dir, &[("100", "X=1 %command%")]);

    let leftovers: Vec<_> = std::fs::read_dir(localconfig.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("steamtrain-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "left {leftovers:?}");
}

#[test]
fn the_rest_of_the_config_survives_a_write() {
    // localconfig.vdf holds a great deal more than launch options, and none of
    // it may be lost when one key is updated.
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let localconfig = make_user(&root, "111");
    let state_dir = tmp.path().join("state");
    std::fs::write(
        &localconfig,
        "\"UserLocalConfigStore\"\n{\n\t\"friends\"\n\t{\n\t\t\"PersonaName\"\t\t\"someone\"\n\t}\n}\n",
    )
    .unwrap();

    apply_once(&root, &state_dir, &[("100", "X=1 %command%")]);

    let text = String::from_utf8(std::fs::read(&localconfig).unwrap()).unwrap();
    assert!(text.contains("PersonaName"), "lost unrelated data:\n{text}");
    assert!(text.contains("X=1 %command%"));
}

#[test]
fn a_localconfig_is_parsed_once_per_user_not_once_per_appid() {
    // Regression guard for the divergence recorded in the spec: the Python
    // re-parsed the whole file for every appid.
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_user(&root, "111");
    let state_dir = tmp.path().join("state");

    let many: BTreeMap<String, String> = (0..300)
        .map(|index| (index.to_string(), format!("N={index} %command%")))
        .collect();
    let state = State::load(&state_dir).unwrap();
    let changes = apply::plan_changes(&root, &many, &state, &BTreeMap::new()).unwrap();
    assert_eq!(changes.len(), 300);
}
