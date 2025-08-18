# Clipboard Image Pasting Design

## Goal

Enable pasting an image from the OS clipboard into the interactive Codex prompt with standard paste shortcuts (Ctrl+V/Cmd+V).

## Overall flow

1. Intercept paste event in the terminal and check for embedded image data.
2. If needed, read image bytes from the OS clipboard.
3. Save the image to a temporary file and insert a reference into the prompt.
4. Clean up temporary files and expose plugin hooks.

## Step-by-step plan

### 1. Intercept paste

- Enable bracketed paste mode in all Codex terminal sessions.
- Inspect the incoming data:
  - If the terminal sends image/file escape sequences (kitty, iTerm2, wezterm), parse them.
  - If only plain text arrives, fall back to reading the OS clipboard.
- **Pitfalls**: inconsistent protocol support.
- **Checks**: compatibility matrix and tests for escape-sequence parsing.

### 2. Read image from the OS clipboard

- **macOS**: use `osascript` or `pbpaste -Prefer tiff` and convert to PNG.
- **Linux (X11)**: attempt `xclip -selection clipboard -t image/png -o`; for Wayland use `wl-paste`.
- **Windows**: `powershell Get-Clipboard -Format Image` and convert to PNG.
- **Pitfalls**: missing utilities, unexpected formats.
- **Checks**: verify tool availability on startup and cover with a `ClipboardImageProvider` abstraction and unit tests.

### 3. Save and reference

- Write the image to a system temporary directory with a unique name.
- Insert a Markdown reference `![image](path/to/file.png)` into the prompt.
- Enforce a size limit (e.g., 5 MB) but do not compress or transform the image.
- **Pitfalls**: oversized files, unwritable temp directory.
- **Checks**: log save operations and test unique name generation.

### 4. Cleanup

- Remove the temporary file after the message is processed or when the session ends.
- Provide a utility that tracks created files and deletes them on shutdown.

### 5. Plugin API

- Expose a hook `on_pasted_image(image_path, user_prompt)` for third‑party plugins.
- The hook receives the image path and the user's original request so plugins can schedule custom processing.
- **Pitfalls**: untrusted plugin execution.
- **Checks**: document sandbox expectations and add integration tests for plugin callbacks.

## Quality assurance

- **Automated tests**:
  - escape sequence recognition across terminals;
  - clipboard extraction on all supported OSes;
  - error handling (missing tools, large file, empty buffer).
- **Manual checks**:
  - paste images in popular terminals (Terminal.app, iTerm2, Kitty, Windows Terminal, GNOME Terminal);
  - paste in ssh sessions and with missing utilities to verify error messages.
