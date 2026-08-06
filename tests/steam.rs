mod support;

use steamtrain::steam::{self, Runtime};
use support::{make_compat_mapping, make_manifest, make_steam_root, make_user};

#[test]
fn finds_games_whose_manifest_and_folder_both_exist() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_manifest(&root, "100", "Fixture Game", "FixtureGame");

    let games = steam::installed_games(&root);
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].appid, "100");
    assert_eq!(games[0].name, "Fixture Game");
    assert_eq!(games[0].runtime, Runtime::Native);
    assert_eq!(games[0].library, root);
    assert!(games[0].installdir.is_dir());
}

#[test]
fn ignores_a_manifest_with_no_install_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_manifest(&root, "100", "Fixture Game", "FixtureGame");
    std::fs::remove_dir_all(root.join("steamapps/common/FixtureGame")).unwrap();

    assert!(steam::installed_games(&root).is_empty());
}

#[test]
fn excludes_proton_and_runtime_tools_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_manifest(&root, "1", "Proton 9.0", "Proton 9.0");
    make_manifest(&root, "2", "Steam Linux Runtime 3.0", "SteamLinuxRuntime");
    make_manifest(
        &root,
        "3",
        "Steamworks Common Redistributables",
        "Steamworks Shared",
    );
    make_manifest(&root, "100", "Fixture Game", "FixtureGame");

    let games = steam::installed_games(&root);
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].appid, "100");
}

#[test]
fn a_malformed_manifest_is_skipped_not_fatal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_manifest(&root, "100", "Fixture Game", "FixtureGame");
    std::fs::write(
        root.join("steamapps/appmanifest_666.acf"),
        "\"AppState\"\n{",
    )
    .unwrap();

    let games = steam::installed_games(&root);
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].appid, "100");
}

#[test]
fn compatdata_marks_a_game_as_proton() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_manifest(&root, "100", "Fixture Game", "FixtureGame");
    std::fs::create_dir_all(root.join("steamapps/compatdata/100")).unwrap();

    assert_eq!(steam::installed_games(&root)[0].runtime, Runtime::Proton);
}

#[test]
fn per_app_compat_mapping_marks_a_game_as_proton() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_manifest(&root, "100", "Fixture Game", "FixtureGame");
    make_compat_mapping(&root, &[("100", "proton_9")]);

    assert_eq!(steam::compat_mapping(&root).get("100").unwrap(), "proton_9");
    assert_eq!(steam::installed_games(&root)[0].runtime, Runtime::Proton);
}

#[test]
fn the_global_zero_mapping_is_ignored() {
    // "0" only affects titles that *need* compat, which cannot be known
    // offline, so only per-app signals are trusted.
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_manifest(&root, "100", "Fixture Game", "FixtureGame");
    make_compat_mapping(&root, &[("0", "proton_9")]);

    assert_eq!(steam::installed_games(&root)[0].runtime, Runtime::Native);
}

#[test]
fn an_empty_mapping_name_does_not_imply_proton() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_manifest(&root, "100", "Fixture Game", "FixtureGame");
    make_compat_mapping(&root, &[("100", "")]);

    assert_eq!(steam::installed_games(&root)[0].runtime, Runtime::Native);
}

#[test]
fn finds_games_in_a_mounted_second_library() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    let second = make_steam_root(&tmp.path().join("elsewhere"));
    make_manifest(&second, "200", "Second Game", "SecondGame");
    std::fs::write(
        root.join("steamapps/libraryfolders.vdf"),
        format!(
            "\"libraryfolders\"\n{{\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
            second.display()
        ),
    )
    .unwrap();

    let appids: Vec<String> = steam::installed_games(&root)
        .into_iter()
        .map(|game| game.appid)
        .collect();
    assert!(appids.iter().any(|appid| appid == "200"), "got {appids:?}");
}

#[test]
fn skips_an_unmounted_library() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    std::fs::write(
        root.join("steamapps/libraryfolders.vdf"),
        "\"libraryfolders\"\n{\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/nonexistent/drive\"\n\t}\n}\n",
    )
    .unwrap();

    assert_eq!(steam::library_paths(&root), vec![root.clone()]);
}

#[test]
fn manifests_are_visited_in_a_stable_order() {
    // Change ordering is observable in the NDJSON stream, so it must not
    // depend on the order the filesystem hands back directory entries.
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    for (appid, name) in [("300", "Gamma"), ("100", "Alpha"), ("200", "Beta")] {
        make_manifest(&root, appid, name, name);
    }

    let appids: Vec<String> = steam::installed_games(&root)
        .iter()
        .map(|game| game.appid.clone())
        .collect();
    assert_eq!(appids, vec!["100", "200", "300"]);
}

#[test]
fn lists_every_numeric_user_with_a_localconfig() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_user(&root, "111");
    make_user(&root, "222");
    // Neither of these counts: one is not numeric, the other has no config.
    std::fs::create_dir_all(root.join("userdata/anonymous/config")).unwrap();
    std::fs::create_dir_all(root.join("userdata/333")).unwrap();

    let users: Vec<String> = steam::user_localconfigs(&root)
        .into_iter()
        .map(|(user, _)| user)
        .collect();
    assert_eq!(users, vec!["111".to_string(), "222".to_string()]);
}

#[test]
fn no_userdata_directory_is_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    assert!(steam::user_localconfigs(&root).is_empty());
}

#[test]
fn is_steam_running_is_false_for_a_fixture_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    assert!(!steam::is_steam_running(&root));
}

#[test]
fn a_stale_pid_file_does_not_report_steam_as_running() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    // A pid that is either unused or belongs to something that is not Steam.
    std::fs::write(root.join("steam.pid"), "999999\n").unwrap();
    assert!(!steam::is_steam_running(&root));
}
