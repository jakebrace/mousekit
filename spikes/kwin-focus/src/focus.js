// Milestone 0, spike (b): tell the spike program about every focus change.
// Runs inside KWin (Plasma 6). Everything is sent as a string so the D-Bus
// types never depend on how KWin converts JavaScript numbers.

const SERVICE = "io.github.mouseprofiles.SpikeFocus";
const PATH = "/Focus";

function kindOf(w) {
    if (w.desktopWindow) return "desktop";
    if (w.dock) return "panel";
    if (w.lockScreen) return "lockscreen";
    if (w.popupWindow) return "popup";
    if (w.dialog) return "dialog";
    if (w.specialWindow) return "special";
    if (w.normalWindow) return w.fullScreen ? "normal fullscreen" : "normal";
    return "other";
}

function report(w) {
    const sent = String(Date.now());
    if (!w) {
        callDBus(SERVICE, PATH, SERVICE, "FocusChanged", sent, "", "", "", "", "none");
        return;
    }
    callDBus(SERVICE, PATH, SERVICE, "FocusChanged", sent,
        String(w.resourceClass), String(w.desktopFileName), String(w.caption),
        String(w.pid), kindOf(w));
}

workspace.windowActivated.connect(report);
// Report the current window immediately, so the listener never starts blind.
report(workspace.activeWindow);
