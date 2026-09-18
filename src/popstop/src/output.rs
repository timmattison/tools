//! The output unit (macOS only).
//!
//! [`OutputUnit`] opens the default output unit of Core Audio and plays the
//! samples that a [`Render`] writes, on the real-time audio thread. The
//! default output unit follows the default output device, so a change of the
//! device needs no listener.
//!
//! The stream is linear PCM, `f32`, packed and interleaved, with two
//! channels.

use std::ffi::c_void;
use std::fmt;
use std::ptr::{self, NonNull};
use std::slice;

use objc2_audio_toolbox::{
    kAudioUnitManufacturer_Apple, kAudioUnitProperty_SetRenderCallback,
    kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, kAudioUnitSubType_DefaultOutput,
    kAudioUnitType_Output, AURenderCallbackStruct, AudioComponentDescription,
    AudioComponentFindNext, AudioComponentInstanceDispose, AudioComponentInstanceNew,
    AudioOutputUnitStart, AudioOutputUnitStop, AudioUnit, AudioUnitInitialize, AudioUnitPropertyID,
    AudioUnitRenderActionFlags, AudioUnitSetProperty, AudioUnitUninitialize,
};
use objc2_core_audio::{
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    kAudioObjectUnknown, AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectPropertySelector,
};
use objc2_core_audio_types::{
    kAudioFormatFlagsNativeFloatPacked, kAudioFormatLinearPCM, AudioBuffer, AudioBufferList,
    AudioStreamBasicDescription, AudioTimeStamp,
};

use objc2_core_foundation::{CFRetained, CFString};

use crate::signal::SampleRate;

/// The number of channels of the stream.
const CHANNELS: u32 = 2;

/// The status of a Core Audio call. Zero is success.
type OSStatus = i32;

/// The status of a call that succeeded.
const NO_ERR: OSStatus = 0;

/// The number of bits in one sample.
const BITS_PER_SAMPLE: u32 = 32;

/// The number of bytes in one sample.
const BYTES_PER_SAMPLE: u32 = BITS_PER_SAMPLE / 8;

/// The number of bytes in one frame, which holds one sample for each channel.
const BYTES_PER_FRAME: u32 = BYTES_PER_SAMPLE * CHANNELS;

// The stream carries `f32` samples, so a sample must have the size of `f32`.
const _: () = assert!(size_of::<f32>() == BYTES_PER_SAMPLE as usize);

/// The element of the output unit that plays to the device. The renderer
/// feeds its input scope.
const OUTPUT_ELEMENT: u32 = 0;

/// The names of the Core Audio calls, as an [`AudioError`] shows them.
mod call {
    pub(super) const FIND_NEXT: &str = "AudioComponentFindNext";
    pub(super) const INSTANCE_NEW: &str = "AudioComponentInstanceNew";
    pub(super) const INSTANCE_DISPOSE: &str = "AudioComponentInstanceDispose";
    pub(super) const SET_STREAM_FORMAT: &str =
        "AudioUnitSetProperty(kAudioUnitProperty_StreamFormat)";
    pub(super) const SET_RENDER_CALLBACK: &str =
        "AudioUnitSetProperty(kAudioUnitProperty_SetRenderCallback)";
    pub(super) const INITIALIZE: &str = "AudioUnitInitialize";
    pub(super) const UNINITIALIZE: &str = "AudioUnitUninitialize";
    pub(super) const OUTPUT_UNIT_START: &str = "AudioOutputUnitStart";
    pub(super) const OUTPUT_UNIT_STOP: &str = "AudioOutputUnitStop";
    pub(super) const GET_DEFAULT_OUTPUT_DEVICE: &str =
        "AudioObjectGetPropertyData(kAudioHardwarePropertyDefaultOutputDevice)";
    pub(super) const GET_NAME: &str = "AudioObjectGetPropertyData(kAudioObjectPropertyName)";
}

/// The audio object of the whole audio system.
const SYSTEM_OBJECT: AudioObjectID = kAudioObjectSystemObject.cast_unsigned();

/// Writes the samples that the output unit plays.
pub trait Render: Send + 'static {
    /// Writes the next samples into an interleaved buffer of `channels`
    /// channels.
    ///
    /// The output unit calls this on the real-time audio thread. It must not
    /// allocate, lock, or panic.
    fn render(&mut self, interleaved: &mut [f32], channels: usize);
}

/// A Core Audio call that failed. The error names the call.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{call} {problem}")]
pub struct AudioError {
    /// The name of the call that failed.
    call: &'static str,
    /// What went wrong.
    problem: Problem,
}

/// What went wrong in a Core Audio call.
#[derive(Debug, Clone, PartialEq)]
enum Problem {
    /// The call returned a status that is not [`NO_ERR`].
    Status(OSStatus),
    /// The call succeeded but gave no result. The text says what is missing.
    NoResult(&'static str),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => {
                write!(f, "failed with status {status}")?;
                match four_character_code(*status) {
                    Some(code) => write!(f, " ('{code}')"),
                    None => Ok(()),
                }
            }
            Self::NoResult(missing) => f.write_str(missing),
        }
    }
}

/// Gives the four bytes of `status` as text, when each byte is printable
/// ASCII.
///
/// Many Core Audio statuses are four-character codes, for example `'!obj'`.
/// The text is easier to find in the headers than the number.
fn four_character_code(status: OSStatus) -> Option<String> {
    let bytes = status.to_be_bytes();
    bytes
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
        .then(|| bytes.iter().copied().map(char::from).collect())
}

impl AudioError {
    /// Makes the error of a call that returned `status`.
    pub(crate) fn from_status(call: &'static str, status: OSStatus) -> Self {
        Self {
            call,
            problem: Problem::Status(status),
        }
    }

    /// Makes the error of a call that gave no result. `missing` says what is
    /// missing, for example "found no default output unit".
    fn no_result(call: &'static str, missing: &'static str) -> Self {
        Self {
            call,
            problem: Problem::NoResult(missing),
        }
    }
}

/// Gives the name of the default output device, for example the name that
/// the Sound settings show.
///
/// # Errors
///
/// Returns an [`AudioError`] that names the call that failed.
pub fn default_output_device_name() -> Result<String, AudioError> {
    let device = default_output_device()?;
    // SAFETY: the data of the name property is a `CFStringRef`.
    let name: *const CFString = unsafe {
        object_property(
            device,
            kAudioObjectPropertyName,
            call::GET_NAME,
            ptr::null(),
        )
    }?;
    let name = NonNull::new(name.cast_mut())
        .ok_or_else(|| AudioError::no_result(call::GET_NAME, "gave no name"))?;
    // SAFETY: the name property gives a string with a retain count of +1,
    // and the caller must release it. `CFRetained` releases it at drop.
    let name = unsafe { CFRetained::from_raw(name) };
    Ok(name.to_string())
}

/// Gives the nominal sample rate of the default output device.
///
/// # Errors
///
/// Returns an [`AudioError`] that names the call that failed.
pub fn default_output_sample_rate() -> Result<SampleRate, AudioError> {
    SampleRate::new(1.0).ok_or_else(|| AudioError::no_result(call::GET_NAME, "stub"))
}

/// Gives the default output device, which the default output unit plays to.
fn default_output_device() -> Result<AudioObjectID, AudioError> {
    // SAFETY: the data of the default output device property is an
    // `AudioObjectID`.
    let device = unsafe {
        object_property(
            SYSTEM_OBJECT,
            kAudioHardwarePropertyDefaultOutputDevice,
            call::GET_DEFAULT_OUTPUT_DEVICE,
            kAudioObjectUnknown,
        )
    }?;
    if device == kAudioObjectUnknown {
        return Err(AudioError::no_result(
            call::GET_DEFAULT_OUTPUT_DEVICE,
            "found no default output device",
        ));
    }
    Ok(device)
}

/// Gives the value of a property in the global scope of the main element of
/// `object`. `initial` stays the value when the call writes nothing.
///
/// # Safety
///
/// `T` is the type of the data of the property `selector`.
unsafe fn object_property<T>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    call: &'static str,
    initial: T,
) -> Result<T, AudioError> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut value = initial;
    let mut size = const { byte_size::<T>() };
    // SAFETY: `address` and `size` live for the whole call, and `size` is
    // the size of `value`. The caller makes sure that `T` is the type of the
    // data of the property, so the call writes a valid `T`. There is no
    // qualifier.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast::<c_void>(),
        )
    };
    check(call, status)?;
    Ok(value)
}

/// Gives an error that names `call` when `status` is not [`NO_ERR`].
fn check(call: &'static str, status: OSStatus) -> Result<(), AudioError> {
    if status == NO_ERR {
        Ok(())
    } else {
        Err(AudioError::from_status(call, status))
    }
}

/// Gives the size of `T` in bytes, as the `u32` that Core Audio takes.
///
/// Call it in a constant, so that a size too large for a `u32` stops the
/// build.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the assert above the cast refuses a size that a u32 does not hold"
)]
const fn byte_size<T>() -> u32 {
    assert!(size_of::<T>() <= u32::MAX as usize);
    size_of::<T>() as u32
}

/// The size of a stream format, in bytes.
const STREAM_FORMAT_SIZE: u32 = byte_size::<AudioStreamBasicDescription>();

/// The size of a render callback registration, in bytes.
const RENDER_CALLBACK_SIZE: u32 = byte_size::<AURenderCallbackStruct>();

/// The default output unit, which plays the samples of a renderer.
///
/// The unit plays until [`OutputUnit::stop`].
pub struct OutputUnit {
    /// The instance of the default output unit. It is not null.
    unit: AudioUnit,
}

impl OutputUnit {
    /// Starts the default output unit at `rate`, with `renderer` as the
    /// source of its samples.
    ///
    /// The stream is linear PCM, `f32`, packed and interleaved, with two
    /// channels. The unit converts `rate` to the rate of the device when the
    /// two differ.
    ///
    /// # Errors
    ///
    /// Returns an [`AudioError`] that names the call that failed.
    pub fn start<R: Render>(rate: SampleRate, renderer: R) -> Result<Self, AudioError> {
        let description = AudioComponentDescription {
            componentType: kAudioUnitType_Output,
            componentSubType: kAudioUnitSubType_DefaultOutput,
            componentManufacturer: kAudioUnitManufacturer_Apple,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        // SAFETY: a null component starts the search at the first component,
        // and `description` lives for the whole call.
        let component =
            unsafe { AudioComponentFindNext(ptr::null_mut(), NonNull::from(&description)) };
        if component.is_null() {
            return Err(AudioError::no_result(
                call::FIND_NEXT,
                "found no default output unit",
            ));
        }

        let mut unit: AudioUnit = ptr::null_mut();
        // SAFETY: `component` is a component that the search found, and
        // `unit` is a valid place for the new instance.
        let status = unsafe { AudioComponentInstanceNew(component, NonNull::from(&mut unit)) };
        check(call::INSTANCE_NEW, status)?;
        if unit.is_null() {
            return Err(AudioError::no_result(
                call::INSTANCE_NEW,
                "gave no instance",
            ));
        }

        let format = stream_format(rate);
        // SAFETY: `unit` is a live instance, and `format` is a stream format
        // of the size that the call gets.
        unsafe {
            set_property(
                unit,
                kAudioUnitProperty_StreamFormat,
                &format,
                STREAM_FORMAT_SIZE,
                call::SET_STREAM_FORMAT,
            )
        }?;

        let renderer = NonNull::from(Box::leak(Box::new(renderer)));
        let callback = AURenderCallbackStruct {
            inputProc: Some(render_callback::<R>),
            inputProcRefCon: renderer.as_ptr().cast::<c_void>(),
        };
        // SAFETY: `unit` is a live instance, and `callback` is a callback
        // registration of the size that the call gets. The refCon points to
        // an `R`, which is what `render_callback::<R>` reads.
        unsafe {
            set_property(
                unit,
                kAudioUnitProperty_SetRenderCallback,
                &callback,
                RENDER_CALLBACK_SIZE,
                call::SET_RENDER_CALLBACK,
            )
        }?;

        // SAFETY: `unit` is a live instance with a stream format and a
        // render callback.
        check(call::INITIALIZE, unsafe { AudioUnitInitialize(unit) })?;
        // SAFETY: `unit` is a live, initialized instance.
        check(call::OUTPUT_UNIT_START, unsafe {
            AudioOutputUnitStart(unit)
        })?;

        Ok(Self { unit })
    }

    /// Stops the output unit, uninitializes it, and disposes of it.
    ///
    /// # Errors
    ///
    /// Returns an [`AudioError`] that names the first call that failed. The
    /// calls after a failed call still run.
    pub fn stop(self) -> Result<(), AudioError> {
        // SAFETY: `self.unit` is a live instance that `start` started.
        let stopped = check(call::OUTPUT_UNIT_STOP, unsafe {
            AudioOutputUnitStop(self.unit)
        });
        // SAFETY: `self.unit` is a live instance that `start` initialized.
        let uninitialized = check(call::UNINITIALIZE, unsafe {
            AudioUnitUninitialize(self.unit)
        });
        // SAFETY: `self.unit` is a live instance, and nothing uses it after
        // this call.
        let disposed = check(call::INSTANCE_DISPOSE, unsafe {
            AudioComponentInstanceDispose(self.unit)
        });
        stopped.and(uninitialized).and(disposed)
    }
}

/// Gives the stream format of the output unit at `rate`.
fn stream_format(rate: SampleRate) -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        mSampleRate: rate.hz(),
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagsNativeFloatPacked,
        mBytesPerPacket: BYTES_PER_FRAME,
        mFramesPerPacket: 1,
        mBytesPerFrame: BYTES_PER_FRAME,
        mChannelsPerFrame: CHANNELS,
        mBitsPerChannel: BITS_PER_SAMPLE,
        mReserved: 0,
    }
}

/// Sets a property on the input scope of the output element of `unit`.
///
/// # Safety
///
/// `unit` is a live instance. `value` is the data of `property`, and `size`
/// is its size in bytes.
unsafe fn set_property<T>(
    unit: AudioUnit,
    property: AudioUnitPropertyID,
    value: &T,
    size: u32,
    call: &'static str,
) -> Result<(), AudioError> {
    // SAFETY: the caller keeps the contract of this function, and `value`
    // lives for the whole call.
    let status = unsafe {
        AudioUnitSetProperty(
            unit,
            property,
            kAudioUnitScope_Input,
            OUTPUT_ELEMENT,
            ptr::from_ref(value).cast::<c_void>(),
            size,
        )
    };
    check(call, status)
}

/// The render callback of the output unit. It passes the buffer of the
/// stream to the renderer.
///
/// It allocates nothing, takes no lock, and does not panic.
///
/// # Safety
///
/// `renderer` points to the `R` that [`OutputUnit::start`] registered, and
/// no other code touches that `R` while the unit can call this function.
/// `data` is null, or it points to a buffer list that Core Audio lends for
/// this call.
unsafe extern "C-unwind" fn render_callback<R: Render>(
    renderer: NonNull<c_void>,
    _action_flags: NonNull<AudioUnitRenderActionFlags>,
    _time_stamp: NonNull<AudioTimeStamp>,
    _bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> OSStatus {
    // SAFETY: the caller keeps the contract of this function, so `renderer`
    // points to a live `R` that only this call uses.
    let renderer = unsafe { renderer.cast::<R>().as_mut() };
    // SAFETY: the caller keeps the contract of this function for `data`.
    if let Some((samples, channels)) = unsafe { first_buffer(data, frames) } {
        renderer.render(samples, channels);
    }
    NO_ERR
}

/// Gives the samples of the first buffer of `data`, and its number of
/// channels.
///
/// The slice holds `frames` frames, or fewer when the buffer is smaller. It
/// gives `None` when there is no buffer, or when the buffer has no memory or
/// memory that is not aligned for `f32`.
///
/// # Safety
///
/// `data` is null, or it points to a buffer list that stays valid, and that
/// nothing else uses, for the lifetime `'a`.
unsafe fn first_buffer<'a>(
    data: *mut AudioBufferList,
    frames: u32,
) -> Option<(&'a mut [f32], usize)> {
    // SAFETY: the caller keeps the contract of this function.
    let list = unsafe { data.as_mut() }?;
    if list.mNumberBuffers == 0 {
        return None;
    }
    let [AudioBuffer {
        mNumberChannels,
        mDataByteSize,
        mData,
    }] = &mut list.mBuffers;
    let samples = mData.cast::<f32>();
    if samples.is_null() || !samples.is_aligned() {
        return None;
    }
    let channels = *mNumberChannels as usize;
    let by_frames = (frames as usize).saturating_mul(channels);
    let by_bytes = *mDataByteSize as usize / size_of::<f32>();
    // SAFETY: `samples` is not null and is aligned. The buffer holds
    // `mDataByteSize` bytes, thus at least `by_bytes` samples, and the length
    // is not larger. The caller makes sure that nothing else uses the
    // buffer for `'a`.
    let samples = unsafe { slice::from_raw_parts_mut(samples, by_frames.min(by_bytes)) };
    Some((samples, channels))
}

#[cfg(test)]
mod tests {
    use super::AudioError;

    #[test]
    fn an_audio_error_names_the_call_and_shows_a_four_character_code_as_text() {
        let cases = [
            // kAudioHardwareBadObjectError, '!obj'.
            (
                "AudioOutputUnitStart",
                0x216F_626A,
                "AudioOutputUnitStart failed with status 560947818 ('!obj')",
            ),
            // kAudioFormatUnsupportedDataFormatError, 'fmt?'. A space and
            // punctuation are printable too.
            (
                "AudioUnitInitialize",
                0x666D_743F,
                "AudioUnitInitialize failed with status 1718449215 ('fmt?')",
            ),
            (
                "AudioUnitInitialize",
                0x6465_6620,
                "AudioUnitInitialize failed with status 1684366880 ('def ')",
            ),
            // paramErr. Its bytes are not text.
            (
                "AudioUnitSetProperty(kAudioUnitProperty_StreamFormat)",
                -50,
                "AudioUnitSetProperty(kAudioUnitProperty_StreamFormat) failed with status -50",
            ),
            // kAudioUnitErr_FormatNotSupported.
            (
                "AudioUnitSetProperty(kAudioUnitProperty_StreamFormat)",
                -10868,
                "AudioUnitSetProperty(kAudioUnitProperty_StreamFormat) failed with status -10868",
            ),
            // Three printable bytes and a NUL are not a code.
            (
                "AudioOutputUnitStop",
                0x216F_6200,
                "AudioOutputUnitStop failed with status 560947712",
            ),
            // A DEL byte is not printable.
            (
                "AudioOutputUnitStop",
                0x216F_627F,
                "AudioOutputUnitStop failed with status 560947839",
            ),
        ];

        for (call, status, expected) in cases {
            assert_eq!(
                AudioError::from_status(call, status).to_string(),
                expected,
                "the text of status {status:#010X}"
            );
        }
    }
}
