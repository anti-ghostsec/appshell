# appshell

Windows and Android builds of [Folio](https://github.com/anti-ghostsec/folio) (a private text editor)
and [Leaflet](https://github.com/anti-ghostsec/leaflet) (PDFs and documents: read, fill in, highlight,
reorder, join, edit and save).

Both apps keep their own HTML interface (`ui/` in each app's repository, at the commit pinned in
`apps/`). What runs around it is here:

- `formats/` — Leaflet's document formats (DOCX, ODT, RTF, text), a Rust port of `leaflet_formats.py`,
  checked against the Python on the same files.
- `core/` — what both apps' windows do for the page: which file the page may read or write, the
  checks on what it sends, settings. No platform code.
- `windows/` — the Windows shell (WebView2).
- `android/` — the Android shell (Android System WebView).

The Linux apps are unchanged; they keep their Python/GTK shells.

## Privacy

Nothing in either app reaches the network. On Android the apps don't have the internet permission at
all. Open files, notes and tokens live in memory and are gone when the app closes; settings are the
only thing written on their own.
