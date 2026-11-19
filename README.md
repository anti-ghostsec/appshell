# appshell

Windows and Android builds of [Folio](https://github.com/anti-ghostsec/folio) (a private text editor)
and [Leaflet](https://github.com/anti-ghostsec/leaflet) (PDFs and documents: read, fill in, highlight,
reorder, join, edit and save).

Both apps keep their own HTML interface: `ui/` in each app's repository, at the commit pinned in
`apps/folio.commit` and `apps/leaflet.commit`. Everything around the interface is here. The Linux apps
are unchanged and keep their Python/GTK shells.

| | |
|---|---|
| `formats/` | Leaflet's document formats (DOCX, ODT, RTF, text), a Rust port of `leaflet_formats.py`. CI checks it against the Python on the same files: same HTML, same warnings, the same contents in every file it saves. |
| `core/` | What the apps' windows do for the page, without the platform: which file the page may reach (by token, never by path), the checks on what it sends, saving, export, settings. The rules and tests of `folio.py` and `leaflet.py`. |
| `windows/` | The Windows shell: one `.exe` per app, the interface built in, shown in WebView2. |
| `android/` | The Android shell: Kotlin around the Android System WebView, the core through JNI (`android/ffi`). |
| `tools/` | The Python comparison, the HTML entity table generator, the self-test scripts. |

## Privacy

Neither app reaches the network, and both forget what they showed when they close.

- **Android**: the apps don't have the internet permission, so Android refuses them every
  connection. Files come only from the system's file picker or another app's "Open with" or "Share",
  with no lasting access: once the app closes, it can't reach them again. The WebView keeps no storage,
  cookies or cache (cleared when a window closes, and again whenever the app starts). Nothing goes into
  backups or to a new phone, and the recent-apps screen shows no picture of the window. WebView usage
  statistics and Safe Browsing lookups are off. There's no list of recent files on Android.
- **Windows**: WebView2 runs InPrivate, with its profile in a private temporary folder that is deleted
  when the app closes (or, after a crash, the next time it starts). Every request the page could make
  goes either to the app itself or to a closed port (`127.0.0.1:9`, with no exceptions, not even this
  computer). SmartScreen, autofill, password saving, downloads, browser shortcuts, devtools and every
  permission are off. Settings (and, if you switch it on in Leaflet, the recent-files list) are kept in
  `%LOCALAPPDATA%\Folio` and `%LOCALAPPDATA%\Leaflet`, not in the roaming profile.
  One thing the app can't control: WebView2 is part of Windows, and its updater and diagnostic data
  follow Windows' own settings.
- Both: the page never names a file. Links in a document open only after a click and a yes, in the
  system's browser or mail program.

## Phones

On Android the interfaces get a small stylesheet of their own (`android/shell/src/main/assets/appshell`):
bigger touch targets, Leaflet's page list floating over the page, documents fitting the screen's width.
It applies only there; the Linux and Windows apps never load it.

## Building

Builds and tests run on GitHub Actions (`.github/workflows/ci.yml`); a `v*` tag makes a release
(`release.yml`) with Windows zips and Android APKs.

By hand:

```sh
# the interfaces, at the pinned commits
for app in folio leaflet; do
  git clone https://github.com/anti-ghostsec/$app build/$app
  git -C build/$app checkout "$(tr -d '[:space:]' < apps/$app.commit)"
done

# formats and core, with the comparison against the Python
python3 tools/formats_oracle.py build/leaflet build/oracle
ORACLE_DIR=$PWD/build/oracle cargo test -p leaflet-formats -p appshell-core

# Windows (on Windows)
APPSHELL_FOLIO_UI=$PWD/build/folio/ui APPSHELL_LEAFLET_UI=$PWD/build/leaflet/ui cargo build --release -p appshell-windows

# Android: the Rust library for three ABIs, then the APKs
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 --platform 26 -o android/shell/src/main/jniLibs build --release -p appshell-android
APPSHELL_FOLIO_UI=$PWD/build/folio/ui APPSHELL_LEAFLET_UI=$PWD/build/leaflet/ui gradle -p android assembleRelease
```

A local build like this signs the APKs with Android's debug key. Releases are signed with the release
key: `release.yml` writes it from the repository's secrets (listed at the top of that file) and refuses
to publish without it, or with APKs signed by a debug key. The key is the maintainer's to make and keep,
and never goes in this repository. To sign a local build with it, set `APPSHELL_KEYSTORE` (its path),
`APPSHELL_KEYSTORE_PASSWORD`, `APPSHELL_KEY_ALIAS` and `APPSHELL_KEY_PASSWORD`.

## Testing

- `formats`: unit tests, the comparison with `leaflet_formats.py` on Leaflet's own fixtures plus extra
  documents (`tools/formats_oracle.py`), and damaged copies of all of them, which must never panic;
  files nested thousands of levels deep are refused with an error instead of overflowing the stack.
- `core`: the page-may-not tests of both Linux apps (paths aren't tokens, a closed document can't be
  written, export goes where the ticket says, links only after a yes, ...).
- Windows and Android: the real apps on CI, driven through their pages by `tools/selftest/*.js`: Folio
  opens a file and saves an edit; Leaflet edits, saves and exports a .docx, and shows, turns and saves a
  PDF. On Windows a test build (`--features selftest`) answers the file dialogs; on Android the
  instrumented tests in `android/*/src/androidTest` do it, in debug builds only.

## Known limits

- Android: saving writes the file in place through its provider (Android has no safe-replace for those).
  A file another app opened read-only can only be saved with Save as.
- Android: Export as PDF prints the page through a route Android doesn't publish; if a phone closes it,
  Leaflet says so and Print (then "Save as PDF") does the job, without Leaflet's metadata cleaning.
- Windows: printing a PDF opens it in WebView2's own PDF viewer with the print dialog on top.
