# Heritage-HelpDesk changes

Each build's GitHub release uses the first section below as its description, and the Heritage
server shows it on the console's Custom client tab. Add new changes to the top section before
pushing. Versions are year.month.build, where build is the GitHub Actions run number.

## Next release

- "Check for updates" button on the About page (End user and Full installs): asks the Heritage
  server for the newest build, which then installs within a few minutes, once nobody is
  connected, the same as when IT clicks Update in the console.
- RustDesk's own update check and Auto update setting are off: a stock RustDesk from
  rustdesk.com would replace the Heritage app. Heritage builds update from the Heritage server.

## 2026.11.12 (2026-11-17)

- Every install, Technician included, reports its Heritage version to the server with its
  device info, so the console's Devices list shows which build each computer runs.

## 2026.11.11 (2026-11-17)

- Test release for updates from the Heritage server: no other changes. A PC on 2026.11.10
  should update itself to this build when an admin clicks Update in the console.

## 2026.11.10 (2026-11-17)

- Updates from the Heritage server: End user and Full installs check the server about once an
  hour and install a newer build by themselves, waiting until nobody is connected. Admins turn
  this on, or update chosen PCs now, in the console. Each update is signed by the server and
  checked before it runs. Technician installs are still updated by reinstalling.
- Heritage's own version number (year.month.build), shown on the About page next to the RustDesk
  version, and in the console's Devices list.
- Release descriptions on GitHub, from this file.

## 1.5.3 build 9 (2026-11-16)

- End user app: no ID or password shown, just "Your IT help desk can connect to this computer to
  support you."
- Technician app: gets the device password from the server when connecting, so techs never type
  it. Only signed-in techs get it, and each one is logged.

## 1.5.3 build 7 (2026-11-16)

- Installer: the printer option is hidden (RustDesk's printer only works under the RustDesk name).

## 1.5.3 build 6 (2026-11-12)

- Device info: Processes tab, like Task Manager, with End task for admins.

## 1.5.3 build 5 (2026-11-11)

- Device info: screen thumbnails, like ScreenConnect, captured with Windows' own screen copy (GDI).

## 1.5.3 build 4 (2026-11-11)

- Device info: Programs and Services tabs. Admins can uninstall programs that have a silent
  uninstaller, and stop, start or restart services. Commands are signed by the server.

## 1.5.3 build 2 (2026-11-11)

- Device info: hardware and network details are sent again when they change.

## 1.5.3 build 1 (2026-11-11)

- First Heritage build, from RustDesk 1.5.3: Device info panel (hardware, live usage, network),
  64-bit Windows only.
