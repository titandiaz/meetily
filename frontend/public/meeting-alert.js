// Floating meeting-alert overlay logic. Loaded only inside the Tauri
// "meeting-alert" window (see src-tauri/src/meeting_detector.rs).
//
// Modes:
// - default: a meeting was detected -> "Start recording" / "Dismiss",
//   auto-dismissed after AUTO_DISMISS_MS.
// - callEnded: the recorded call ended -> "Stop now" / "Keep recording" with a
//   countdown. The backend owns the deadline and stops the recording itself,
//   so this page only displays it.
(function () {
    var AUTO_DISMISS_MS = 30000;

    var params = new URLSearchParams(window.location.search);
    var mode = params.get('mode');
    var title = params.get('title');
    var body = params.get('body');
    if (title) document.getElementById('title').textContent = title;
    if (body) document.getElementById('body').textContent = body;

    function act(action) {
        try {
            window.__TAURI__.core.invoke('meeting_alert_action', { action: action });
        } catch (e) {
            console.error('meeting-alert: invoke failed', e);
        }
    }

    var primary = document.getElementById('start');
    var secondary = document.getElementById('dismiss');
    var bar = document.getElementById('bar');

    if (mode === 'callEnded') {
        var seconds = parseInt(params.get('seconds'), 10) || 30;
        var endsAt = Date.now() + seconds * 1000;

        document.querySelector('.icon').textContent = '⏹️';
        primary.textContent = 'Stop now';
        secondary.textContent = 'Keep recording';
        primary.addEventListener('click', function () { act('stop_now'); });
        secondary.addEventListener('click', function () { act('keep_recording'); });

        var bodyEl = document.getElementById('body');
        var tick = function () {
            var left = Math.max(0, Math.ceil((endsAt - Date.now()) / 1000));
            bodyEl.textContent = 'Recording stops in ' + left + ' s';
        };
        tick();
        setInterval(tick, 1000);
        bar.style.animationDuration = seconds * 1000 + 'ms';
        return;
    }

    primary.addEventListener('click', function () { act('start'); });
    secondary.addEventListener('click', function () { act('dismiss'); });

    bar.style.animationDuration = AUTO_DISMISS_MS + 'ms';
    setTimeout(function () { act('dismiss'); }, AUTO_DISMISS_MS);
})();
