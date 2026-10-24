//! Microphones: listing them, matching stored labels, and hearing about
//! hot-plug and default-device changes (Chromium's `devicechange`).

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait};

use crate::error::CaptureError;

/// A connected microphone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Microphone {
    /// Stored in `audio.input_device`. Labels are stable across runs; ids are not
    /// guaranteed to be.
    pub label: String,
    /// The Windows endpoint id.
    pub id: String,
}

/// Chromium appends a USB `(vid:pid)` to the Windows name, e.g.
/// `Microphone (Razer Seiren Mini) (1532:0531)`. Returns the label without it.
fn strip_usb_suffix(label: &str) -> &str {
    const LEN: usize = " (1234:abcd)".len();
    if label.len() < LEN || !label.is_char_boundary(label.len() - LEN) {
        return label;
    }
    let (head, tail) = label.split_at(label.len() - LEN);
    let b = tail.as_bytes();
    let hex = |s: &[u8]| s.iter().all(u8::is_ascii_hexdigit);
    if b[0] == b' ' && b[1] == b'(' && hex(&b[2..6]) && b[6] == b':' && hex(&b[7..11]) && b[11] == b')' {
        head
    } else {
        label
    }
}

/// Chromium's label suffix for a USB device, from the instance id of the
/// controller the endpoint is connected to: `USB\VID_1532&PID_0531&MI_00\…`
/// gives `" (1532:0531)"`. Other buses get none.
fn usb_suffix(instance_id: &str) -> Option<String> {
    let id = instance_id.to_ascii_uppercase();
    let rest = id.strip_prefix("USB\\")?;
    let field = |prefix: &str| {
        let start = rest.find(prefix)? + prefix.len();
        let value = rest.get(start..start + 4)?;
        value.bytes().all(|b| b.is_ascii_hexdigit()).then(|| value.to_ascii_lowercase())
    };
    Some(format!(" ({}:{})", field("VID_")?, field("PID_")?))
}

/// True when two microphone labels name the same device. Labels written by the
/// Electron build carry Chromium's USB `(vid:pid)` suffix; the native names do
/// not, so the suffix is ignored on both sides.
pub fn same_device(a: &str, b: &str) -> bool {
    strip_usb_suffix(a) == strip_usb_suffix(b)
}

/// A key that is the same for every spelling of a device's label.
pub(crate) fn device_key(label: &str) -> String {
    strip_usb_suffix(label).to_string()
}

/// Lists the active microphones in the order Windows reports them, labelled as
/// Chromium labels them (with the USB `(vid:pid)`). Never fails; an enumeration
/// error yields an empty list.
pub fn list_microphones() -> Vec<Microphone> {
    match endpoints::capture_endpoints() {
        Ok(list) => list,
        Err(err) => {
            log::warn!(target: "capture", "could not list microphones: {err}");
            Vec::new()
        }
    }
}

/// Finds the cpal device for a stored label, or the system default for
/// `"default"` (and an empty label). The default device follows Windows when the
/// default microphone changes.
pub(crate) fn resolve(device: &str) -> Result<(cpal::Device, bool), CaptureError> {
    crate::com::ensure();
    let host = cpal::default_host();
    if device.is_empty() || device == "default" {
        return host.default_input_device().map(|d| (d, true)).ok_or_else(CaptureError::not_found);
    }
    let mics = list_microphones();
    let Some(mic) = mics.iter().find(|m| same_device(&m.label, device)) else {
        return Err(CaptureError::not_connected(device));
    };
    let inputs = host.input_devices().map_err(|e| crate::error::from_cpal(&e))?;
    for candidate in inputs {
        if candidate.id().is_ok_and(|id| id.id() == mic.id) {
            return Ok((candidate, false));
        }
    }
    Err(CaptureError::not_connected(device))
}

/// Calls back whenever a microphone or speaker is added, removed, enabled,
/// disabled, or becomes the default, like Chromium's `devicechange` event. The
/// callback runs on a system thread; it must be cheap and must not block. Several
/// calls may arrive for one change.
pub struct DeviceWatcher {
    #[cfg(windows)]
    inner: Option<endpoints::Registration>,
}

impl DeviceWatcher {
    pub fn new(on_change: Arc<dyn Fn() + Send + Sync>) -> Self {
        #[cfg(windows)]
        {
            let inner = match endpoints::Registration::new(on_change) {
                Ok(r) => Some(r),
                Err(err) => {
                    log::warn!(target: "capture", "device notifications unavailable: {err}");
                    None
                }
            };
            Self { inner }
        }
        #[cfg(not(windows))]
        {
            let _ = on_change;
            Self {}
        }
    }

    /// False when Windows refused the registration (no notifications will come).
    pub fn active(&self) -> bool {
        #[cfg(windows)]
        {
            self.inner.is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

#[cfg(windows)]
mod endpoints {
    use std::sync::Arc;

    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{
        DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow, ERole, IDeviceTopology, IMMDevice, IMMDeviceEnumerator,
        IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, eCapture,
    };
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
    use windows::core::{GUID, PCWSTR, implement};

    use super::Microphone;

    /// `PKEY_Device_FriendlyName`, e.g. "Microphone (Razer Seiren Mini)".
    const FRIENDLY_NAME: PROPERTYKEY =
        PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };
    /// `PKEY_Device_InstanceId`, e.g. `USB\VID_1532&PID_0531&MI_00\7&…`.
    const INSTANCE_ID: PROPERTYKEY =
        PROPERTYKEY { fmtid: GUID::from_u128(0x78c34fc8_104a_4aca_9ea4_524d52996e57), pid: 256 };

    /// A string property of a device, or `None`.
    ///
    /// # Safety
    /// COM must be initialised on this thread.
    unsafe fn string_property(device: &IMMDevice, key: &PROPERTYKEY) -> Option<String> {
        unsafe {
            let mut value = device.OpenPropertyStore(STGM_READ).and_then(|store| store.GetValue(key)).ok()?;
            let text = PropVariantToStringAlloc(&value).ok().map(|s| {
                let text = s.to_string().unwrap_or_default();
                CoTaskMemFree(Some(s.0 as *const _));
                text
            });
            let _ = PropVariantClear(&mut value);
            text
        }
    }

    /// Chromium's USB suffix: the instance id of the controller (KS filter) the
    /// endpoint's first connector leads to, as `CoreAudioUtil::GetAudioControllerID`.
    ///
    /// # Safety
    /// COM must be initialised on this thread.
    unsafe fn usb_suffix(enumerator: &IMMDeviceEnumerator, device: &IMMDevice) -> Option<String> {
        unsafe {
            let topology: IDeviceTopology = device.Activate(CLSCTX_ALL, None).ok()?;
            let filter = topology.GetConnector(0).ok()?.GetDeviceIdConnectedTo().ok()?;
            let node = enumerator.GetDevice(PCWSTR(filter.0));
            CoTaskMemFree(Some(filter.0 as *const _));
            super::usb_suffix(&string_property(&node.ok()?, &INSTANCE_ID)?)
        }
    }

    fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
        crate::com::ensure();
        // SAFETY: COM is initialised on this thread.
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
    }

    pub(super) fn capture_endpoints() -> windows::core::Result<Vec<Microphone>> {
        let enumerator = enumerator()?;
        // SAFETY: plain COM calls on live interfaces; strings are freed below.
        unsafe {
            let collection = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
            let count = collection.GetCount()?;
            let mut out = Vec::with_capacity(count as usize);
            for i in 0..count {
                let Ok(device) = collection.Item(i) else { continue };
                let Ok(raw_id) = device.GetId() else { continue };
                let id = raw_id.to_string().unwrap_or_default();
                CoTaskMemFree(Some(raw_id.0 as *const _));
                let mut label = string_property(&device, &FRIENDLY_NAME).unwrap_or_default();
                // Like Chromium, a device without a name is not offered.
                if !label.is_empty() && !id.is_empty() {
                    label.extend(usb_suffix(&enumerator, &device));
                    out.push(Microphone { label, id });
                }
            }
            Ok(out)
        }
    }

    #[implement(IMMNotificationClient)]
    struct Client {
        on_change: Arc<dyn Fn() + Send + Sync>,
    }

    impl IMMNotificationClient_Impl for Client_Impl {
        fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
            (self.on_change)();
            Ok(())
        }
        fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
            (self.on_change)();
            Ok(())
        }
        fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
            (self.on_change)();
            Ok(())
        }
        fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
            // Fired once per role; the console role stands for all three.
            if role == windows::Win32::Media::Audio::eConsole {
                (self.on_change)();
            }
            Ok(())
        }
        fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
            Ok(())
        }
    }

    pub(super) struct Registration {
        enumerator: IMMDeviceEnumerator,
        client: IMMNotificationClient,
    }

    // SAFETY: the device enumerator is free-threaded and the client is an agile
    // `#[implement]` object; both may be used and released from any thread.
    unsafe impl Send for Registration {}
    unsafe impl Sync for Registration {}

    impl Registration {
        pub(super) fn new(on_change: Arc<dyn Fn() + Send + Sync>) -> windows::core::Result<Self> {
            let enumerator = enumerator()?;
            let client: IMMNotificationClient = Client { on_change }.into();
            // SAFETY: both interfaces are live.
            unsafe { enumerator.RegisterEndpointNotificationCallback(&client)? };
            Ok(Self { enumerator, client })
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            crate::com::ensure();
            // SAFETY: unregisters the client registered in `new`; waits for running callbacks.
            let _ = unsafe { self.enumerator.UnregisterEndpointNotificationCallback(&self.client) };
        }
    }
}

#[cfg(not(windows))]
mod endpoints {
    use super::Microphone;
    pub(super) fn capture_endpoints() -> Result<Vec<Microphone>, std::io::Error> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_device_ignores_the_usb_suffix() {
        let stored = "Microphone (Razer Seiren Mini) (1532:0531)";
        assert!(same_device(stored, "Microphone (Razer Seiren Mini)"));
        assert!(same_device("Microphone (Razer Seiren Mini)", stored));
        assert!(same_device(stored, stored));
        assert!(same_device("default", "default"));
        assert!(same_device("Headset (WH-1000XM4) (054C:0D58)", "Headset (WH-1000XM4)"));
    }

    #[test]
    fn same_device_keeps_names_that_only_look_like_a_suffix() {
        assert!(!same_device("Microphone (Razer Seiren Mini)", "Microphone"));
        assert!(!same_device("Microphone (Realtek(R) Audio)", "Microphone"));
        assert!(!same_device("Mic (12:34)", "Mic"));
        assert!(!same_device("Mic (wxyz:0531)", "Mic"));
        assert!(!same_device("Microphone (A)", "Microphone (B)"));
        assert!(!same_device("default", "Microphone (A)"));
        assert_eq!(strip_usb_suffix("Mikrofon (Blue Yeti) (046d:0ab7)"), "Mikrofon (Blue Yeti)");
        assert_eq!(strip_usb_suffix("Ünïcode (1234:5678)"), "Ünïcode");
        assert_eq!(strip_usb_suffix("é(1234:5678)"), "é(1234:5678)");
    }

    #[test]
    fn usb_suffix_matches_chromium() {
        assert_eq!(usb_suffix(r"USB\VID_1532&PID_0531&MI_00\7&2A8B0F1&0&0000").as_deref(), Some(" (1532:0531)"));
        assert_eq!(usb_suffix(r"usb\vid_046D&pid_0AB7\5&1").as_deref(), Some(" (046d:0ab7)"));
        assert_eq!(usb_suffix(r"HDAUDIO\FUNC_01&VEN_10EC&DEV_0897\4&1"), None);
        assert_eq!(usb_suffix(r"BTHENUM\{0000111e}_VID&0002054c_PID&0d58\8&1"), None);
        assert_eq!(usb_suffix(r"USB\VID_12&PID_0531"), None);
    }

    #[test]
    fn lists_microphones_with_labels_and_ids() {
        // Works with or without hardware: every listed device has both fields.
        for mic in list_microphones() {
            assert!(!mic.label.is_empty());
            assert!(!mic.id.is_empty());
        }
    }
}
