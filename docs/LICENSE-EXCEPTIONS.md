# License Exceptions: GUI Linking Exception

ModManager is licensed under **GPL-2.0-only** (see [LICENSE](../LICENSE)).

## Additional permission for GUI windowing/rendering libraries

As an additional permission under section 10 of the GNU General Public
License version 2, the copyright holders of ModManager grant you permission
to combine the `modman-gui` program with the following Apache-2.0-licensed
libraries (or modified versions of them, under the same license), and to
convey the resulting work:

| Crate                 | Role                                        |
| --------------------- | ------------------------------------------- |
| `winit`               | Cross-platform window creation and events   |
| `dpi`                 | DPI/scale-factor types (winit companion)    |
| `ab_glyph`            | Font loading and glyph outlines             |
| `ab_glyph_rasterizer` | Glyph rasterization                         |
| `owned_ttf_parser`    | TTF font parsing (ab_glyph dependency)      |
| `unicode-linebreak`   | Unicode line-breaking for text layout       |
| `clipboard_macos`     | macOS clipboard backend                     |
| `clipboard_wayland`   | Wayland clipboard backend                   |
| `approx`              | Float comparisons (glyph layout dependency) |
| `gethostname`         | Hostname lookup (X11 windowing dependency)  |
| `codespan-reporting`  | Shader-compiler diagnostics (naga/wgpu)     |
| `spirv`               | SPIR-V type definitions (naga/wgpu)         |
| `gl_generator`        | Build-time OpenGL binding codegen (wgpu)    |
| `khronos_api`         | Khronos XML registry data (build-time)      |
| `glutin_wgl_sys`      | Windows WGL bindings (wgpu GL fallback)     |

You must comply with the Apache License 2.0 in all respects for those
libraries themselves. If you modify ModManager, you may extend this
exception to your version, but you are not obligated to do so; if you do
not wish to, delete this exception statement from your version.

## Scope: what it deliberately does *not* cover

- This exception exists **only** because every viable native Rust
  windowing/GPU stack is built on `winit` and `wgpu`, which are
  Apache-2.0 licensed (or depend on Apache-2.0-only crates) and have no
  GPLv2-compatible substitute.
- It applies **only** to the `modman-gui` binary. The engine
  (`modman-core`), the download manager (`modman-download`), the IPC
  layer (`modman-ipc`), the protocol handler, the CLI, and the TUI link
  **zero** Apache-2.0 code - this is enforced mechanically by
  [`deny.toml`](../deny.toml), which forbids Apache-2.0 globally and excepts
  precisely the fifteen crates named above, by name.
- It is **not** a general grant to add Apache-2.0 dependencies. Any new
  Apache-2.0 crate fails `cargo deny check` unless it is a
  windowing/rendering requirement of the GUI and is added both here and
  in `deny.toml` with justification.
