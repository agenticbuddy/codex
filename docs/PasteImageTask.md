# Clipboard Image Pasting Tasks

The following file-level changes implement pasting images from the OS clipboard into the interactive prompt.

## `codex-rs/tui`

- **src/app.rs** – intercept bracketed paste data, parse image/file escape sequences (kitty, iTerm2, wezterm) and fall back to the OS clipboard when only plain text is received. Send a new `AppEvent::PasteImage(PathBuf)` with the saved file path.
- **src/app_event.rs** – add the `PasteImage(PathBuf)` variant to route image pastes through the event loop.
- **src/bottom_pane/chat_composer.rs** – handle `PasteImage` by inserting `![image](path)` into the input buffer and rendering a placeholder while typing.
- **src/clipboard_image.rs** (new) – define a `ClipboardImageProvider` trait and platform-specific implementations: `pbpaste`/`osascript` (macOS), `xclip` or `wl-paste` (Linux), and `powershell Get-Clipboard` (Windows). Each returns raw bytes and reports tool availability.
- **src/temp_images.rs** (new) – track temporary files created for pastes and delete them on session shutdown.
- **Cargo.toml** – add dependencies `image` for PNG detection, `tempfile` for unique paths, and `which` for locating system utilities.

## `codex-rs/common`

- **src/temp_file_tracker.rs** (new) – reusable utility that registers temp files and removes them in `Drop`; used by `tui::temp_images`.

## Plugin hook (`codex-rs/core`)

- **src/plugin.rs** – extend the plugin trait with `fn on_pasted_image(&self, image: &Path, user_prompt: &str)`.
- **src/plugin/manager.rs** – invoke `on_pasted_image` for all registered plugins when `AppEvent::PasteImage` is received.
- **src/plugin/mod.rs** – wire the new method into existing plugin registration and documentation.

## Tests

- **tui/tests/paste_image.rs** – simulate kitty/iTerm2/wezterm paste sequences and OS clipboard fallbacks, verifying temporary file creation and Markdown insertion.
- **core/tests/plugin_paste_image.rs** – ensure plugin hooks run with the correct path and user prompt.

## Documentation

- Update `docs/PasteImage.md` after implementation to note any deviations and reference the new plugin hook.
