// the app manifest registers every tauri command as an acl permission
// (allow-<command>). that is required before any capability can reference
// them, including the runtime-granted remote capabilities for the hosted
// client page (see chrome::grant_remote_access).
fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&[
                "capture_log",
                "close_app",
                "get_servers",
                "add_server",
                "clear_server",
                "open_server",
                "get_settings",
                "save_settings",
                "hk_start_capture",
                "hk_cancel_capture",
                "minimize_to_tray",
                "hide_share_pill",
                "system_audio_start",
                "system_audio_stop",
                "system_audio_get_enabled",
                "system_audio_set_enabled",
            ]),
        ),
    )
    .expect("failed to run tauri build");
}
