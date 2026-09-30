//! Binary entry point. All logic lives in the library so it can be
//! unit-tested without spinning up the Tauri runtime.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    researchai_lib::run()
}
