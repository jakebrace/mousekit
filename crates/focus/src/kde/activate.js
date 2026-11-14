// Runs inside KWin (Plasma 6), once per request. Brings the first window
// whose class is CLASS to the front. CLASS is defined by a line the daemon
// puts in front of this file.

(function () {
    const wanted = CLASS.toLowerCase();
    const list = workspace.windowList();
    for (let i = 0; i < list.length; i++) {
        const w = list[i];
        if (String(w.resourceClass).toLowerCase() !== wanted) continue;
        if (!w.normalWindow) continue;
        w.minimized = false;
        workspace.activeWindow = w;
        return;
    }
})();
