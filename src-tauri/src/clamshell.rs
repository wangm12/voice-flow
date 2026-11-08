//! Lid-aware microphone availability using CoreAudio and IOKit metadata.

pub fn resolve_input_device(selected: &str, clamshell: &str, closed: bool) -> String {
    if closed && !clamshell.trim().is_empty() {
        clamshell.trim().to_owned()
    } else {
        selected.trim().to_owned()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LidState {
    Closed,
    Open,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeviceTransport {
    BuiltIn,
    NonBuiltIn,
    Unknown,
}

pub(crate) const CLOSED_LID_BUILTIN_INPUT_MESSAGE: &str =
    "selected input device is unavailable while the lid is closed; open the display or choose an external microphone";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CoreAudioDeviceIdentity {
    pub(crate) device_id: u32,
    pub(crate) name: String,
    pub(crate) uid: String,
    pub(crate) transport: DeviceTransport,
}

impl CoreAudioDeviceIdentity {
    fn has_stable_identity(&self) -> bool {
        self.device_id != 0 && !self.name.trim().is_empty() && !self.uid.trim().is_empty()
    }
}

pub(crate) fn selected_coreaudio_identity<'a>(
    selected_name: &str,
    inventory: &'a [CoreAudioDeviceIdentity],
    inventory_complete: bool,
) -> Option<&'a CoreAudioDeviceIdentity> {
    if !inventory_complete || selected_name.trim().is_empty() {
        return None;
    }

    // Count every exact-name device before checking identity quality. Dropping
    // a same-name entry with a missing UID could otherwise make an ambiguous
    // CoreAudio inventory look unique and misclassify the selected route.
    let mut matches = inventory
        .iter()
        .filter(|identity| identity.name == selected_name);
    let selected = matches.next()?;
    if matches.next().is_some()
        || !selected.has_stable_identity()
        || inventory
            .iter()
            .filter(|identity| identity.uid == selected.uid)
            .count()
            != 1
    {
        return None;
    }
    Some(selected)
}

pub(crate) fn should_reject_closed_lid_input(
    lid: LidState,
    identity: Option<&CoreAudioDeviceIdentity>,
) -> bool {
    lid == LidState::Closed
        && identity.is_some_and(|identity| {
            identity.has_stable_identity() && identity.transport == DeviceTransport::BuiltIn
        })
}

pub(crate) fn selection_snapshot_matches<T: PartialEq>(
    selected_before: Option<&T>,
    selected_after: Option<&T>,
) -> bool {
    matches!((selected_before, selected_after), (Some(before), Some(after)) if before == after)
}

fn should_reject_closed_lid_input_for_stable_selection(
    lid: LidState,
    identity: Option<&CoreAudioDeviceIdentity>,
    selected_before_inventory: bool,
    selected_after_inventory: bool,
) -> bool {
    selected_before_inventory
        && selected_after_inventory
        && should_reject_closed_lid_input(lid, identity)
}

pub(crate) fn ensure_input_device_available<F>(
    selected_name: &str,
    mut selection_is_current: F,
) -> Result<(), &'static str>
where
    F: FnMut() -> bool,
{
    if lid_state() != LidState::Closed {
        return Ok(());
    }

    let selected_before_inventory = selection_is_current();
    if !selected_before_inventory {
        return Ok(());
    }
    let Some(identity) = current_coreaudio_identity(selected_name) else {
        // Unknown lid-adjacent metadata is not evidence that a usable device is
        // unavailable. In particular, never reject an external route because
        // a CoreAudio property could not be read.
        return Ok(());
    };
    let selected_after_inventory = selection_is_current();
    if should_reject_closed_lid_input_for_stable_selection(
        LidState::Closed,
        Some(&identity),
        selected_before_inventory,
        selected_after_inventory,
    ) {
        Err(CLOSED_LID_BUILTIN_INPUT_MESSAGE)
    } else {
        Ok(())
    }
}

pub(crate) fn current_lid_state() -> LidState {
    lid_state()
}

pub fn lid_closed() -> bool {
    current_lid_state() == LidState::Closed
}

pub(crate) fn actual_coreaudio_identity_for_stable_selection<F>(
    device_name: &str,
    mut selection_is_current: F,
) -> Option<CoreAudioDeviceIdentity>
where
    F: FnMut() -> bool,
{
    actual_coreaudio_identity_with(
        device_name,
        &mut selection_is_current,
        current_coreaudio_identity,
    )
}

fn actual_coreaudio_identity_with<F, G>(
    device_name: &str,
    selection_is_current: &mut F,
    mut lookup_identity: G,
) -> Option<CoreAudioDeviceIdentity>
where
    F: FnMut() -> bool,
    G: FnMut(&str) -> Option<CoreAudioDeviceIdentity>,
{
    if !selection_is_current() {
        return None;
    }
    let identity = lookup_identity(device_name)?;
    selection_is_current().then_some(identity)
}

#[cfg(target_os = "macos")]
fn lid_state() -> LidState {
    macos::lid_state()
}

#[cfg(not(target_os = "macos"))]
fn lid_state() -> LidState {
    LidState::Unknown
}

#[cfg(target_os = "macos")]
fn current_coreaudio_identity(selected_name: &str) -> Option<CoreAudioDeviceIdentity> {
    macos::current_coreaudio_identity(selected_name)
}

#[cfg(not(target_os = "macos"))]
fn current_coreaudio_identity(_selected_name: &str) -> Option<CoreAudioDeviceIdentity> {
    None
}

struct ReleaseOnDrop<T, F: FnOnce(T)> {
    value: Option<T>,
    release: Option<F>,
}

impl<T, F: FnOnce(T)> Drop for ReleaseOnDrop<T, F> {
    fn drop(&mut self) {
        if let (Some(value), Some(release)) = (self.value.take(), self.release.take()) {
            release(value);
        }
    }
}

fn with_successful_property_output<T, R>(
    status: i32,
    value: Option<T>,
    release: impl FnOnce(T),
    read: impl FnOnce(&T) -> Option<R>,
) -> Option<R> {
    if status != 0 {
        // CoreAudio leaves the output undefined after a failed property read.
        return None;
    }
    let value = value?;
    let _release_on_drop = ReleaseOnDrop {
        value: Some(value),
        release: Some(release),
    };
    read(_release_on_drop.value.as_ref()?)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{
        with_successful_property_output, CoreAudioDeviceIdentity, DeviceTransport, LidState,
    };
    use std::ffi::{c_char, c_void, CStr};
    use std::mem::size_of;
    use std::ptr;

    const AUDIO_OBJECT_SYSTEM: u32 = 1;
    const AUDIO_GLOBAL_SCOPE: u32 = u32::from_be_bytes(*b"glob");
    const AUDIO_OUTPUT_SCOPE: u32 = u32::from_be_bytes(*b"outp");
    const AUDIO_MAIN_ELEMENT: u32 = 0;
    const PROPERTY_DEVICE_NAME: u32 = u32::from_be_bytes(*b"lnam");
    const PROPERTY_DEVICE_UID: u32 = u32::from_be_bytes(*b"uid ");
    const PROPERTY_TRANSPORT_TYPE: u32 = u32::from_be_bytes(*b"tran");
    const PROPERTY_HARDWARE_DEVICES: u32 = u32::from_be_bytes(*b"dev#");
    const TRANSPORT_BUILT_IN: u32 = u32::from_be_bytes(*b"bltn");
    const TRANSPORT_UNKNOWN: u32 = 0;
    const IO_REGISTRY_RECURSIVE: u32 = 1;
    const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const MAX_AUDIO_DEVICE_IDS: usize = 256;
    const MAX_CF_STRING_BYTES: usize = 1024;

    #[repr(C)]
    struct AudioObjectPropertyAddress {
        selector: u32,
        scope: u32,
        element: u32,
    }

    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFAllocatorRef = *const c_void;
    type CFTypeId = usize;

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyDataSize(
            object_id: u32,
            address: *const AudioObjectPropertyAddress,
            qualifier_data_size: u32,
            qualifier_data: *const c_void,
            data_size: *mut u32,
        ) -> i32;
        fn AudioObjectGetPropertyData(
            object_id: u32,
            address: *const AudioObjectPropertyAddress,
            qualifier_data_size: u32,
            qualifier_data: *const c_void,
            data_size: *mut u32,
            data: *mut c_void,
        ) -> i32;
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IORegistryGetRootEntry(main_port: u32) -> u32;
        fn IORegistryEntrySearchCFProperty(
            entry: u32,
            plane: *const c_char,
            key: CFStringRef,
            allocator: CFAllocatorRef,
            options: u32,
        ) -> CFTypeRef;
        fn IOObjectRelease(object: u32) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            allocator: CFAllocatorRef,
            c_string: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFGetTypeID(value: CFTypeRef) -> CFTypeId;
        fn CFStringGetTypeID() -> CFTypeId;
        fn CFBooleanGetTypeID() -> CFTypeId;
        fn CFBooleanGetValue(value: CFTypeRef) -> u8;
        fn CFStringGetCString(
            value: CFStringRef,
            buffer: *mut c_char,
            buffer_size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(value: CFTypeRef);
    }

    fn property_address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress {
            selector,
            scope,
            element: AUDIO_MAIN_ELEMENT,
        }
    }

    unsafe fn read_u32(object_id: u32, selector: u32, scope: u32) -> Option<u32> {
        let address = property_address(selector, scope);
        let mut size = size_of::<u32>() as u32;
        let mut value = 0_u32;
        let status = AudioObjectGetPropertyData(
            object_id,
            &address,
            0,
            ptr::null(),
            &mut size,
            (&mut value as *mut u32).cast(),
        );
        (status == 0 && size == size_of::<u32>() as u32).then_some(value)
    }

    unsafe fn read_cf_string(object_id: u32, selector: u32, scope: u32) -> Option<String> {
        let address = property_address(selector, scope);
        let mut size = size_of::<CFStringRef>() as u32;
        let mut value: CFStringRef = ptr::null();
        let status = AudioObjectGetPropertyData(
            object_id,
            &address,
            0,
            ptr::null(),
            &mut size,
            (&mut value as *mut CFStringRef).cast(),
        );
        with_successful_property_output(
            status,
            (!value.is_null()).then_some(value),
            |value| unsafe { CFRelease(value) },
            |value| {
                if size != size_of::<CFStringRef>() as u32
                    || CFGetTypeID(*value) != CFStringGetTypeID()
                {
                    return None;
                }
                let mut buffer = [0_u8; MAX_CF_STRING_BYTES];
                let converted = CFStringGetCString(
                    *value,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as isize,
                    CF_STRING_ENCODING_UTF8,
                );
                if converted == 0 {
                    return None;
                }
                CStr::from_ptr(buffer.as_ptr().cast())
                    .to_str()
                    .ok()
                    .map(str::to_owned)
            },
        )
    }

    unsafe fn device_identity(device_id: u32) -> Option<CoreAudioDeviceIdentity> {
        let name = read_cf_string(device_id, PROPERTY_DEVICE_NAME, AUDIO_OUTPUT_SCOPE)?;
        let uid = read_cf_string(device_id, PROPERTY_DEVICE_UID, AUDIO_GLOBAL_SCOPE)?;
        let transport = match read_u32(device_id, PROPERTY_TRANSPORT_TYPE, AUDIO_GLOBAL_SCOPE)? {
            TRANSPORT_BUILT_IN => DeviceTransport::BuiltIn,
            TRANSPORT_UNKNOWN => DeviceTransport::Unknown,
            _ => DeviceTransport::NonBuiltIn,
        };
        Some(CoreAudioDeviceIdentity {
            device_id,
            name,
            uid,
            transport,
        })
    }

    unsafe fn read_device_inventory() -> Option<(Vec<CoreAudioDeviceIdentity>, bool)> {
        let address = property_address(PROPERTY_HARDWARE_DEVICES, AUDIO_GLOBAL_SCOPE);
        let mut size = 0_u32;
        let status = AudioObjectGetPropertyDataSize(
            AUDIO_OBJECT_SYSTEM,
            &address,
            0,
            ptr::null(),
            &mut size,
        );
        if status != 0
            || size == 0
            || !size.is_multiple_of(size_of::<u32>() as u32)
            || size as usize / size_of::<u32>() > MAX_AUDIO_DEVICE_IDS
        {
            return None;
        }
        let mut ids = vec![0_u32; size as usize / size_of::<u32>()];
        let mut actual_size = size;
        let status = AudioObjectGetPropertyData(
            AUDIO_OBJECT_SYSTEM,
            &address,
            0,
            ptr::null(),
            &mut actual_size,
            ids.as_mut_ptr().cast(),
        );
        if status != 0 || actual_size != size {
            return None;
        }

        let mut complete = true;
        let identities = ids
            .into_iter()
            .filter_map(|id| match device_identity(id) {
                Some(identity) if identity.has_stable_identity() => Some(identity),
                Some(identity) => {
                    complete = false;
                    Some(identity)
                }
                None => {
                    complete = false;
                    None
                }
            })
            .collect();
        Some((identities, complete))
    }

    pub(super) fn current_coreaudio_identity(
        selected_name: &str,
    ) -> Option<CoreAudioDeviceIdentity> {
        unsafe {
            let (inventory, complete) = read_device_inventory()?;
            super::selected_coreaudio_identity(selected_name, &inventory, complete).cloned()
        }
    }

    fn read_lid_state_in_plane(plane: &'static [u8]) -> LidState {
        let Ok(key_name) = std::ffi::CString::new("AppleClamshellState") else {
            return LidState::Unknown;
        };
        let Ok(plane) = std::ffi::CStr::from_bytes_with_nul(plane) else {
            return LidState::Unknown;
        };
        unsafe {
            let root = IORegistryGetRootEntry(0);
            if root == 0 {
                return LidState::Unknown;
            }
            let key =
                CFStringCreateWithCString(ptr::null(), key_name.as_ptr(), CF_STRING_ENCODING_UTF8);
            if key.is_null() {
                let _ = IOObjectRelease(root);
                return LidState::Unknown;
            }
            let value = IORegistryEntrySearchCFProperty(
                root,
                plane.as_ptr(),
                key,
                ptr::null(),
                IO_REGISTRY_RECURSIVE,
            );
            CFRelease(key);
            let _ = IOObjectRelease(root);
            if value.is_null() {
                return LidState::Unknown;
            }
            let state = parse_cf_lid_value(value);
            CFRelease(value);
            state
        }
    }

    unsafe fn parse_cf_lid_value(value: CFTypeRef) -> LidState {
        let type_id = CFGetTypeID(value);
        if type_id == CFBooleanGetTypeID() {
            return if CFBooleanGetValue(value) != 0 {
                LidState::Closed
            } else {
                LidState::Open
            };
        }
        if type_id != CFStringGetTypeID() {
            return LidState::Unknown;
        }
        let mut buffer = [0_u8; 16];
        if CFStringGetCString(
            value,
            buffer.as_mut_ptr().cast(),
            buffer.len() as isize,
            CF_STRING_ENCODING_UTF8,
        ) == 0
        {
            return LidState::Unknown;
        }
        match CStr::from_ptr(buffer.as_ptr().cast()).to_bytes() {
            b"Yes" | b"true" => LidState::Closed,
            b"No" | b"false" => LidState::Open,
            _ => LidState::Unknown,
        }
    }

    pub(super) fn lid_state() -> LidState {
        let service = read_lid_state_in_plane(b"IOService\0");
        let device_tree = read_lid_state_in_plane(b"IODeviceTree\0");
        match (service, device_tree) {
            (LidState::Closed, LidState::Open) | (LidState::Open, LidState::Closed) => {
                LidState::Unknown
            }
            (LidState::Closed, _) | (_, LidState::Closed) => LidState::Closed,
            (LidState::Open, LidState::Open) => LidState::Open,
            _ => LidState::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        actual_coreaudio_identity_with, resolve_input_device, selected_coreaudio_identity,
        selection_snapshot_matches, should_reject_closed_lid_input,
        should_reject_closed_lid_input_for_stable_selection, with_successful_property_output,
        CoreAudioDeviceIdentity, DeviceTransport, LidState, CLOSED_LID_BUILTIN_INPUT_MESSAGE,
    };
    use std::cell::Cell;

    fn identity(
        id: u32,
        name: &str,
        uid: &str,
        transport: DeviceTransport,
    ) -> CoreAudioDeviceIdentity {
        CoreAudioDeviceIdentity {
            device_id: id,
            name: name.to_owned(),
            uid: uid.to_owned(),
            transport,
        }
    }

    #[test]
    fn closed_lid_blocks_only_known_builtin_transport_with_stable_uid() {
        let builtin = identity(
            1,
            "MacBook Microphone",
            "builtin-uid",
            DeviceTransport::BuiltIn,
        );
        let usb = identity(
            2,
            "EarPods Microphone",
            "usb-uid",
            DeviceTransport::NonBuiltIn,
        );
        let unknown = identity(
            3,
            "Unresolved Input",
            "unknown-uid",
            DeviceTransport::Unknown,
        );
        assert!(should_reject_closed_lid_input(
            LidState::Closed,
            Some(&builtin)
        ));
        assert!(!should_reject_closed_lid_input(
            LidState::Closed,
            Some(&usb)
        ));
        assert!(!should_reject_closed_lid_input(
            LidState::Closed,
            Some(&unknown)
        ));
        assert!(!should_reject_closed_lid_input(
            LidState::Unknown,
            Some(&builtin)
        ));
        assert!(!should_reject_closed_lid_input(
            LidState::Open,
            Some(&builtin)
        ));
        let mut no_uid = builtin.clone();
        no_uid.uid.clear();
        assert!(!should_reject_closed_lid_input(
            LidState::Closed,
            Some(&no_uid)
        ));
    }

    #[test]
    fn external_transport_is_allowed_even_if_its_name_mentions_builtin() {
        let external = identity(
            4,
            "Built-in Microphone over USB",
            "usb-uid",
            DeviceTransport::NonBuiltIn,
        );
        assert!(!should_reject_closed_lid_input(
            LidState::Closed,
            Some(&external)
        ));
        assert!(CLOSED_LID_BUILTIN_INPUT_MESSAGE.contains("selected input device is unavailable"));
        assert!(CLOSED_LID_BUILTIN_INPUT_MESSAGE.contains("choose an external microphone"));
    }

    #[test]
    fn default_and_explicit_selection_use_a_complete_unique_name_inventory() {
        let builtin = identity(
            1,
            "MacBook Microphone",
            "builtin-uid",
            DeviceTransport::BuiltIn,
        );
        let unrelated = identity(
            2,
            "EarPods Microphone",
            "earpods-uid",
            DeviceTransport::NonBuiltIn,
        );
        let inventory = [builtin.clone(), unrelated];
        let selected = selected_coreaudio_identity("MacBook Microphone", &inventory, true);
        assert_eq!(selected, Some(&builtin));
        assert!(selected_coreaudio_identity("Other Name", &inventory, true).is_none());
        assert!(selected_coreaudio_identity("MacBook Microphone", &inventory, false).is_none());
    }

    #[test]
    fn explicit_selection_requires_one_name_and_unique_device_uid() {
        let builtin = identity(
            10,
            "MacBook Microphone",
            "builtin-uid",
            DeviceTransport::BuiltIn,
        );
        let usb = identity(
            20,
            "EarPods Microphone",
            "usb-uid",
            DeviceTransport::NonBuiltIn,
        );
        assert_eq!(
            selected_coreaudio_identity(
                "MacBook Microphone",
                &[builtin.clone(), usb.clone()],
                true
            ),
            Some(&builtin),
        );
        assert!(selected_coreaudio_identity(
            "MacBook Microphone",
            std::slice::from_ref(&builtin),
            false
        )
        .is_none());

        let duplicate_name = identity(
            11,
            "MacBook Microphone",
            "second-uid",
            DeviceTransport::NonBuiltIn,
        );
        assert!(selected_coreaudio_identity(
            "MacBook Microphone",
            &[builtin.clone(), duplicate_name],
            true,
        )
        .is_none());

        let duplicate_uid = identity(
            12,
            "Other Device",
            "builtin-uid",
            DeviceTransport::NonBuiltIn,
        );
        assert!(
            selected_coreaudio_identity("MacBook Microphone", &[builtin, duplicate_uid], true,)
                .is_none()
        );
    }

    #[test]
    fn same_name_entry_with_unknown_uid_makes_selection_ambiguous() {
        let builtin = identity(
            10,
            "MacBook Microphone",
            "builtin-uid",
            DeviceTransport::BuiltIn,
        );
        let same_name_without_uid =
            identity(11, "MacBook Microphone", "", DeviceTransport::NonBuiltIn);
        assert!(selected_coreaudio_identity(
            "MacBook Microphone",
            &[builtin, same_name_without_uid],
            true,
        )
        .is_none());
    }

    #[test]
    fn incomplete_inventory_never_resolves_a_known_builtin_entry() {
        let builtin = identity(
            10,
            "MacBook Microphone",
            "builtin-uid",
            DeviceTransport::BuiltIn,
        );
        let unrelated_missing_uid = identity(12, "Other Device", "", DeviceTransport::NonBuiltIn);
        assert!(selected_coreaudio_identity(
            "MacBook Microphone",
            &[builtin, unrelated_missing_uid],
            false,
        )
        .is_none());
    }

    #[test]
    fn a_selection_change_while_coreaudio_is_read_prevents_rejection() {
        let builtin = identity(
            1,
            "MacBook Microphone",
            "builtin-uid",
            DeviceTransport::BuiltIn,
        );
        assert!(should_reject_closed_lid_input_for_stable_selection(
            LidState::Closed,
            Some(&builtin),
            true,
            true,
        ));
        assert!(!should_reject_closed_lid_input_for_stable_selection(
            LidState::Closed,
            Some(&builtin),
            true,
            false,
        ));
        assert!(!should_reject_closed_lid_input_for_stable_selection(
            LidState::Closed,
            Some(&builtin),
            false,
            true,
        ));
    }

    #[test]
    fn selection_snapshots_use_public_equality_and_require_both_observations() {
        #[derive(PartialEq, Eq)]
        struct DeviceSnapshot {
            id: u32,
            name: &'static str,
        }

        let original = DeviceSnapshot {
            id: 1,
            name: "MacBook Microphone",
        };
        let same = DeviceSnapshot {
            id: 1,
            name: "MacBook Microphone",
        };
        let same_name_different_id = DeviceSnapshot {
            id: 2,
            name: "MacBook Microphone",
        };
        assert!(selection_snapshot_matches(Some(&original), Some(&same)));
        assert!(!selection_snapshot_matches(
            Some(&original),
            Some(&same_name_different_id)
        ));
        assert!(!selection_snapshot_matches(Some(&original), None));
        assert!(!selection_snapshot_matches(None, Some(&original)));
    }

    #[test]
    fn actual_identity_requires_stable_opened_selection_around_lookup() {
        let identity = CoreAudioDeviceIdentity {
            device_id: 7,
            name: "USB Mic".into(),
            uid: "usb:stable-uid".into(),
            transport: DeviceTransport::NonBuiltIn,
        };
        let observations = Cell::new(0);
        let captured = actual_coreaudio_identity_with(
            "USB Mic",
            &mut || {
                observations.set(observations.get() + 1);
                observations.get() == 1 || observations.get() == 2
            },
            |_| Some(identity.clone()),
        );
        assert_eq!(captured, Some(identity.clone()));

        let observations = Cell::new(0);
        let replaced_same_name = actual_coreaudio_identity_with(
            "USB Mic",
            &mut || {
                observations.set(observations.get() + 1);
                observations.get() == 1
            },
            |_| Some(identity.clone()),
        );
        assert_eq!(replaced_same_name, None);
    }

    #[test]
    fn successful_nonnull_property_outputs_release_once_even_when_reading_fails() {
        let release_count = Cell::new(0);
        let value = with_successful_property_output(
            0,
            Some(7_u32),
            |_| release_count.set(release_count.get() + 1),
            |value| Some(*value * 2),
        );
        assert_eq!(value, Some(14));
        assert_eq!(release_count.get(), 1);

        let failed_conversion = with_successful_property_output(
            0,
            Some(9_u32),
            |_| release_count.set(release_count.get() + 1),
            |_| None::<u32>,
        );
        assert_eq!(failed_conversion, None);
        assert_eq!(release_count.get(), 2);
    }

    #[test]
    fn failed_or_null_property_outputs_are_not_released() {
        let release_count = Cell::new(0);
        let failed_status = with_successful_property_output(
            -1,
            Some(7_u32),
            |_| release_count.set(release_count.get() + 1),
            |value| Some(*value),
        );
        let null_output = with_successful_property_output(
            0,
            None::<u32>,
            |_| release_count.set(release_count.get() + 1),
            |value| Some(*value),
        );
        assert_eq!(failed_status, None);
        assert_eq!(null_output, None);
        assert_eq!(release_count.get(), 0);
    }

    #[test]
    fn configured_clamshell_override_is_applied_before_transport_guard() {
        let selected = resolve_input_device("MacBook Microphone", "EarPods Microphone", true);
        assert_eq!(selected, "EarPods Microphone");
        let earpods = identity(2, &selected, "earpods-uid", DeviceTransport::NonBuiltIn);
        assert!(!should_reject_closed_lid_input(
            LidState::Closed,
            Some(&earpods)
        ));

        let no_override = resolve_input_device("MacBook Microphone", " ", true);
        assert_eq!(no_override, "MacBook Microphone");
        let builtin = identity(1, &no_override, "builtin-uid", DeviceTransport::BuiltIn);
        assert!(should_reject_closed_lid_input(
            LidState::Closed,
            Some(&builtin)
        ));
    }

    #[test]
    fn process_id_does_not_participate_in_audio_device_identity() {
        let first = identity(
            1,
            "MacBook Microphone",
            "stable-uid",
            DeviceTransport::BuiltIn,
        );
        let same_device_after_restart = identity(
            99,
            "MacBook Microphone",
            "stable-uid",
            DeviceTransport::BuiltIn,
        );
        assert_eq!(first.uid, same_device_after_restart.uid);
        assert!(should_reject_closed_lid_input(
            LidState::Closed,
            Some(&same_device_after_restart)
        ));
    }
}
