## AtlasDisco 0.2.1

Fixes found while testing on a real phone (Motorola, Android 16):
- Phone layout: tabs become a one-line selector, so results have room; duplicate and search controls wrap instead of going off-screen
- Android menu no longer shows desktop-only items (choose folder, schedule, new window); path hint adapted
- Duplicate auto-marking: on equal dates, keeps the file with the shortest path (the "original")
- Decimal comma in timing messages; desktop toolbar back on a single line

## AtlasDisco 0.2.0 — Android

- **Android 8+ app** (APK): same interface as the desktop, adapted to phones (folder tab, larger touch targets, system bars and on-screen keyboard)
- "All files access" request, internal storage and SD cards in Locations, reports saved to Download/AtlasDisco
- Total matches the system settings: space Android does not let any app read (installed apps, app data, caches) is shown as its own block
- With "Usage access", that block is broken down per app (app, data and cache), like the system Files app
- Informational items (no file operations) are shown in amber and excluded from searches, duplicates and largest files
- Volume size and free space on Linux/macOS/Android (statvfs)

**Downloads:** Android (.apk), Windows x64/ARM64 (.zip), macOS universal (.dmg), Linux x64/ARM64 (.deb, .tar.gz).

## AtlasDisco 0.1.0 — first public release

Disk space analyzer for Windows, Linux and macOS (interface in Brazilian Portuguese).

- Multithreaded scan with allocated size, NTFS compression, hardlink deduplication, alternate data streams and long paths
- Cloud-aware: OneDrive Files On-Demand placeholders counted separately and never downloaded
- Folder tree, details (16 columns), treemap, charts, file types, file age, largest files, owners
- Search by name, size, dates, path length, attributes and content; saved templates
- Duplicate finder (SHA-256 / MD5 / name+size) with hardlink replacement
- Bulk operations: recycle bin, delete, copy/move, regex rename, NTFS compression
- Reports: Excel, CSV, HTML, PDF, TXT, JSON, SQLite
- Snapshots, scan comparison and per-folder history
- `atlasdisco-cli` with scheduled scans and e-mail reports

**Downloads:** Windows x64/ARM64 (.zip, portable), macOS universal (.dmg), Linux x64/ARM64 (.deb, .tar.gz).
Binaries are not code-signed yet; see the README for first-launch instructions.
