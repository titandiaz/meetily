/**
 * Trigger the recording start flow from anywhere in the app (meeting-detected
 * toast, floating alert overlay, tray).
 *
 * The direct-start listener (useRecordingStart) only exists on the home page,
 * so events dispatched from other routes used to vanish and the user had to
 * press "Start Recording" manually. On other routes we navigate home with the
 * same auto-start flag the sidebar uses.
 */
export function triggerRecordingStart(navigate: (path: string) => void): void {
  const { pathname } = window.location;
  // In the packaged app the webview may load the home page as /index.html
  if (pathname === '/' || pathname === '/index.html') {
    window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
  } else {
    sessionStorage.setItem('autoStartRecording', 'true');
    navigate('/');
  }
}
