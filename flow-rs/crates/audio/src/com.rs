//! COM must be initialised on every thread that talks to the Windows audio
//! endpoint API. Threads are joined to the multithreaded apartment once and
//! leave it when they exit. A thread already in a single-threaded apartment
//! (cpal initialises its own threads that way) is fine as it is.

#[cfg(windows)]
pub(crate) fn ensure() {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

    struct Guard(bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: balances the successful CoInitializeEx on this same thread.
                unsafe { CoUninitialize() };
            }
        }
    }
    thread_local! {
        // SAFETY: CoInitializeEx has no preconditions; RPC_E_CHANGED_MODE (already in an STA) is fine.
        static GUARD: Guard = Guard(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok());
    }
    GUARD.with(|_| {});
}

#[cfg(not(windows))]
pub(crate) fn ensure() {}
