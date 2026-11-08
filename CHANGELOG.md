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
