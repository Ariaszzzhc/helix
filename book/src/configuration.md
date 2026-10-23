# Configuration

To override global configuration parameters, create a `config.toml` file located in your config directory:

- Linux and Mac: `~/.config/helix/config.toml`
- Windows: `%AppData%\helix\config.toml`

> 💡 You can easily open the config file by typing `:config-open` within Helix normal mode.

Example config:

```toml
theme = "onedark"

[editor]
line-number = "relative"
mouse = false

[editor.cursor-shape]
insert = "bar"
normal = "block"
select = "underline"

[editor.file-picker]
hidden = false
```

You can use a custom configuration file by specifying it with the `-c` or
`--config` command line argument, for example `hx -c path/to/custom-config.toml`.
You can reload the config file by issuing the `:config-reload` command. Alternatively, on Unix operating systems, you can reload it by sending the USR1
signal to the Helix process, such as by using the command `pkill -USR1 hx`.

Finally, you can have a `config.toml` and a `languages.toml` local to a project by putting it under a `.helix` directory in your repository.
Its settings will be merged with the configuration directory and the built-in configuration.

## JSON and JSONC (fork extension)

The main configuration and language configuration also accept JSON and JSONC.
In the configuration directory or a project's `.helix` directory, use
`config.json` / `config.jsonc` and `languages.json` / `languages.jsonc`.
Each directory selects **one file per configuration type**, in this order:
`.toml`, `.json`, `.jsonc`. Files of different formats in the same directory
are not merged; a parse error in the selected file does not fall back to
another format. If no file exists, `:config-open` and
`:config-open-workspace` continue to open a new `config.toml`.

Global and workspace files may use different formats. Workspace settings merge
with global settings as before, and local files require workspace trust for
every supported format. `:config-reload` reloads the selected files;
the global main configuration path is selected at startup, so restart Helix
when switching that file's format. `hx -c path/to/config.jsonc` explicitly
selects a main configuration regardless of the default file priority.
The filename extension selects the parser; other extensions retain TOML parsing.

JSON uses the same setting names and nested structure as TOML. For example,
`config.jsonc`:

```jsonc
{
  // Comments and trailing commas are supported in JSONC.
  "theme": "onedark",
  "editor": {
    "line-number": "relative",
    "mouse": false,
    "cursor-shape": {
      "insert": "bar",
      "normal": "block",
      "select": "underline",
    },
    "file-picker": { "hidden": false },
  },
  "keys": { "normal": { "C-s": ":write" } },
}
```

JSONC supports `//` and `/* ... */` comments and trailing commas in objects
and arrays. `.json` files use strict JSON, without comments or trailing commas.
Both formats require a top-level object. `null` is unsupported; omit optional
settings to use their defaults. [Theme files](./themes.md#json-and-jsonc-themes-fork-extension)
also support TOML, JSON and JSONC.

## Devicons (fork extension)

This fork provides optional Nerd Font icons in file and directory pickers
(including directory previews), the buffer picker, bufferline, statusline,
breadcrumbs, and completion menu. Configure them in the **top-level** `[devicons]`
table of `config.toml`:

```toml
[devicons]
enable = true
```

Your terminal must have a Nerd Font or an appropriate symbol fallback configured.
Icons are disabled by default, so existing configurations and the default UI keep
their existing behavior. Existing settings such as `editor.statusline.separator`,
`editor.whitespace.characters`, `editor.indent-guides.character`, and
`editor.soft-wrap.wrap-indicator` remain in their original locations.

The following options all belong to `[devicons]`:

| Key | Default | Description |
| --- | --- | --- |
| `enable` | `false` | Enable devicons. |
| `picker` | `true` | Show file and directory icons in file, directory, and buffer pickers and directory previews, plus kind icons in document and workspace symbol pickers. Icons do not participate in fuzzy matching. |
| `bufferline` | `true` | Show file icons in the bufferline, when the bufferline itself is enabled. |
| `breadcrumb` | `true` | Show file, directory, and symbol-kind icons in breadcrumbs and their navigation menus. Requires `editor.breadcrumb.enable`. |
| `statusline` | `true` | Show an icon before file names. If no file-name element is configured, show it before the `file-type` element instead. Existing text is preserved. |
| `completion` | `true` | Show kind icons before completion labels. Filtering and actual color swatches are preserved. |
| `color` | `true` | Use icon colors. Set to `false` to inherit the surrounding UI style. |

For example, to use icons everywhere except completions:

```toml
[devicons]
enable = true
completion = false
```

Built-in mappings cover file names, extensions, directories, and symbol/completion kinds.
Override individual entries with a glyph string or a table with optional `glyph`
and `fg` fields (`fg` accepts a hexadecimal color such as `"#ff8800"`). An empty
glyph hides that icon. Glyphs may contain multiple characters, but cannot contain
control characters or line breaks; terminal display width is used for layout.

```toml
[devicons.filename]
"Dockerfile" = "D"
"README.md" = { glyph = "R", fg = "#66bbff" }

[devicons.extension]
rs = { fg = "#ff8800" } # retain the built-in Rust glyph
"blade.php" = "B"
myformat = { glyph = "M", fg = "#88cc66" }

[devicons.directory]
assets = "A"
default = "d" # fallback for directories without a specific mapping

[devicons.kind]
function = "f"
struct = { glyph = "S", fg = "#bb88ff" }
```

Exact file names take precedence over extensions. Compound extensions use the
longest recognized suffix (`blade.php` before `php`). Unknown file types fall
back to a generic file icon; scratch buffers use the selected language where a
matching mapping exists. Entries are case-sensitive.

User `fg` overrides take precedence over theme styles, which take precedence over
built-in colors. Theme authors can use `ui.devicons.file`,
`ui.devicons.directory`, and `ui.devicons.kind` (or a more specific scope such as
`ui.devicons.kind.function`) in the existing theme file. No theme changes are
required. `color = false` disables icon styling, including these overrides;
language-server color swatches continue to show the actual color.

`:config-reload` applies changes to the bufferline, statusline, and breadcrumb row immediately.
Reopen a picker, breadcrumb navigation menu, or completion menu to use the refreshed configuration and theme.
Trusted project-local `.helix/config.toml` settings merge with global settings,
including individual fields of icon overrides. Disabling or removing `[devicons]`
restores the original icon-free UI without migrating any existing settings.
Completion highlighting is configured independently in
[`[editor]`](editor.md#completion-label-highlighting).

Document (`Space s`) and workspace (`Space S`) symbol pickers use syntax-theme
colors for symbol names and kinds, including the selected row. With picker icons
enabled, the kind column displays an icon before the name; otherwise it retains
the textual kind. Symbol colors work independently of devicons and completion
highlighting. Container names use the theme's comment style.
Nested document symbols display their immediate parent's name in the container
column; top-level symbols have no container. Flat LSP responses retain the
container names supplied by the server.
