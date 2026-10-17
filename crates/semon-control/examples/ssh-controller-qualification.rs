//! Offline fixture adapter for retained controller security and native oracles.
//! Cargo installs product binaries, not this explicitly built example.
#[allow(dead_code)]
#[path = "../src/bin/semon-ssh-controller.rs"]
mod controller;

fn main() -> std::io::Result<()> {
    controller::run_retained_controller()
}
