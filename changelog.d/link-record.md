### Command line and install

- Every conversion that writes a design now also writes `<design>.link.json` beside it, naming the source file it was converted from, the SHA-256 of the source and of the design, and the options used, so a later run can tell that the source changed. `--no-link-record` (MCP `link_record: false`, and a checkbox in the desktop app) writes none; see "Link records" in docs/command-line.md.
