mod support;

use steamtrain::rules::{self, Config};
use steamtrain::steam::Runtime;
use support::{fake_game, fake_profile};

fn config_from(json: serde_json::Value) -> Config {
    Config::from_value(json).unwrap()
}

#[test]
fn nvidia_proton_gets_nvapi_and_shader_cache() {
    let opts = rules::baseline(
        &fake_game("100", Runtime::Proton),
        &fake_profile("nvidia"),
        &Config::defaults(),
    );
    assert_eq!(
        opts,
        "PROTON_ENABLE_NVAPI=1 __GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1 %command%"
    );
}

#[test]
fn nvidia_native_gets_shader_cache_but_not_nvapi() {
    let opts = rules::baseline(
        &fake_game("100", Runtime::Native),
        &fake_profile("nvidia"),
        &Config::defaults(),
    );
    assert_eq!(opts, "__GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1 %command%");
}

#[test]
fn amd_native_gets_mesa_glthread() {
    let opts = rules::baseline(
        &fake_game("100", Runtime::Native),
        &fake_profile("amd"),
        &Config::defaults(),
    );
    assert_eq!(opts, "mesa_glthread=true %command%");
}

#[test]
fn amd_proton_gets_no_mesa_glthread() {
    let opts = rules::baseline(
        &fake_game("100", Runtime::Proton),
        &fake_profile("amd"),
        &Config::defaults(),
    );
    assert_eq!(opts, "%command%");
}

#[test]
fn an_unknown_vendor_gets_no_vendor_options() {
    let opts = rules::baseline(
        &fake_game("100", Runtime::Native),
        &fake_profile("unknown"),
        &Config::defaults(),
    );
    assert_eq!(opts, "%command%");
}

#[test]
fn wrappers_follow_env_and_precede_command() {
    let mut profile = fake_profile("nvidia");
    profile.has_gamemode = true;
    profile.has_mangohud = true;
    let config = config_from(serde_json::json!({ "enable_mangohud": true }));

    let opts = rules::baseline(&fake_game("100", Runtime::Native), &profile, &config);
    assert_eq!(
        opts,
        "__GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1 gamemoderun mangohud %command%"
    );
}

#[test]
fn mangohud_is_off_by_default_even_when_installed() {
    let mut profile = fake_profile("amd");
    profile.has_mangohud = true;
    let opts = rules::baseline(
        &fake_game("100", Runtime::Native),
        &profile,
        &Config::defaults(),
    );
    assert!(!opts.contains("mangohud"), "got {opts}");
}

#[test]
fn a_disabled_rule_drops_its_option() {
    let mut profile = fake_profile("nvidia");
    profile.has_gamemode = true;
    let config = config_from(serde_json::json!({
        "enable_gamemode": false,
        "enable_shader_cache_skip_cleanup": false,
    }));
    let opts = rules::baseline(&fake_game("100", Runtime::Native), &profile, &config);
    assert_eq!(opts, "%command%");
}

#[test]
fn proton_wayland_is_opt_in() {
    let config = config_from(serde_json::json!({ "enable_proton_wayland": true }));
    let opts = rules::baseline(
        &fake_game("100", Runtime::Proton),
        &fake_profile("amd"),
        &config,
    );
    assert_eq!(opts, "PROTON_ENABLE_WAYLAND=1 %command%");
}

#[test]
fn proton_wayland_needs_a_wayland_session() {
    let mut profile = fake_profile("amd");
    profile.session = "x11".to_string();
    let config = config_from(serde_json::json!({ "enable_proton_wayland": true }));
    let opts = rules::baseline(&fake_game("100", Runtime::Proton), &profile, &config);
    assert_eq!(opts, "%command%");
}

#[test]
fn an_excluded_appid_yields_no_options() {
    let config = config_from(serde_json::json!({ "exclude": ["100"] }));
    assert_eq!(
        rules::build_options(
            &fake_game("100", Runtime::Native),
            &fake_profile("amd"),
            &config
        ),
        None
    );
}

#[test]
fn numeric_exclude_entries_are_coerced_to_strings() {
    let config = config_from(serde_json::json!({ "exclude": [100] }));
    assert_eq!(
        rules::build_options(
            &fake_game("100", Runtime::Native),
            &fake_profile("amd"),
            &config
        ),
        None
    );
}

#[test]
fn an_override_replaces_the_baseline_verbatim() {
    let config = config_from(serde_json::json!({ "overrides": { "100": "-novid %command%" } }));
    assert_eq!(
        rules::build_options(
            &fake_game("100", Runtime::Native),
            &fake_profile("amd"),
            &config
        )
        .unwrap(),
        "-novid %command%"
    );
}

#[test]
fn auto_expands_to_the_generated_baseline() {
    let config = config_from(serde_json::json!({ "overrides": { "100": "{auto} -dx11" } }));
    assert_eq!(
        rules::build_options(
            &fake_game("100", Runtime::Native),
            &fake_profile("amd"),
            &config
        )
        .unwrap(),
        "mesa_glthread=true %command% -dx11"
    );
}

#[test]
fn an_override_for_another_appid_is_not_applied() {
    let config = config_from(serde_json::json!({ "overrides": { "999": "-novid %command%" } }));
    assert_eq!(
        rules::build_options(
            &fake_game("100", Runtime::Native),
            &fake_profile("amd"),
            &config
        )
        .unwrap(),
        "mesa_glthread=true %command%"
    );
}

#[test]
fn load_config_creates_a_documented_default_on_first_run() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("nested/config.json");

    let config = rules::load_config(&path).unwrap();

    assert!(path.is_file());
    assert_eq!(config.gpu_vendor_raw(), "");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("_doc"));
    assert!(text.contains("protondb.com"));
    assert!(text.ends_with('\n'));
    // json.dumps(indent=2) shape, so a user editing it by hand sees what they
    // saw before.
    assert!(text.starts_with("{\n  \""), "got {text:.40}");
}

#[test]
fn load_config_fills_missing_keys_from_defaults() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    std::fs::write(&path, r#"{"enable_gamemode": false}"#).unwrap();

    let config = rules::load_config(&path).unwrap();
    assert!(!config.flag("enable_gamemode"));
    assert!(config.flag("enable_nvapi"), "defaulted");
    assert_eq!(config.advisor_command(), "claude -p");
}

#[test]
fn invalid_json_is_an_error_naming_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    std::fs::write(&path, "{not json").unwrap();

    let err = rules::load_config(&path).unwrap_err().to_string();
    assert!(err.contains("config.json"), "got {err}");
    assert!(
        err.contains("delete it to regenerate defaults"),
        "got {err}"
    );
}

#[test]
fn a_non_object_top_level_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    std::fs::write(&path, "[]").unwrap();

    let err = rules::load_config(&path).unwrap_err().to_string();
    assert!(err.contains("must be a JSON object"), "got {err}");
    assert!(err.contains("not list"), "got {err}");
}

#[test]
fn save_override_merges_and_preserves_unknown_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"zzz_custom": 7, "overrides": {"1": "a %command%"}}"#,
    )
    .unwrap();

    rules::save_override(&path, "100", "{auto} -dx11").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(data["zzz_custom"], 7);
    assert_eq!(data["overrides"]["1"], "a %command%");
    assert_eq!(data["overrides"]["100"], "{auto} -dx11");
}

#[test]
fn save_override_creates_the_file_when_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");

    rules::save_override(&path, "100", "-novid %command%").unwrap();

    let config = rules::load_config(&path).unwrap();
    assert_eq!(config.override_for("100"), Some("-novid %command%"));
}

#[test]
fn save_gpu_vendor_merges_into_an_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    std::fs::write(&path, r#"{"exclude": ["9"]}"#).unwrap();

    rules::save_gpu_vendor(&path, "nvidia").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(data["gpu_vendor"], "nvidia");
    assert_eq!(data["exclude"][0], "9");
}

#[test]
fn save_gpu_vendor_can_clear_the_override() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    rules::save_gpu_vendor(&path, "nvidia").unwrap();

    rules::save_gpu_vendor(&path, "").unwrap();

    assert_eq!(rules::load_config(&path).unwrap().gpu_vendor_raw(), "");
}

#[test]
fn a_users_key_order_survives_a_save() {
    // The file is the user's; rewriting it must not reshuffle what they wrote.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    std::fs::write(
        &path,
        "{\n  \"zebra\": 1,\n  \"gpu_vendor\": \"amd\",\n  \"alpha\": 2\n}\n",
    )
    .unwrap();

    rules::save_gpu_vendor(&path, "intel").unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    let zebra = text.find("zebra").unwrap();
    let vendor = text.find("gpu_vendor").unwrap();
    let alpha = text.find("alpha").unwrap();
    assert!(zebra < vendor && vendor < alpha, "reordered: {text}");
}
