# Building the Windows plugin through GitHub Actions

GitHub's own Windows machines build the plugin, so nothing compiles on your computer.
It works the same way as the tape machine.

## 1. Put the project in a GitHub repo

1. On github.com, make a new repository (or use an existing one, such as `Them-Changes`).
2. Open the repo, click **Add file → Upload files**.
3. Drag in **both** of these from the unzipped folder, keeping their structure:
   - the `console-4000e` folder
   - the `.github` folder (it holds `workflows/build-windows.yml`)
   The workflow file has to end up at `.github/workflows/build-windows.yml` at the top of the
   repo, next to the `console-4000e` folder. If the repo already has a `build-windows.yml` from
   the tape machine, give this one another name (for example `build-console.yml`) so it does
   not replace that one.
   Hidden folders: on macOS, press Cmd+Shift+. in Finder to see `.github` before dragging.
4. Click **Commit changes**.

## 2. Let it build

The commit starts the build. Open the **Actions** tab and click the run called
**"Build 4000 E Console (Windows)"**. The first run takes roughly 5 to 10 minutes. It runs the
circuit parity tests first (EQ range and Q, filter corners and slopes, mic gain range,
compressor and bus attack times), then builds.

To rebuild later without changing anything, use **Run workflow** on that workflow's page.

## 3. Download and install

1. When the run shows a green check, open it and scroll to **Artifacts**.
2. Download **console-4000e-windows** and unzip it. Inside are `console-4000e.vst3` and
   `console-4000e.clap`.
3. Copy `console-4000e.vst3` into `C:\Program Files\Common Files\VST3\`
   (and the `.clap` into `C:\Program Files\Common Files\CLAP\` if your DAW uses CLAP), or run
   `install.ps1` from the unzipped folder.
4. Rescan plugins in your DAW and look for **4000 E Console** by **Them Changes**.

## Notes

- The plugin is unsigned, the same as the tape machine build.
- The workflow only fires on changes inside `console-4000e/` or to the workflow file itself.
