//! Audio files through AVAudioFile: a take's length, and splitting a stereo take into one mono file
//! per mic (left = TX1, right = TX2 in stereo mode).

use std::fmt;
use std::path::Path;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_avf_audio::{
    AVAudioFile, AVAudioFormat, AVAudioFrameCount, AVAudioPCMBuffer, AVChannelLayoutKey,
    AVNumberOfChannelsKey,
};
use objc2_foundation::{
    NSError, NSFileManager, NSMutableDictionary, NSNumber, NSString, NSURL, NSUUID,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplitError {
    NotStereo,
    FormatUnavailable,
    /// AVAudioFile's own error, in its words.
    File(String),
}

impl fmt::Display for SplitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStereo => f.write_str("notStereo"),
            Self::FormatUnavailable => f.write_str("formatUnavailable"),
            Self::File(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SplitError {}

impl From<Retained<NSError>> for SplitError {
    fn from(error: Retained<NSError>) -> Self {
        Self::File(error.localizedDescription().to_string())
    }
}

fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn open(path: &Path) -> Result<Retained<AVAudioFile>, Retained<NSError>> {
    // SAFETY: plain initialiser with a file URL.
    unsafe { AVAudioFile::initForReading_error(AVAudioFile::alloc(), &file_url(path)) }
}

/// Seconds of audio in a file, or `None` if AVAudioFile can't read it.
pub fn duration(path: &Path) -> Option<f64> {
    let file = open(path).ok()?;
    // SAFETY: reading properties of an open file.
    unsafe { Some(file.length() as f64 / file.fileFormat().sampleRate()) }
}

/// Writes one channel (0 = left/TX1, 1 = right/TX2) of a stereo file as a mono file.
pub fn extract(source: &Path, channel: usize, destination: &Path) -> Result<(), SplitError> {
    let folder = destination.parent().unwrap_or(Path::new("."));
    let scratch = folder.join(format!(".{}.wav", NSUUID::new().UUIDString()));
    let result = if channel == 0 {
        split(source, destination, &scratch)
    } else {
        split(source, &scratch, destination)
    };
    let _ = NSFileManager::defaultManager().removeItemAtURL_error(&file_url(&scratch));
    result
}

/// Writes `left` and `right` as mono files in the source's format family (WAV stays 24-bit PCM).
pub fn split(source: &Path, left: &Path, right: &Path) -> Result<(), SplitError> {
    let input = open(source)?;
    // SAFETY: AVFAudio calls on objects we own; the buffers are sized for `FRAMES` and every copy
    // stays within `frameLength` of both the source and destination channels.
    unsafe {
        let format = input.processingFormat();
        if format.channelCount() != 2 {
            return Err(SplitError::NotStereo);
        }
        let mono = AVAudioFormat::initWithCommonFormat_sampleRate_channels_interleaved(
            AVAudioFormat::alloc(),
            format.commonFormat(),
            format.sampleRate(),
            1,
            false,
        )
        .ok_or(SplitError::FormatUnavailable)?;
        let settings =
            NSMutableDictionary::dictionaryWithDictionary(&input.fileFormat().settings());
        if let Some(key) = AVNumberOfChannelsKey {
            settings.insert(key, &NSNumber::new_i32(1));
        }
        if let Some(key) = AVChannelLayoutKey {
            settings.removeObjectForKey(key);
        }
        let writer = |path: &Path| {
            AVAudioFile::initForWriting_settings_commonFormat_interleaved_error(
                AVAudioFile::alloc(),
                &file_url(path),
                &settings,
                format.commonFormat(),
                false,
            )
        };
        let out_left = writer(left)?;
        let out_right = writer(right)?;

        const FRAMES: AVAudioFrameCount = 48_000;
        let buffer = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
            AVAudioPCMBuffer::alloc(),
            &format,
            FRAMES,
        );
        let l = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
            AVAudioPCMBuffer::alloc(),
            &mono,
            FRAMES,
        );
        let r = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
            AVAudioPCMBuffer::alloc(),
            &mono,
            FRAMES,
        );
        let (Some(buffer), Some(l), Some(r)) = (buffer, l, r) else {
            return Err(SplitError::FormatUnavailable);
        };
        while input.framePosition() < input.length() {
            input.readIntoBuffer_frameCount_error(&buffer, FRAMES)?;
            let count = buffer.frameLength() as usize;
            let (src, dl, dr) = (
                buffer.floatChannelData(),
                l.floatChannelData(),
                r.floatChannelData(),
            );
            if count == 0 || src.is_null() || dl.is_null() || dr.is_null() {
                break;
            }
            std::ptr::copy_nonoverlapping((*src).as_ptr(), (*dl).as_ptr(), count);
            std::ptr::copy_nonoverlapping((*src.add(1)).as_ptr(), (*dr).as_ptr(), count);
            l.setFrameLength(buffer.frameLength());
            r.setFrameLength(buffer.frameLength());
            out_left.writeFromBuffer_error(&l)?;
            out_right.writeFromBuffer_error(&r)?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::scratch_folder;

    /// A 24-bit 48 kHz WAV with `channels` channels and a ramp per channel.
    pub fn write_wav(path: &Path, channels: u16, frames: u32) {
        let block = 3 * channels as u32;
        let mut data = Vec::new();
        for i in 0..frames {
            for c in 0..channels {
                let sample: i32 =
                    (i as i32 * 37 + c as i32 * 100_000) % 8_000_000 - (c as i32) * 4_000_000;
                data.extend_from_slice(&sample.to_le_bytes()[..3]);
            }
        }
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + data.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(channels.to_le_bytes());
        wav.extend(48_000u32.to_le_bytes());
        wav.extend((48_000 * block).to_le_bytes());
        wav.extend((block as u16).to_le_bytes());
        wav.extend(24u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((data.len() as u32).to_le_bytes());
        wav.extend(data);
        std::fs::write(path, wav).unwrap();
    }

    fn samples(path: &Path) -> (u16, u16, Vec<i32>) {
        let bytes = std::fs::read(path).unwrap();
        let fmt = bytes.windows(4).position(|w| w == b"fmt ").unwrap();
        let channels = u16::from_le_bytes([bytes[fmt + 10], bytes[fmt + 11]]);
        let bits = u16::from_le_bytes([bytes[fmt + 22], bytes[fmt + 23]]);
        let data = bytes.windows(4).position(|w| w == b"data").unwrap();
        let len = u32::from_le_bytes(bytes[data + 4..data + 8].try_into().unwrap()) as usize;
        let body = &bytes[data + 8..data + 8 + len];
        let values = body
            .chunks(3)
            .map(|c| i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8)
            .collect();
        (channels, bits, values)
    }

    #[test]
    fn splits_a_stereo_wav_into_24_bit_mono_tracks() {
        let folder = scratch_folder();
        let source = folder.join("12.00.00 Stereo.wav");
        write_wav(&source, 2, 100_000);
        let (left, right) = (
            folder.join("12.00.00 TX1.wav"),
            folder.join("12.00.00 TX2.wav"),
        );
        split(&source, &left, &right).unwrap();
        let (_, _, original) = samples(&source);
        let (channels, bits, l) = samples(&left);
        assert_eq!((channels, bits), (1, 24));
        let (_, _, r) = samples(&right);
        assert_eq!(l, original.iter().step_by(2).copied().collect::<Vec<_>>());
        assert_eq!(
            r,
            original
                .iter()
                .skip(1)
                .step_by(2)
                .copied()
                .collect::<Vec<_>>()
        );
        assert!((duration(&left).unwrap() - 100_000.0 / 48_000.0).abs() < 1e-9);

        // One mic alone keeps only its channel and leaves no scratch file behind.
        let solo = folder.join("12.00.00 TX2 solo.wav");
        extract(&source, 1, &solo).unwrap();
        assert_eq!(samples(&solo).2, r);
        let leftovers: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty());

        let mono = folder.join("mono.wav");
        write_wav(&mono, 1, 10);
        assert_eq!(
            split(&mono, &folder.join("a.wav"), &folder.join("b.wav")),
            Err(SplitError::NotStereo)
        );
        assert!(duration(&folder.join("missing.wav")).is_none());
        let _ = std::fs::remove_dir_all(&folder);
    }
}
