# ADR-020: In-app update check
Status: Accepted (provisional)
Date: 2026-10-13
Owner: AND + REL
Related tickets: REL-08 (offline/privacy check)
Amends: the "no permissions" note in `AndroidManifest.xml` and README (Privacy)

## Context
Veskra is distributed as APKs from GitHub Releases and from the website
(`veskra.hairbui76.id.vn`), not through a store, so nothing tells an installed copy that a new
version exists. The owner asked for a "Check for updates" button that also installs the update
from inside the app. Until now the app declared no permissions at all, and the website said "No
internet permission".

## Decision
1. **User-initiated only.** Veskra goes online only when the user taps *Check for updates* (the
   ⋯ menu in the editor, or next to the version in the gallery). No check on start-up, no
   background job, no analytics, no identifiers beyond a `User-Agent: Veskra/<version>
   (Android <release>)` header. MASTER_PLAN §8.4 ("no hidden external network calls") holds.
2. **Source.** `https://veskra.hairbui76.id.vn/download/latest.json`, written on every website
   deploy by `scripts/site_release_bundle.py` from the newest GitHub release (pre-releases
   included): tag, release URL, date, notes, and per ABI the APK file (served next to the JSON),
   size and SHA-256 (verified against the release's `SHA256SUMS.txt` at deploy time). HTTPS only;
   asset paths must be plain file names in the same folder.
3. **Version rule.** Semantic Versioning 2.0 precedence on the tag versus `versionName`
   (`org.veskra.kit.AppVersion`); only a strictly newer release is offered.
4. **APK choice.** The first entry of `Build.SUPPORTED_ABIS` that has its own APK, else the
   universal APK.
5. **Verification before install.** Exact size, SHA-256, package name equal to the installed
   one, and a higher `versionCode` (`PackageManager.getPackageArchiveInfo`). A file failing any
   check is deleted. Android itself refuses an APK signed with a different key.
6. **Install.** `PackageInstaller` session (`USER_ACTION_REQUIRED`); the result returns to the
   single-top `MainActivity`, which shows the system confirmation. The first time, the user allows
   "Install unknown apps" for Veskra in Settings. Before the session is committed the open
   drawing is checkpointed (the normal recovery checkpoint), because the update replaces the
   running app.
7. **Permissions.** `INTERNET` and `REQUEST_INSTALL_PACKAGES`, used only by this feature. Still
   no storage permissions; backups stay disabled. Artwork, brushes and paths never leave the
   device (AGENTS.md §3).

## Consequences
* The website and README no longer claim "no internet permission"; they say the app goes online
  only when the user checks for updates.
* Copies installed with a different signing key (debug builds) cannot update in place; the
  dialog explains this (`STATUS_FAILURE_CONFLICT`).
* A compromised website could offer a different APK, but Android only installs it over Veskra if
  it is signed with the release key, which never leaves the owner's machine and CI secrets.

## Evidence
* `UpdateTest` (kit): version precedence, `latest.json` parsing, ABI choice, rejected assets,
  notes.
* Emulator (`veskra_tablet`, x86_64, API 36), 2026-10-13, release-signed build labelled
  0.1.0-alpha.3: offline → "could not reach" error; online → alpha.4 offered (16.3 MB x86_64 APK),
  downloaded, verified, "Install unknown apps" granted, confirmation cancelled (dialog returns to
  Install) then accepted → `versionName=0.1.0-alpha.4` with the same signature; repeated from the
  editor with an unsaved drawing → the recovery prompt restored it after the update. 0.1.0-alpha.5
  → "You have the latest version".
* Not run on a physical tablet.

## Review
Owner 2026-10-13 (approved option 1, in-app download and install, in the working session).
