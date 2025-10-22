// This module is not public

// TODO: improve feature selection (make sure that there is always exactly one implementation, or enable dynamic backend selection)
// TODO: allow to disable build dependency on ALSA

#[cfg(all(
    feature = "bluetooth",
    any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "ios",
        target_os = "android"
    )
))]
mod bluetooth;
#[cfg(all(
    feature = "bluetooth",
    any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "ios",
        target_os = "android"
    )
))]
pub use self::bluetooth::*;

#[cfg(all(
    target_os = "windows",
    not(feature = "winrt"),
    not(feature = "bluetooth")
))]
mod winmm;
#[cfg(all(
    target_os = "windows",
    not(feature = "winrt"),
    not(feature = "bluetooth")
))]
pub use self::winmm::*;

#[cfg(all(target_os = "windows", feature = "winrt", not(feature = "bluetooth")))]
mod winrt;
#[cfg(all(target_os = "windows", feature = "winrt", not(feature = "bluetooth")))]
pub use self::winrt::*;

#[cfg(all(target_os = "macos", not(feature = "jack"), not(feature = "bluetooth")))]
mod coremidi;
#[cfg(all(target_os = "macos", not(feature = "jack"), not(feature = "bluetooth")))]
pub use self::coremidi::*;

#[cfg(all(target_os = "ios", not(feature = "jack"), not(feature = "bluetooth")))]
mod coremidi;
#[cfg(all(target_os = "ios", not(feature = "jack"), not(feature = "bluetooth")))]
pub use self::coremidi::*;

#[cfg(all(target_os = "linux", not(feature = "jack"), not(feature = "bluetooth")))]
mod alsa;
#[cfg(all(target_os = "linux", not(feature = "jack"), not(feature = "bluetooth")))]
pub use self::alsa::*;

#[cfg(all(
    feature = "jack",
    not(target_os = "windows"),
    not(feature = "bluetooth")
))]
mod jack;
#[cfg(all(
    feature = "jack",
    not(target_os = "windows"),
    not(feature = "bluetooth")
))]
pub use self::jack::*;

#[cfg(target_arch = "wasm32")]
mod webmidi;
#[cfg(target_arch = "wasm32")]
pub use self::webmidi::*;
