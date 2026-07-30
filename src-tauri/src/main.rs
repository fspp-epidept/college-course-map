// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebKitGTK 2.42+ composites through DMABUF buffer sharing, which
    // NVIDIA's proprietary driver mishandles under Wayland — the webview
    // dies at startup ("Gdk Error 71 Protocol error"). Fall back to the
    // stable non-DMABUF renderer unless the user has set the variable
    // themselves. Mirrors the guard `task dev` applies to dev runs.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: `set_var` is unsafe in edition 2024 because env mutation
        // races against concurrent reads from other threads; none have been
        // spawned yet — this is the first statement in `main`.
        #[expect(unsafe_code, reason = "env mutation before any threads exist")]
        unsafe {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }

    course_classifier_lib::run();
}
