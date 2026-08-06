//! Ports of the Python setup tests.
//!
//! The two KeyboardInterrupt cases (exit 130) have no in-process equivalent
//! here and need none: Ctrl-C delivers SIGINT, whose default disposition
//! already terminates with 130, and this binary installs no handler that would
//! change that. There is nothing to assert that would not be asserting on the
//! kernel.

mod support;

use std::path::PathBuf;

use support::{Cli, Run};

struct Fixture {
    _tmp: tempfile::TempDir,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        Fixture {
            config: tmp.path().join("config.json"),
            _tmp: tmp,
        }
    }

    fn with_override(vendor: &str) -> Self {
        let fixture = Fixture::new();
        std::fs::write(&fixture.config, format!("{{\"gpu_vendor\": \"{vendor}\"}}")).unwrap();
        fixture
    }

    fn setup(&self, vendor: &str, stdin: &str) -> Run {
        Cli::new(vendor).run(
            &["setup", "--config", &self.config.display().to_string()],
            stdin,
        )
    }

    fn run(&self, args: &[&str]) -> Run {
        let mut argv: Vec<&str> = vec!["setup", "--config"];
        let config = self.config.display().to_string();
        argv.push(&config);
        argv.extend_from_slice(args);
        Cli::new("amd").run(&argv, "")
    }

    fn saved_vendor(&self) -> String {
        let text = std::fs::read_to_string(&self.config).unwrap();
        let data: serde_json::Value = serde_json::from_str(&text).unwrap();
        data["gpu_vendor"].as_str().unwrap_or_default().to_string()
    }
}

// -------------------------------------------------- autodetection failed

#[test]
fn an_undetected_vendor_is_chosen_from_the_menu() {
    let fixture = Fixture::new();
    let run = fixture.setup("unknown", "1\n");

    assert_eq!(run.code, 0);
    assert_eq!(fixture.saved_vendor(), "nvidia");
}

#[test]
fn a_failed_detection_skips_the_confirm_step() {
    let fixture = Fixture::new();
    let run = fixture.setup("unknown", "1\n");

    assert_eq!(run.code, 0);
    assert!(!run.out.contains("[Y/n]"), "got {}", run.out);
    assert_eq!(fixture.saved_vendor(), "nvidia");
}

#[test]
fn the_menu_reprompts_until_a_valid_number() {
    let fixture = Fixture::new();
    let run = fixture.setup("unknown", "9\nnonsense\n2\n");

    assert_eq!(run.code, 0);
    assert!(
        run.out.contains("Please enter a number 1-5."),
        "got {}",
        run.out
    );
    assert_eq!(fixture.saved_vendor(), "amd");
}

#[test]
fn skip_writes_nothing() {
    let fixture = Fixture::new();
    let run = fixture.setup("unknown", "5\n");

    assert_eq!(run.code, 0);
    assert!(
        run.out.contains("autodetection stays in effect"),
        "got {}",
        run.out
    );
    assert_eq!(fixture.saved_vendor(), "");
}

#[test]
fn end_of_input_at_the_menu_writes_nothing() {
    let fixture = Fixture::new();
    let run = fixture.setup("unknown", "");

    assert_eq!(run.code, 0);
    assert_eq!(fixture.saved_vendor(), "");
}

#[test]
fn skip_keeps_an_existing_override_and_names_it() {
    let fixture = Fixture::with_override("nvidia");
    let run = fixture.setup("unknown", "5\n");

    assert_eq!(run.code, 0);
    assert!(run.out.contains("gpu_vendor='nvidia'"), "got {}", run.out);
    assert!(run.out.contains("stays in effect"), "got {}", run.out);
    assert!(
        !run.out.contains("autodetection stays in effect"),
        "the override is what stays: {}",
        run.out
    );
    assert_eq!(fixture.saved_vendor(), "nvidia");
}

#[test]
fn an_existing_override_can_be_changed() {
    let fixture = Fixture::with_override("nvidia");
    let run = fixture.setup("unknown", "2\n");

    assert_eq!(run.code, 0);
    assert_eq!(fixture.saved_vendor(), "amd");
}

#[test]
fn an_unrecognised_override_is_noted_and_left_alone() {
    let fixture = Fixture::with_override("banana");
    let run = fixture.setup("unknown", "5\n");

    assert_eq!(run.code, 0);
    assert!(run.out.contains("not recognized"), "got {}", run.out);
    assert!(run.out.contains("gpu_vendor='banana'"), "got {}", run.out);
    assert_eq!(fixture.saved_vendor(), "banana", "untouched, still ignored");
}

// ----------------------------------------------------- autodetection worked

#[test]
fn confirming_the_detection_writes_no_vendor() {
    let fixture = Fixture::new();
    let run = fixture.setup("nvidia", "\n");

    assert_eq!(run.code, 0);
    assert!(run.out.contains("nvidia"), "got {}", run.out);
    assert!(
        !run.out.contains("1) NVIDIA"),
        "only the confirm prompt, no menu: {}",
        run.out
    );
    // load_config creates the documented default file; the point is that
    // confirming the detection writes no vendor.
    assert_eq!(fixture.saved_vendor(), "");
}

#[test]
fn an_explicit_yes_writes_nothing() {
    let fixture = Fixture::new();
    let run = fixture.setup("nvidia", "y\n");

    assert_eq!(run.code, 0);
    assert!(!run.out.contains("1) NVIDIA"), "got {}", run.out);
    assert_eq!(fixture.saved_vendor(), "");
}

#[test]
fn the_confirm_reprompts_on_an_ambiguous_answer() {
    let fixture = Fixture::new();
    let run = fixture.setup("nvidia", "nope\nn\n2\n");

    assert_eq!(run.code, 0);
    assert!(run.out.contains("Please answer y or n"), "got {}", run.out);
    assert_eq!(fixture.saved_vendor(), "amd");
}

#[test]
fn disagreeing_with_the_detection_persists_the_choice() {
    let fixture = Fixture::new();
    let run = fixture.setup("nvidia", "n\n2\n");

    assert_eq!(run.code, 0);
    assert_eq!(fixture.saved_vendor(), "amd");
}

#[test]
fn disagreeing_then_skipping_writes_nothing() {
    let fixture = Fixture::new();
    let run = fixture.setup("nvidia", "n\n5\n");

    assert_eq!(run.code, 0);
    assert_eq!(fixture.saved_vendor(), "");
}

#[test]
fn disagreeing_then_ending_input_writes_nothing() {
    let fixture = Fixture::new();
    let run = fixture.setup("nvidia", "n\n");

    assert_eq!(run.code, 0);
    assert_eq!(fixture.saved_vendor(), "");
}

#[test]
fn clearing_with_no_override_is_an_honest_noop() {
    let fixture = Fixture::new();
    let run = fixture.setup("nvidia", "n\n4\n");

    assert_eq!(run.code, 0);
    assert!(run.out.contains("already in effect"), "got {}", run.out);
    assert!(
        !run.out.contains("Cleared"),
        "nothing was cleared: {}",
        run.out
    );
    assert_eq!(fixture.saved_vendor(), "");
}

#[test]
fn an_active_override_is_reported_as_winning() {
    let fixture = Fixture::with_override("amd");
    let run = fixture.setup("nvidia", "\n");

    assert_eq!(run.code, 0);
    assert!(
        run.out.contains("wins over autodetection"),
        "got {}",
        run.out
    );
    assert_eq!(
        fixture.saved_vendor(),
        "amd",
        "confirm accepted, override untouched"
    );
}

#[test]
fn an_unrecognised_override_is_reported_as_ignored() {
    let fixture = Fixture::with_override("banana");
    let run = fixture.setup("nvidia", "\n");

    assert_eq!(run.code, 0);
    assert!(run.out.contains("not recognized"), "got {}", run.out);
}

#[test]
fn clearing_an_existing_override_restores_autodetection() {
    let fixture = Fixture::with_override("amd");
    let run = fixture.setup("nvidia", "n\n4\n");

    assert_eq!(run.code, 0);
    assert!(
        run.out.contains("autodetection is back in effect"),
        "got {}",
        run.out
    );
    assert_eq!(fixture.saved_vendor(), "");
}

#[test]
fn the_detected_profile_is_printed_before_anything_is_asked() {
    let fixture = Fixture::new();
    let run = fixture.setup("nvidia", "\n");

    assert!(run.out.contains("Detected hardware profile:"));
    assert!(run.out.contains("distro : Arch Linux"));
    assert!(run.out.contains("desktop: KDE (wayland)"));
    assert!(run.out.contains("GPU    : NVIDIA GPU [nvidia] 595.71.05"));
    assert!(run.out.contains("helpers: gamemode=no mangohud=no"));
}

// -------------------------------------------------------- non-interactive

#[test]
fn a_vendor_flag_writes_without_prompting() {
    let fixture = Fixture::new();
    let run = fixture.run(&["--gpu-vendor", "nvidia"]);

    assert_eq!(run.code, 0);
    assert!(!run.out.contains("[Y/n]"), "got {}", run.out);
    assert_eq!(fixture.saved_vendor(), "nvidia");
}

#[test]
fn auto_clears_the_override_rather_than_storing_a_literal() {
    let fixture = Fixture::with_override("nvidia");
    let run = fixture.run(&["--gpu-vendor", "auto"]);

    assert_eq!(run.code, 0);
    assert!(
        run.out.contains("autodetection is back in effect"),
        "got {}",
        run.out
    );
    assert_eq!(fixture.saved_vendor(), "");
}

#[test]
fn the_vendor_flag_reports_through_json_for_the_desktop_interface() {
    let fixture = Fixture::new();
    let run = fixture.run(&["--gpu-vendor", "nvidia", "--json"]);

    assert_eq!(run.code, 0);
    let result = run.result();
    assert_eq!(result["ok"], true);
    assert_eq!(result["gpu_vendor"], "nvidia");
    assert_eq!(result["config_path"], fixture.config.display().to_string());
}

#[test]
fn an_unknown_vendor_flag_is_a_usage_error() {
    let fixture = Fixture::new();
    let run = fixture.run(&["--gpu-vendor", "banana"]);
    assert_eq!(run.code, 2);
}
