#[path = "../../../tests/support/native_home_cli.rs"]
mod fixture;
#[test]
fn native_environment_selects_capture_inputs() {
    fixture::capture(env!("CARGO_BIN_EXE_semon-claude"), "claude", "--projects");
}
