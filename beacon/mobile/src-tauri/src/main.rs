// Prevents an extra console window on Windows in release builds (this app is for phones; the desktop build is for testing).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    beacon_mobile_lib::run();
}
