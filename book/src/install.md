# Installing

patchtrack is one program, for Linux and Windows. The `check` and
`branches` commands also need `git` on your `PATH`.

- **A release**: download the archive for your system from the
  [GitHub releases](https://github.com/ryanabx/patchtrack/releases), and
  put `patchtrack` (or `patchtrack.exe`) somewhere on your `PATH`.
- **From a checkout**, with the Rust toolchain that `rust-toolchain.toml`
  names:

  ```sh
  cargo install --locked --path crates/cli
  patchtrack --version
  ```

The store is created on first use in your data directory:
`$XDG_DATA_HOME/patchtrack/store.sqlite3` (or
`~/.local/share/patchtrack/store.sqlite3`) on Linux,
`%LOCALAPPDATA%\patchtrack\store.sqlite3` on Windows. `--db` names
another. Everything patchtrack keeps is in that one file, settings
included; `patchtrack export FILE` copies it.
