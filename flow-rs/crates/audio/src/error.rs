//! Why a microphone could not be opened, in the words the overlay shows.

use flow_core::types::CaptureErrorReason;

/// A failure to open or keep a microphone. `message` is what the user sees.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct CaptureError {
    pub reason: CaptureErrorReason,
    pub message: String,
}

impl CaptureError {
    pub fn new(reason: CaptureErrorReason, message: impl Into<String>) -> Self {
        Self { reason, message: message.into() }
    }

    pub(crate) fn not_found() -> Self {
        Self::new(CaptureErrorReason::DeviceMissing, "Microphone not found")
    }

    pub(crate) fn not_connected(device: &str) -> Self {
        Self::new(CaptureErrorReason::DeviceMissing, format!("Microphone \"{device}\" is not connected"))
    }

    pub(crate) fn permission() -> Self {
        Self::new(CaptureErrorReason::Permission, "Microphone access is blocked in Windows privacy settings")
    }

    pub(crate) fn busy() -> Self {
        Self::new(CaptureErrorReason::Busy, "Microphone is in use by another app")
    }
}

/// `E_ACCESSDENIED` as cpal prints it (`io::Error` from the HRESULT).
const ACCESS_DENIED: &str = "os error -2147024891";

/// Port of `toCaptureError`: maps a failure to open the microphone to a reason.
/// Chromium's `NotFoundError` is a missing device, `NotAllowedError` the Windows
/// privacy switch, `NotReadableError` a device held by another app.
pub(crate) fn from_cpal(err: &cpal::Error) -> CaptureError {
    use cpal::ErrorKind as K;
    let text = err.to_string();
    match err.kind() {
        K::DeviceNotAvailable => CaptureError::not_found(),
        K::PermissionDenied => CaptureError::permission(),
        K::DeviceBusy => CaptureError::busy(),
        _ if text.contains(ACCESS_DENIED) || privacy_blocked() => CaptureError::permission(),
        _ => CaptureError::new(CaptureErrorReason::Unknown, text),
    }
}

/// True when "Let desktop apps access your microphone" (or microphone access
/// as a whole) is switched off in Windows privacy settings. Consulted only after
/// an open has failed, to tell a privacy block from any other failure.
#[cfg(windows)]
pub(crate) fn privacy_blocked() -> bool {
    const STORE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
    let denied = |path: &str| read_hkcu_string(path, "Value").is_some_and(|v| v.eq_ignore_ascii_case("Deny"));
    denied(STORE) || denied(&format!(r"{STORE}\NonPackaged"))
}

#[cfg(not(windows))]
pub(crate) fn privacy_blocked() -> bool {
    false
}

#[cfg(windows)]
fn read_hkcu_string(path: &str, name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::HSTRING;
    let path = HSTRING::from(path);
    let name = HSTRING::from(name);
    let mut buf = [0u16; 64];
    let mut size = std::mem::size_of_val(&buf) as u32;
    // SAFETY: the buffer and its size describe valid writable memory.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &path,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = (size as usize / 2).min(buf.len());
    let text = String::from_utf16_lossy(&buf[..len]);
    Some(text.trim_end_matches('\0').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_cpal_kinds_to_reasons() {
        let missing = from_cpal(&cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable));
        assert_eq!(missing, CaptureError::new(CaptureErrorReason::DeviceMissing, "Microphone not found"));
        let busy = from_cpal(&cpal::Error::new(cpal::ErrorKind::DeviceBusy));
        assert_eq!(busy.reason, CaptureErrorReason::Busy);
        assert_eq!(busy.message, "Microphone is in use by another app");
        let denied = from_cpal(&cpal::Error::with_message(
            cpal::ErrorKind::BackendError,
            "Failed to build audio client: Access is denied. (os error -2147024891)",
        ));
        assert_eq!(denied.reason, CaptureErrorReason::Permission);
        assert_eq!(denied.message, "Microphone access is blocked in Windows privacy settings");
    }

    #[test]
    fn names_a_missing_labelled_device() {
        let err = CaptureError::not_connected("Microphone (Razer Seiren Mini) (1532:0531)");
        assert_eq!(err.message, "Microphone \"Microphone (Razer Seiren Mini) (1532:0531)\" is not connected");
    }
}
