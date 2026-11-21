# Silent print sidecar

Nearby Print prints through a bundled CLI tool so that nothing ever opens a
PDF viewer or a print dialog.

## What is in here

```
SumatraPDF-x86_64-pc-windows-msvc.exe   the real tool (SumatraPDF 3.6.1, portable)
```

**SumatraPDF (portable)**, not a third-party `PDFtoPrinter` build, because it
is the only option that needs nothing installed:

* single executable, statically linked — its import table lists nothing but
  operating-system DLLs (`KERNEL32`, `WINSPOOL.DRV`, `GDI32`, …), so there is
  **no Visual C++ redistributable**, no runtime installer, no PDF reader.
* prints headlessly with `-silent -exit-when-done` — no window, nothing to
  click, and it returns as soon as the job is spooled.
* free, open source, no registry writes in portable mode.

The `-x86_64-pc-windows-msvc` suffix is the Tauri `externalBin` naming
convention (`<name>-<target-triple>.exe`). `tauri.conf.json` declares
`bundle.externalBin: ["bin/SumatraPDF"]`, so `tauri-build` copies this file
next to the application as `SumatraPDF.exe` on every build — into
`target/release/` and into the installer's files. If this file is missing the
build fails immediately rather than shipping an app that cannot print.

Nearby Print invokes it as:

```
SumatraPDF.exe -print-to "Printer Name" -silent -exit-when-done "C:\path\job.pdf"
```

`-print-to-default` is substituted when no queue is selected. `-exit-when-done`
is not optional: without it the viewer stays open and the call never returns.

Argument order lives in one place — `sidecar_args()` in
`src-tauri/src/main.rs`. If a different tool is ever bundled, change
`SIDECAR_EXE`, `sidecar_args()` and this file together.

## Updating SumatraPDF

1. Download the portable zip from <https://www.sumatrapdfreader.org/download-free-pdf-viewer>
   (the "Portable" link — a single `.exe`).
2. Drop `SumatraPDF.exe` into this folder as
   `SumatraPDF-x86_64-pc-windows-msvc.exe`, replacing the old one.
3. Rebuild (`npm run tauri build`).

There is no placeholder anymore: the real binary ships with the source, so a
build that cannot find it is a configuration error worth failing on.
