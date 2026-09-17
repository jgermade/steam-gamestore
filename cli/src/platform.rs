//! Compile-time platform selection.

use gamestore_core::Platform;

/// The platform this binary was built for.
pub fn current() -> impl Platform {
    #[cfg(target_os = "linux")]
    {
        gamestore_platform_linux::LinuxPlatform
    }

    #[cfg(target_os = "windows")]
    {
        gamestore_platform_windows::WindowsPlatform
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        gamestore_core::platform::UnsupportedPlatform
    }
}
