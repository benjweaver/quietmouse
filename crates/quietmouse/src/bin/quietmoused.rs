//! The background agent: `quietmouse run` with the default config, logging to a
//! file. On Windows it has no console window. Start it with `quietmouse start`,
//! stop it with `quietmouse stop`.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    quietmouse::agent_main()
}
