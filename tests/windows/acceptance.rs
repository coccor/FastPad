#![cfg(windows)]
mod support;

use support::acceptance::AcceptanceHarness;

#[test]
fn empty_launch_accepts_input_before_optional_work() {
    AcceptanceHarness::new().empty_launch_order();
}

#[test]
fn json_window_appears_before_file_and_lexer_finish() {
    AcceptanceHarness::new().json_launch_order();
}

#[test]
fn json_parse_is_not_called_on_startup() {
    AcceptanceHarness::new().assert_no_startup_json_parse();
}

#[test]
fn markdown_never_loads_a_browser_runtime() {
    AcceptanceHarness::new().assert_no_browser_module();
}

#[test]
fn corrupt_settings_and_recovery_do_not_block_input() {
    AcceptanceHarness::new().corrupt_state_order();
}

#[test]
fn normal_binary_has_no_network_imports() {
    AcceptanceHarness::new().assert_no_network_imports();
}

#[test]
fn app_works_without_resident_mode() {
    AcceptanceHarness::new().assert_no_resident_process();
}
