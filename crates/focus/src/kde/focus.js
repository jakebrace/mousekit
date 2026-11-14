// Runs inside KWin (Plasma 6). Tells the mousekit daemon about every
// focus change. Everything is sent as a string so the D-Bus types never
// depend on how KWin converts JavaScript numbers.

const SERVICE = "io.github.jakebrace.mousekit.Focus";
const PATH = "/Focus";
const INTERFACE = "io.github.jakebrace.mousekit.Focus1";

// Windows that belong to the desktop itself. They take focus briefly (app
// launcher, run dialog, logout screen) and must not change the profile.
const SHELL_CLASSES = [
    "plasmashell", "org.kde.plasmashell", "krunner", "org.kde.krunner",
    "ksmserver-logout-greeter", "kwin_wayland", "xwaylandvideobridge",
];

function kindOf(w, cls) {
    if (w.lockScreen || cls === "kscreenlocker_greet") return "lock";
    if (w.desktopWindow || w.dock || w.popupWindow || w.specialWindow) return "shell";
    if (SHELL_CLASSES.indexOf(cls) !== -1) return "shell";
    if (w.normalWindow || w.dialog) return "app";
    return "shell";
}

// Sends the list of open application windows, one per line, for the
// settings window's "add from running windows" picker.
function sendWindows() {
    const lines = [];
    const list = workspace.windowList();
    for (let i = 0; i < list.length; i++) {
        const w = list[i];
        const cls = String(w.resourceClass);
        if (cls === "" || kindOf(w, cls.toLowerCase()) !== "app") continue;
        const title = String(w.caption).replace(/[\t\n\r]/g, " ");
        lines.push(cls + "\t" + String(w.pid) + "\t" + title);
    }
    callDBus(SERVICE, PATH, INTERFACE, "WindowsChanged", lines.join("\n"));
}

function report(w) {
    sendWindows();
    if (!w) {
        // Nothing focused at all, e.g. after closing the last window.
        callDBus(SERVICE, PATH, INTERFACE, "FocusChanged", "", "", "0", "shell");
        return;
    }
    const cls = String(w.resourceClass).toLowerCase();
    callDBus(SERVICE, PATH, INTERFACE, "FocusChanged",
        String(w.resourceClass), String(w.caption), String(w.pid), kindOf(w, cls));
}

workspace.windowActivated.connect(report);
workspace.windowAdded.connect(sendWindows);
workspace.windowRemoved.connect(sendWindows);
// Report the current window immediately, so the daemon never starts blind.
report(workspace.activeWindow);
