<div align="center">

<h1>
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="logo_dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="logo_light.svg">
  <img alt="Helix" height="128" src="logo_light.svg">
</picture>
</h1>

[![GitHub Release](https://img.shields.io/github/v/release/helix-editor/helix)](https://github.com/helix-editor/helix/releases/latest)
[![Documentation](https://shields.io/badge/-documentation-452859)](https://docs.helix-editor.com/)
[![GitHub contributors](https://img.shields.io/github/contributors/helix-editor/helix)](https://github.com/helix-editor/helix/graphs/contributors)
[![Matrix Space](https://img.shields.io/matrix/helix-community:matrix.org)](https://matrix.to/#/#helix-community:matrix.org)

</div>

![Screenshot](./screenshot.png)

A [Kakoune](https://github.com/mawww/kakoune) / [Neovim](https://github.com/neovim/neovim) inspired editor, written in Rust.

This personal fork builds on Helix's Kakoune-inspired editing model and adds
the features listed below.

For more information, see the [website](https://helix-editor.com) or
[documentation](https://docs.helix-editor.com/).

All shortcuts/keymaps can be found [in the documentation on the website](https://docs.helix-editor.com/keymap.html).

[Troubleshooting](https://github.com/helix-editor/helix/wiki/Troubleshooting)

# Features

## Upstream baseline

Based on upstream [`helix-editor/helix`](https://github.com/helix-editor/helix)
`master` at [`ba40e5474`](https://github.com/helix-editor/helix/commit/ba40e547426b0f9896c8bdc699a4ab11f2b37dbc)
(2026-09-29), a development snapshot following the 25.07 release. The fork retains
Helix's core features:

- Vim-like modal editing
- Multiple selections
- Built-in language server support
- Smart, incremental syntax highlighting and code editing via tree-sitter

## Features added in this fork

- **[Devicons](./book/src/configuration.md#devicons-fork-extension):** Optional,
  customizable Nerd Font icons in file, directory, buffer, and symbol pickers,
  the bufferline, statusline, breadcrumbs, and completion menu. Enable with
  `devicons.enable = true`; requires a Nerd Font or suitable symbol fallback.
- **[Inline Git blame](./book/src/editor.md#editorinline-blame-section):** Show
  commit information on the cursor line or all lines, with background loading,
  caching, and a configurable format. Annotations and automatic loading are
  disabled by default.
- **[Completion highlighting](./book/src/editor.md#completion-label-highlighting):**
  Syntax-colored completion labels, formatted signatures, and optional kind
  icons. Label highlighting is enabled by default and works independently of
  devicons. Document and workspace symbol pickers also use syntax-theme colors.
- **[JSON and JSONC configuration](./book/src/configuration.md#json-and-jsonc-fork-extension):**
  Use JSON or JSONC for editor configuration, language overrides, and themes,
  alongside existing TOML support.
- **Welcome screen:** A theme-aware start screen with shortcuts and a layout
  that adapts to split panes and small views. Enabled by default; controlled by
  `editor.welcome-screen`.
- **[Interactive breadcrumbs](./book/src/editor.md#editorbreadcrumb-section):**
  Navigate file paths and enclosing symbols with the keyboard or mouse, using
  LSP document symbols with a Tree-sitter fallback. Enable with
  `editor.breadcrumb.enable = true`, then press `Space ;` to navigate.

Note: Only certain languages have indentation definitions at the moment. Check
`runtime/queries/<lang>/` for `indents.scm`.

# Installation

Install this fork from source. You need Git, Rust via rustup (the repository pins
Rust 1.90.0), and a C++14-compatible compiler such as GCC or Clang for the
Tree-sitter grammars.

```sh
git clone https://github.com/Ariaszzzhc/helix.git
cd helix
cargo install --path helix-term --locked
```

This installs `hx` into Cargo's bin directory (normally `~/.cargo/bin`) and
fetches and builds the grammars into the checkout's `runtime` directory. Ensure
Cargo's bin directory is on your `PATH`.

On Linux and macOS, point Helix to this checkout's runtime, then check the
installation:

```sh
export HELIX_RUNTIME="$PWD/runtime"
hx --health
```

Keep the checkout and add the export to your shell configuration using the
absolute path to its `runtime` directory. See the
[source installation guide](./book/src/building-from-source.md#configuring-helixs-runtime-files)
for Windows runtime setup and other installation options.

# Contributing

Contributing guidelines can be found [here](./docs/CONTRIBUTING.md).

# Getting help

Your question might already be answered on the [FAQ](https://github.com/helix-editor/helix/wiki/FAQ).

Discuss the project on the community [Matrix Space](https://matrix.to/#/#helix-community:matrix.org) (make sure to join `#helix-editor:matrix.org` if you're on a client that doesn't support Matrix Spaces yet).

# Credits

Thanks to [@jakenvac](https://github.com/jakenvac) for designing the logo!
