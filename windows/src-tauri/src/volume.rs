// The master output volume and the mute flag, read straight from the default
// audio endpoint.
//
// Windows already exposes this through any ordinary COM consumer — there is no
// message to hook and nothing to poll the registry for. The only ceremony is
// saying COM hello on the watcher thread, which `CoInitializeEx` handles; the
// endpoint itself is created once and reused, because asking the audio stack for
// a new device every time a slider moves is wasteful.

/// What the user currently hears: a level between 0 and 1, and whether it is muted.
#[derive(Clone, Copy, PartialEq)]
pub struct Volume {
    pub level: f32,
    pub muted: bool,
}

#[cfg(windows)]
mod imp {
    use super::Volume;
    use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };

    /// A live handle on the default speakers. Dropped and rebuilt if the endpoint
    /// disappears, which is what unplugging a USB headset does.
    pub struct Endpoint {
        volume: IAudioEndpointVolume,
    }

    impl Endpoint {
        pub fn open() -> Option<Self> {
            // SAFETY: CoInitializeEx on a fresh thread, and a CoCreateInstance for a
            // well-known CLSID. Both are documented entry points; a failure here
            // only means there is no audio endpoint worth watching.
            unsafe {
                // An already-initialised apartment is not an error worth propagating —
                // RPC_E_CHANGED_MODE just means something else got here first.
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
                let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
                let volume = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()?;
                Some(Self { volume })
            }
        }

        pub fn read(&self) -> Option<Volume> {
            // SAFETY: two argument-free getters on an endpoint we already hold.
            unsafe {
                let level = self.volume.GetMasterVolumeLevelScalar().ok()?;
                let muted = self.volume.GetMute().ok()?.as_bool();
                Some(Volume { level: level.clamp(0.0, 1.0), muted })
            }
        }
    }
}

#[cfg(windows)]
pub use imp::Endpoint;

#[cfg(not(windows))]
pub struct Endpoint;

#[cfg(not(windows))]
impl Endpoint {
    pub fn open() -> Option<Self> {
        None
    }

    pub fn read(&self) -> Option<Volume> {
        None
    }
}