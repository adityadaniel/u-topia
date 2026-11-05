//! System-wide quick-capture hotkey (FR-2.19), Cmd+Shift+Space. GPUI has no global
//! hotkeys, so on macOS this registers a Carbon hot key on the application event target
//! (delivered through the Cocoa run loop GPUI already runs). The handler only raises a
//! flag; the app polls it and opens the capture palette on the UI thread.

use std::sync::atomic::{AtomicBool, Ordering};

static FIRED: AtomicBool = AtomicBool::new(false);

/// Whether the hotkey fired since the last call.
pub fn take_fired() -> bool {
    FIRED.swap(false, Ordering::SeqCst)
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::sync::atomic::Ordering;

    type OSStatus = i32;
    type Ref = *mut c_void;

    #[repr(C)]
    struct EventHotKeyID {
        signature: u32,
        id: u32,
    }

    #[repr(C)]
    struct EventTypeSpec {
        event_class: u32,
        event_kind: u32,
    }

    type Handler = extern "C" fn(Ref, Ref, *mut c_void) -> OSStatus;

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn GetApplicationEventTarget() -> Ref;
        fn InstallEventHandler(
            target: Ref,
            handler: Handler,
            num_types: u32,
            list: *const EventTypeSpec,
            user_data: *mut c_void,
            out_ref: *mut Ref,
        ) -> OSStatus;
        fn RegisterEventHotKey(
            key_code: u32,
            modifiers: u32,
            id: EventHotKeyID,
            target: Ref,
            options: u32,
            out_ref: *mut Ref,
        ) -> OSStatus;
    }

    const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
    const EVENT_HOT_KEY_PRESSED: u32 = 5;
    const CMD_KEY: u32 = 1 << 8;
    const SHIFT_KEY: u32 = 1 << 9;
    const KVK_SPACE: u32 = 0x31;

    extern "C" fn on_hot_key(_: Ref, _: Ref, _: *mut c_void) -> OSStatus {
        super::FIRED.store(true, Ordering::SeqCst);
        0
    }

    pub fn register() -> bool {
        // SAFETY: plain Carbon calls with valid pointers to locals; the handler is a
        // `'static` function that only touches an atomic.
        unsafe {
            let target = GetApplicationEventTarget();
            let spec = EventTypeSpec {
                event_class: EVENT_CLASS_KEYBOARD,
                event_kind: EVENT_HOT_KEY_PRESSED,
            };
            let mut handler_ref: Ref = std::ptr::null_mut();
            if InstallEventHandler(
                target,
                on_hot_key,
                1,
                &spec,
                std::ptr::null_mut(),
                &mut handler_ref,
            ) != 0
            {
                return false;
            }
            let mut hot_key: Ref = std::ptr::null_mut();
            let id = EventHotKeyID {
                signature: u32::from_be_bytes(*b"TRLS"),
                id: 1,
            };
            RegisterEventHotKey(KVK_SPACE, CMD_KEY | SHIFT_KEY, id, target, 0, &mut hot_key) == 0
        }
    }
}

/// Register the hotkey; `false` where unsupported or refused (the in-app binding remains).
pub fn register() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::register()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
