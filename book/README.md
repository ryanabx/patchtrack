# The book

The documentation site, built with [mdBook](https://rust-lang.github.io/mdBook/).

Hand-written pages are in `src/`. The command pages, the settings
reference and `src/SUMMARY.md` are generated from the code, so each
command's page is its `--help` text and a new command gets a page without
anyone writing one. They are not in git.

```sh
cargo run -q -p patchtrack -- book-pages book   # writes src/commands/, src/settings.md, src/SUMMARY.md
mdbook build book                               # into book/build/
```

`SUMMARY.template.md` is the table of contents; `{{commands}}` becomes
the list of commands.
