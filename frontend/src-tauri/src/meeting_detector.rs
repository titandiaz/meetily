//! Detects active meetings and notifies the user with an action to start recording.
//!
//! Currently supported (macOS only):
//! - Google Meet: an open `meet.google.com/<code>` tab in a running browser
//!   (Chrome, Brave, Edge, Arc, Safari), queried via AppleScript.
//! - Slack huddles: the microphone becoming active while Slack is the
//!   frontmost application (CoreAudio + Launch Services).
//!
//! The detector is gated by the `meeting_detection_enabled` notification
//! preference and stays quiet while a recording is already in progress.
//!
//! While recording it also watches for the end of the call (gated by
//! `auto_stop_on_call_end`): once another app (browser, Slack, Zoom…) has been
//! seen capturing the microphone, the call is considered over when no other
//! process has captured it for `CALL_END_GRACE`. The overlay then offers to keep
//! recording and otherwise stops the recording after `AUTO_STOP_COUNTDOWN`.

use std::collections::HashSet;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::FutureExt;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, Wry};

use crate::notifications::commands::NotificationManagerState;

const POLL_INTERVAL: Duration = Duration::from_secs(3);
const SLACK_BUNDLE_ID: &str = "com.tinyspeck.slackmacgap";
/// A mic start right after a Meet alert belongs to that same meeting
/// (e.g. clicking "Join" after the lobby was detected), so don't alert twice.
const MEET_REJOIN_COOLDOWN: Duration = Duration::from_secs(10 * 60);

/// No other app has used the mic for this long: the call has probably ended.
const CALL_END_GRACE: Duration = Duration::from_secs(30);
/// Time the "call ended" overlay gives the user to keep the recording going.
const AUTO_STOP_COUNTDOWN: Duration = Duration::from_secs(30);

/// Set by the overlay's "Keep recording" button, consumed by the poll loop.
static KEEP_RECORDING_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Bundle ids of the browsers in `BROWSERS`, used to tell whether a mic start
/// comes from a Meet tab (browser frontmost) rather than another app.
const BROWSER_BUNDLE_IDS: &[&str] = &[
    "com.google.Chrome",
    "com.brave.Browser",
    "com.microsoft.edgemac",
    "company.thebrowser.Browser",
    "com.apple.Safari",
];

/// Browsers that expose tab URLs through the same AppleScript interface.
const BROWSERS: &[&str] = &[
    "Google Chrome",
    "Brave Browser",
    "Microsoft Edge",
    "Arc",
    "Safari",
];

#[derive(Default)]
struct DetectorState {
    /// Meet codes already handled whose tabs are still open. Each code is
    /// dropped once its tab closes, so reopening the link alerts again.
    known_meet_codes: HashSet<String>,
    /// When the last Meet alert fired (see MEET_REJOIN_COOLDOWN)
    last_meet_alert: Option<Instant>,
    /// Whether the previous poll saw a recording in progress
    was_recording: bool,
    /// Whether the mic was in use on the previous poll (edge detection)
    mic_was_active: bool,
    /// Already notified during the current mic-active session
    notified_this_mic_session: bool,
    /// Call-end tracking for the recording in progress
    call: CallTracker,
}

#[derive(Default)]
struct CallTracker {
    /// Another app has captured the mic during this recording
    armed: bool,
    /// Since when no other app has captured the mic
    quiet_since: Option<Instant>,
    /// Pending auto-stop (overlay shown)
    stop_at: Option<Instant>,
}

/// Spawn the background detection loop. Call once during app setup.
pub fn start(app: AppHandle<Wry>) {
    tauri::async_runtime::spawn(async move {
        log::info!("Meeting detector started (Google Meet + Slack huddles)");

        // Debug hook: fire a fake detection shortly after startup so the
        // notification + start-recording plumbing can be tested in isolation.
        if std::env::var("MEETILY_TEST_MEETING_NOTIFICATION").is_ok() {
            let app_for_test = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                notify_meeting(
                    &app_for_test,
                    "Google Meet detected (test)",
                    "This is a test notification. Start recording?",
                );
            });
        }

        #[cfg(target_os = "macos")]
        disable_app_nap();

        let mut state = DetectorState::default();
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            // A panic inside one poll must not end the task: the detector
            // would silently stop until the app restarts.
            let poll = std::panic::AssertUnwindSafe(poll_once(&app, &mut state));
            if poll.catch_unwind().await.is_err() {
                log::error!("Meeting detector: poll panicked, resetting state and continuing");
                state = DetectorState::default();
            }
        }
    });
}

/// Opt this process out of App Nap for its whole lifetime. While the window
/// stays hidden for hours, App Nap throttles timers, so the 3 s poll can fall
/// behind by minutes and meetings go unnoticed.
#[cfg(target_os = "macos")]
fn disable_app_nap() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    // NSActivityUserInitiatedAllowingIdleSystemSleep: keeps timers running
    // without preventing the Mac from sleeping when idle.
    const OPTIONS: u64 = 0x00FF_FFFF & !(1u64 << 20);
    unsafe {
        let info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
        let reason: *mut Object = msg_send![class!(NSString),
            stringWithUTF8String: b"Detecting meetings to offer recording\0".as_ptr()];
        let activity: *mut Object = msg_send![info, beginActivityWithOptions: OPTIONS reason: reason];
        // Held for the app's lifetime; the activity ends when the process exits.
        let _: *mut Object = msg_send![activity, retain];
    }
    log::info!("Meeting detector: App Nap disabled");
}

async fn poll_once(app: &AppHandle<Wry>, state: &mut DetectorState) {
    let (detection_enabled, auto_stop_enabled) = detector_preferences(app).await;

    // Don't nag while already recording; also swallow the mic edge our own
    // recording produces so stopping doesn't immediately re-trigger Slack detection.
    if crate::audio::recording_commands::is_recording().await {
        state.mic_was_active = true;
        state.was_recording = true;
        if auto_stop_enabled {
            track_call_end(app, &mut state.call);
        } else if state.call.stop_at.is_some() {
            cancel_auto_stop(app, &mut state.call);
        }
        return;
    }
    if state.call.stop_at.is_some() {
        // Stopped by other means while the overlay was up
        close_alert_overlay(app);
    }
    state.call = CallTracker::default();
    KEEP_RECORDING_REQUESTED.store(false, Ordering::SeqCst);

    if !detection_enabled {
        return;
    }

    // Google Meet: look at every Meet tab, not just the first one. Tabs of
    // meetings already left keep their URL, so checking only one let an old
    // tab hide a new meeting for the rest of the day.
    let meet_codes = find_meet_codes(app).await;
    state.known_meet_codes.retain(|code| meet_codes.contains(code));
    let new_code = meet_codes
        .iter()
        .find(|code| !state.known_meet_codes.contains(*code))
        .cloned();
    state.known_meet_codes.extend(meet_codes.iter().cloned());

    // Tabs already open when a recording ends belong to that meeting.
    let just_stopped_recording = std::mem::take(&mut state.was_recording);

    let mic_active = mic_in_use();
    let mic_started = mic_active && !state.mic_was_active && !state.notified_this_mic_session;

    if let (Some(code), false) = (&new_code, just_stopped_recording) {
        notify_meet(app, state, code, mic_active);
    } else if mic_started {
        let frontmost = frontmost_bundle_id();
        let meet_cooldown_over = state
            .last_meet_alert
            .map_or(true, |at| at.elapsed() >= MEET_REJOIN_COOLDOWN);

        if let (Some(code), Some(true), true) = (
            meet_codes.first(),
            frontmost.as_deref().map(|id| BROWSER_BUNDLE_IDS.contains(&id)),
            meet_cooldown_over,
        ) {
            // Rejoining a meeting whose tab stayed open (same link, e.g. a
            // daily standup): the tab isn't new, but the mic just started.
            notify_meet(app, state, code, true);
        } else if frontmost.as_deref() == Some(SLACK_BUNDLE_ID) {
            // Slack huddle: mic just became active while Slack is frontmost
            state.notified_this_mic_session = true;
            notify_meeting(
                app,
                "Slack huddle detected",
                "Looks like you joined a Slack huddle. Start recording?",
            );
        }
    }
    if !mic_active {
        state.notified_this_mic_session = false;
    }
    state.mic_was_active = mic_active;
}

// ---------------------------------------------------------------------------
// Auto-stop when the call ends
// ---------------------------------------------------------------------------

fn track_call_end(app: &AppHandle<Wry>, call: &mut CallTracker) {
    // No per-process data (macOS < 14): can't tell the call apart from our
    // own mic capture, so never auto-stop.
    let Some(others) = other_processes_capturing_mic() else {
        return;
    };

    if KEEP_RECORDING_REQUESTED.swap(false, Ordering::SeqCst) {
        log::info!("Auto-stop: user chose to keep recording; disarmed until the mic is used again");
        *call = CallTracker::default();
        close_alert_overlay(app);
        return;
    }

    if !others.is_empty() {
        if !call.armed {
            log::info!("Auto-stop: call detected (processes capturing mic: {:?})", others);
        }
        call.armed = true;
        call.quiet_since = None;
        if call.stop_at.is_some() {
            log::info!("Auto-stop: mic in use again, cancelling");
            cancel_auto_stop(app, call);
        }
        return;
    }

    if !call.armed {
        // Recording without a call app (in-person meeting, voice memo…)
        return;
    }

    let quiet_since = *call.quiet_since.get_or_insert_with(Instant::now);
    match call.stop_at {
        None if quiet_since.elapsed() >= CALL_END_GRACE => {
            log::info!("Auto-stop: no other app has used the mic for {:?}, asking before stopping", CALL_END_GRACE);
            call.stop_at = Some(Instant::now() + AUTO_STOP_COUNTDOWN);
            show_call_ended_overlay(app);
        }
        Some(stop_at) if Instant::now() >= stop_at => {
            log::info!("Auto-stop: call ended, stopping the recording");
            *call = CallTracker::default();
            close_alert_overlay(app);
            crate::tray::stop_recording_handler(app);
        }
        _ => {}
    }
}

fn cancel_auto_stop(app: &AppHandle<Wry>, call: &mut CallTracker) {
    call.stop_at = None;
    call.quiet_since = None;
    close_alert_overlay(app);
}

fn show_call_ended_overlay(app: &AppHandle<Wry>) {
    let seconds = AUTO_STOP_COUNTDOWN.as_secs();
    show_alert_overlay(
        app,
        &format!(
            "meeting-alert.html?mode=callEnded&seconds={}&title={}&body={}",
            seconds,
            percent_encode("Call ended"),
            percent_encode(&format!("Recording will stop in {} s", seconds)),
        ),
    );
}

fn notify_meet(app: &AppHandle<Wry>, state: &mut DetectorState, code: &str, mic_active: bool) {
    state.last_meet_alert = Some(Instant::now());
    if mic_active {
        state.notified_this_mic_session = true;
    }
    notify_meeting(
        app,
        "Google Meet detected",
        &format!("A Meet tab ({}) is open. Start recording?", code),
    );
}

/// (meeting detection enabled, auto-stop on call end enabled)
async fn detector_preferences(app: &AppHandle<Wry>) -> (bool, bool) {
    let Some(manager_state) = app.try_state::<NotificationManagerState<Wry>>() else {
        return (false, false);
    };
    let lock = manager_state.read().await;
    match lock.as_ref() {
        Some(manager) => {
            let prefs = manager.get_settings().await.notification_preferences;
            (prefs.meeting_detection_enabled, prefs.auto_stop_on_call_end)
        }
        None => (false, false),
    }
}

// ---------------------------------------------------------------------------
// Google Meet detection (browser tabs via AppleScript)
// ---------------------------------------------------------------------------

/// Meet codes of every open Meet tab across all running browsers, in tab order.
async fn find_meet_codes(app: &AppHandle<Wry>) -> Vec<String> {
    let mut codes = Vec::new();
    for browser in BROWSERS {
        // Never `tell` a browser that isn't running: AppleScript would launch it.
        if !app_running(browser) {
            continue;
        }
        for code in query_browser_tabs(app, browser).await {
            if !codes.contains(&code) {
                codes.push(code);
            }
        }
    }
    codes
}

fn app_running(process_name: &str) -> bool {
    Command::new("pgrep")
        .args(["-x", process_name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn query_browser_tabs(app: &AppHandle<Wry>, browser: &str) -> Vec<String> {
    let script = format!(
        r#"set urlList to {{}}
tell application "{browser}"
    repeat with w in windows
        repeat with t in tabs of w
            set end of urlList to URL of t
        end repeat
    end repeat
end tell
set AppleScript's text item delimiters to linefeed
return urlList as text"#
    );

    // Generous timeout: on the first run after (re)install macOS shows the
    // Automation consent dialog and osascript blocks until the user answers.
    // A short timeout would kill osascript under the user's cursor and the
    // request would be recorded as denied.
    let output = tokio::time::timeout(
        Duration::from_secs(45),
        tokio::process::Command::new("osascript")
            .args(["-e", &script])
            // Without this, a timed-out osascript (e.g. waiting on the
            // Automation permission dialog) stays alive and piles up.
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let Ok(Ok(output)) = output else {
        return Vec::new();
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        // -1743 = errAEEventNotPermitted: the Automation permission for this
        // browser is denied (it silently stops carrying over when the app
        // binary is replaced, since ad-hoc signatures change every build).
        // Detection would die silently otherwise, so tell the user once.
        if stderr.contains("-1743") || stderr.contains("Not authorized") {
            log::warn!(
                "Meeting detector: Automation permission denied for {}: {}",
                browser,
                stderr
            );
            notify_permission_denied_once(app, browser);
        } else {
            log::debug!(
                "Meeting detector: could not read {} tabs: {}",
                browser,
                stderr
            );
        }
        return Vec::new();
    }

    extract_meet_codes(&String::from_utf8_lossy(&output.stdout))
}

/// Emit a one-time (per app run) event so the frontend can tell the user how
/// to restore the Automation permission that Meet-tab detection depends on.
fn notify_permission_denied_once(app: &AppHandle<Wry>, browser: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static NOTIFIED: AtomicBool = AtomicBool::new(false);
    if NOTIFIED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = app.emit(
        "meeting-detection-permission-error",
        serde_json::json!({ "browser": browser }),
    );
}

/// Extract every Meet meeting code (`xxx-xxxx-xxx`) from a block of URLs.
fn extract_meet_codes(text: &str) -> Vec<String> {
    let mut codes = Vec::new();
    for chunk in text.split(|c: char| c.is_whitespace() || c == ',') {
        let Some(idx) = chunk.find("meet.google.com/") else {
            continue;
        };
        let rest = &chunk[idx + "meet.google.com/".len()..];
        let code: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let parts: Vec<&str> = code.split('-').collect();
        if parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
            && !codes.contains(&code)
        {
            codes.push(code);
        }
    }
    codes
}

// ---------------------------------------------------------------------------
// Slack huddle detection (mic in use + frontmost app)
// ---------------------------------------------------------------------------

#[repr(C)]
struct AudioObjectPropertyAddress {
    m_selector: u32,
    m_scope: u32,
    m_element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(
        in_object_id: u32,
        in_address: *const AudioObjectPropertyAddress,
        in_qualifier_data_size: u32,
        in_qualifier_data: *const std::ffi::c_void,
        io_data_size: *mut u32,
        out_data: *mut std::ffi::c_void,
    ) -> i32;

    fn AudioObjectGetPropertyDataSize(
        in_object_id: u32,
        in_address: *const AudioObjectPropertyAddress,
        in_qualifier_data_size: u32,
        in_qualifier_data: *const std::ffi::c_void,
        out_data_size: *mut u32,
    ) -> i32;
}

const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
const K_DEFAULT_INPUT_DEVICE: u32 = u32::from_be_bytes(*b"dIn ");
const K_DEVICE_IS_RUNNING_SOMEWHERE: u32 = u32::from_be_bytes(*b"gone");
const K_SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
// Per-process audio objects (macOS 14+)
const K_PROCESS_OBJECT_LIST: u32 = u32::from_be_bytes(*b"prs#");
const K_PROCESS_PID: u32 = u32::from_be_bytes(*b"ppid");
const K_PROCESS_IS_RUNNING_INPUT: u32 = u32::from_be_bytes(*b"piri");

/// PIDs of processes other than ours that are capturing audio input.
/// `None` when the per-process API is unavailable (macOS < 14).
fn other_processes_capturing_mic() -> Option<Vec<i32>> {
    fn read_u32(object: u32, selector: u32) -> Option<u32> {
        let addr = AudioObjectPropertyAddress {
            m_selector: selector,
            m_scope: K_SCOPE_GLOBAL,
            m_element: 0,
        };
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(object, &addr, 0, std::ptr::null(), &mut size, &mut value as *mut u32 as *mut _)
        };
        (status == 0).then_some(value)
    }

    let addr = AudioObjectPropertyAddress {
        m_selector: K_PROCESS_OBJECT_LIST,
        m_scope: K_SCOPE_GLOBAL,
        m_element: 0,
    };
    let mut size: u32 = 0;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(K_AUDIO_OBJECT_SYSTEM_OBJECT, &addr, 0, std::ptr::null(), &mut size)
    };
    if status != 0 {
        return None;
    }
    let mut objects = vec![0u32; size as usize / std::mem::size_of::<u32>()];
    let status = unsafe {
        AudioObjectGetPropertyData(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            objects.as_mut_ptr() as *mut _,
        )
    };
    if status != 0 {
        return None;
    }
    objects.truncate(size as usize / std::mem::size_of::<u32>());

    let own_pid = std::process::id() as i32;
    Some(
        objects
            .into_iter()
            .filter(|&object| read_u32(object, K_PROCESS_IS_RUNNING_INPUT).unwrap_or(0) != 0)
            .filter_map(|object| read_u32(object, K_PROCESS_PID).map(|pid| pid as i32))
            .filter(|&pid| pid != own_pid)
            .collect(),
    )
}

/// Whether the default input device is being used by any process.
fn mic_in_use() -> bool {
    unsafe {
        let addr = AudioObjectPropertyAddress {
            m_selector: K_DEFAULT_INPUT_DEVICE,
            m_scope: K_SCOPE_GLOBAL,
            m_element: 0,
        };
        let mut device_id: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = AudioObjectGetPropertyData(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut device_id as *mut u32 as *mut _,
        );
        if status != 0 || device_id == 0 {
            return false;
        }

        let addr = AudioObjectPropertyAddress {
            m_selector: K_DEVICE_IS_RUNNING_SOMEWHERE,
            m_scope: K_SCOPE_GLOBAL,
            m_element: 0,
        };
        let mut running: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = AudioObjectGetPropertyData(
            device_id,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut running as *mut u32 as *mut _,
        );
        status == 0 && running != 0
    }
}

/// Bundle id of the frontmost application via Launch Services (no TCC prompt).
fn frontmost_bundle_id() -> Option<String> {
    let front = Command::new("lsappinfo").arg("front").output().ok()?;
    let asn = String::from_utf8_lossy(&front.stdout).trim().to_string();
    if asn.is_empty() {
        return None;
    }
    let info = Command::new("lsappinfo")
        .args(["info", "-only", "bundleid", &asn])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&info.stdout);
    // Output looks like: "CFBundleIdentifier"="com.tinyspeck.slackmacgap"
    text.split('=')
        .last()
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Notification with "Start recording" action
// ---------------------------------------------------------------------------

fn notify_meeting(app: &AppHandle<Wry>, title: &str, body: &str) {
    log::info!("Meeting detector: {} — {}", title, body);

    // System notification through the same plugin path the rest of the app
    // uses. (mac-notification-sys with an action button was tried here and
    // crashed the app inside deprecated NSUserNotification XPC code.)
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        log::warn!("Meeting detector: failed to show system notification: {}", e);
    }

    // In-app toast with a "Start recording" action, handled by the frontend
    if let Err(e) = app.emit(
        "meeting-detected",
        serde_json::json!({ "title": title, "body": body }),
    ) {
        log::error!("Meeting detector: failed to emit meeting-detected: {}", e);
    }

    // Floating overlay window: not a system notification, so Focus/Do Not
    // Disturb and per-app notification settings can't suppress it.
    show_alert_overlay(
        app,
        &format!(
            "meeting-alert.html?title={}&body={}",
            percent_encode(title),
            percent_encode(body)
        ),
    );
}

const ALERT_WINDOW_LABEL: &str = "meeting-alert";
const ALERT_WIDTH: f64 = 480.0;
const ALERT_HEIGHT: f64 = 88.0;

fn close_alert_overlay(app: &AppHandle<Wry>) {
    if let Some(overlay) = app.get_webview_window(ALERT_WINDOW_LABEL) {
        let _ = overlay.close();
    }
}

fn show_alert_overlay(app: &AppHandle<Wry>, url: &str) {
    let app = app.clone();
    let url = url.to_string();

    // Window creation must happen on the main thread on macOS
    let result = app.clone().run_on_main_thread(move || {
        // Replace any previous overlay so the new one picks up fresh params.
        // destroy() (not close()) — close() only requests teardown, so the
        // label would still be taken when the builder runs right below and
        // the new overlay would silently fail to appear.
        if let Some(existing) = app.get_webview_window(ALERT_WINDOW_LABEL) {
            let _ = existing.destroy();
        }

        // Top-center of the primary screen
        let (x, y) = match app.primary_monitor() {
            Ok(Some(monitor)) => {
                let size = monitor.size().to_logical::<f64>(monitor.scale_factor());
                (((size.width - ALERT_WIDTH) / 2.0).max(0.0), 28.0)
            }
            _ => (400.0, 28.0),
        };

        let window = WebviewWindowBuilder::new(&app, ALERT_WINDOW_LABEL, WebviewUrl::App(url.into()))
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .resizable(false)
            .inner_size(ALERT_WIDTH, ALERT_HEIGHT)
            .position(x, y)
            .build();

        match window {
            Ok(window) => {
                // Also show over fullscreen apps (e.g. a fullscreen Meet):
                // canJoinAllSpaces (1<<0) | fullScreenAuxiliary (1<<8)
                if let Ok(ns_window) = window.ns_window() {
                    unsafe {
                        use objc::{msg_send, sel, sel_impl};
                        let ns_window = ns_window as *mut objc::runtime::Object;
                        let behavior: u64 = msg_send![ns_window, collectionBehavior];
                        let _: () = msg_send![ns_window, setCollectionBehavior: behavior | (1u64 << 0) | (1u64 << 8)];
                    }
                }
            }
            Err(e) => log::error!("Meeting detector: failed to create alert overlay: {}", e),
        }
    });

    if let Err(e) = result {
        log::error!("Meeting detector: failed to dispatch overlay to main thread: {}", e);
    }
}

/// Minimal percent-encoding for values embedded in the overlay URL query.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// Handle a button press from the overlay window: "start"/"dismiss" for a
/// detected meeting, "stop_now"/"keep_recording" for a call that ended.
pub async fn handle_alert_action(app: AppHandle<Wry>, action: String) -> Result<(), String> {
    log::info!("Meeting alert action: {}", action);

    close_alert_overlay(&app);

    if action == "keep_recording" {
        KEEP_RECORDING_REQUESTED.store(true, Ordering::SeqCst);
        return Ok(());
    }
    if action == "stop_now" {
        if crate::audio::recording_commands::is_recording().await {
            crate::tray::stop_recording_handler(&app);
        }
        return Ok(());
    }

    if action == "start" {
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.show();
            let _ = main.set_focus();
        }
        // Same path as the tray toggle: the layout listener forwards it
        // to the recording start flow.
        app.emit("request-recording-toggle", ())
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::extract_meet_codes;

    #[test]
    fn extracts_meet_code_from_urls() {
        let text = "https://mail.google.com/mail/u/0\nhttps://meet.google.com/abc-defg-hij?authuser=0\n";
        assert_eq!(extract_meet_codes(text), vec!["abc-defg-hij".to_string()]);
    }

    #[test]
    fn extracts_every_meet_tab_once() {
        let text = "https://meet.google.com/old-code-aaa\nhttps://meet.google.com/new-code-bbb\nhttps://meet.google.com/old-code-aaa";
        assert_eq!(extract_meet_codes(text), vec!["old-code-aaa".to_string(), "new-code-bbb".to_string()]);
    }

    #[test]
    fn per_process_mic_query_excludes_own_process() {
        // Available on macOS 14+ (the only target the auto-stop supports)
        let pids = super::other_processes_capturing_mic()
            .expect("per-process CoreAudio API unavailable");
        assert!(!pids.contains(&(std::process::id() as i32)));
    }

    #[test]
    fn ignores_non_meeting_meet_urls() {
        assert!(extract_meet_codes("https://meet.google.com/landing").is_empty());
        assert!(extract_meet_codes("https://meet.google.com/").is_empty());
        assert!(extract_meet_codes("https://example.com/").is_empty());
    }
}
