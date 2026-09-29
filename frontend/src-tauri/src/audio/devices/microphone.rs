use anyhow::{anyhow, Result};
use cpal::traits::{HostTrait, DeviceTrait};
use log::{info, warn};

use super::configuration::{AudioDevice, DeviceType};

/// Get the default input (microphone) device for the system
pub fn default_input_device() -> Result<AudioDevice> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("No default input device found"))?;
    Ok(AudioDevice::new(device.name()?, DeviceType::Input))
}

/// Find the built-in microphone device (wired, stable, consistent sample rate)
///
/// Searches for MacBook/built-in microphone patterns to find the hardware
/// microphone instead of Bluetooth devices. This is useful for:
/// - Avoiding Bluetooth variable sample rate issues
/// - Getting stable wired audio for recording
/// - Fallback when Bluetooth device is default but unreliable
///
/// Returns None if no built-in microphone found
pub fn find_builtin_input_device() -> Result<Option<AudioDevice>> {
    let host = cpal::default_host();

    // Built-in microphone name patterns (platform-specific)
    let builtin_patterns = [
        // macOS patterns
        "macbook",
        "built-in microphone",
        "internal microphone",
        // Windows patterns
        "microphone array",
        "realtek",
        "conexant",
        // Linux patterns
        "hda intel",
        "built-in audio",
    ];

    // Search all input devices for built-in pattern matches
    for device in host.input_devices()? {
        if let Ok(name) = device.name() {
            let name_lower = name.to_lowercase();

            // Check if this is a built-in device
            for pattern in &builtin_patterns {
                if name_lower.contains(pattern) {
                    // Additional filter: exclude Bluetooth/wireless devices
                    if name_lower.contains("bluetooth") ||
                       name_lower.contains("airpods") ||
                       name_lower.contains("wireless") {
                        continue; // Skip Bluetooth devices
                    }

                    info!("🎤 Found built-in microphone: '{}'", name);
                    return Ok(Some(AudioDevice::new(name, DeviceType::Input)));
                }
            }
        }
    }

    warn!("⚠️ No built-in microphone found (searched {} patterns)", builtin_patterns.len());
    Ok(None)
}

/// Find a non-Bluetooth microphone to record from instead of a Bluetooth one:
/// the built-in mic when there is one, otherwise any other physical wired
/// input (USB mic, webcam…). Desktop Macs such as the Mac mini have no
/// built-in mic, so without the second step a Bluetooth headset could never
/// be replaced there.
///
/// Returns None if only Bluetooth or virtual inputs are available.
pub fn find_wired_input_device() -> Result<Option<AudioDevice>> {
    if let Some(builtin) = find_builtin_input_device()? {
        return Ok(Some(builtin));
    }

    // Loopback/virtual inputs carry no microphone signal (and device detection
    // classifies some of them as wired), so they are never a replacement.
    const VIRTUAL_PATTERNS: &[&str] = &[
        "blackhole", "vb-audio", "virtual", "loopback", "monitor", "aggregate",
        "soundflower", "zoomaudio", "zoom audio", "teams audio", "loom",
    ];

    let host = cpal::default_host();
    for device in host.input_devices()? {
        let Ok(name) = device.name() else { continue };
        let name_lower = name.to_lowercase();
        if VIRTUAL_PATTERNS.iter().any(|p| name_lower.contains(p)) {
            continue;
        }
        if crate::audio::device_detection::InputDeviceKind::detect(&name, 0, 0).is_wired() {
            info!("🎤 Found wired microphone: '{}'", name);
            return Ok(Some(AudioDevice::new(name, DeviceType::Input)));
        }
    }

    warn!("⚠️ No wired (non-Bluetooth, non-virtual) microphone found");
    Ok(None)
}