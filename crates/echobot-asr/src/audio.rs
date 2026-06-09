//! Audio decoding, encoding, and resampling for the ASR subsystem.
//!
//! Mirrors the helpers in `echobot/asr/audio.py`:
//!
//! * `read_wav_bytes` decodes a 16-bit PCM WAV blob (any channel count) to
//!   mono `f32` samples at the target sample rate. Uses `hound`.
//! * `decode_audio` decodes any common audio format (mp3, wav, ogg, flac,
//!   …) using `symphonia` and resamples / downmixes to mono `f32` at the
//!   target sample rate.
//! * `write_wav_bytes` encodes mono `f32` samples as a 16-bit PCM WAV blob.
//! * `pcm16le_bytes_to_floats` turns a raw little-endian PCM-16 byte stream
//!   into mono `f32` samples (used by the VAD layer for streaming input).
//!
//! The functions are CPU-heavy; callers should run them via
//! `tokio::task::spawn_blocking` or `asr_service::transcribe_wav_bytes` (which
//! already does so).

use std::io::Cursor;

use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::base::{AsrError, Result};

/// Decode a WAV byte buffer (any number of channels) to mono `f32`
/// samples at the target sample rate.
///
/// The Python equivalent only handles WAV; this function is a strict
/// superset because we delegate the header parsing to `hound`.
pub fn read_wav_bytes(audio_bytes: &[u8], target_sample_rate: u32) -> Result<Vec<f32>> {
    if audio_bytes.is_empty() {
        return Err(AsrError::Audio(
            "ASR audio body must not be empty".to_string(),
        ));
    }

    let cursor = Cursor::new(audio_bytes);
    let mut reader = hound::WavReader::new(cursor)
        .map_err(|e| AsrError::Audio(format!("WAV decode failed: {e}")))?;

    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let channels = spec.channels as usize;
    let bits = spec.bits_per_sample;

    // Collect into a vec of normalized f32 samples in [-1.0, 1.0].
    let mut mono: Vec<f32> = Vec::with_capacity(reader.duration() as usize * channels.max(1));

    let mut downmix_buf: Vec<f32> = Vec::new();
    let mut sample_index = 0_usize;
    let mut samples = reader.samples::<i32>();

    loop {
        match samples.next() {
            Some(Ok(value)) => {
                let channel = sample_index % channels.max(1);
                let normalized = match bits {
                    8 => (value - 128) as f32 / 128.0,
                    16 => value as f32 / 32_768.0,
                    24 => value as f32 / 8_388_608.0,
                    32 => value as f32 / 2_147_483_648.0,
                    other => {
                        return Err(AsrError::Audio(format!(
                            "unsupported WAV bit depth: {other}"
                        )));
                    }
                };
                if channels <= 1 {
                    mono.push(normalized);
                } else {
                    if channel == 0 {
                        downmix_buf.clear();
                    }
                    downmix_buf.push(normalized);
                    if channel + 1 == channels {
                        let avg: f32 =
                            downmix_buf.iter().copied().sum::<f32>() / downmix_buf.len() as f32;
                        mono.push(avg);
                    }
                }
                sample_index += 1;
            }
            Some(Err(e)) => {
                return Err(AsrError::Audio(format!("WAV read failed: {e}")));
            }
            None => break,
        }
    }

    if mono.is_empty() {
        return Ok(Vec::new());
    }

    if sample_rate == target_sample_rate {
        Ok(mono)
    } else {
        Ok(resample_samples(
            &mono,
            sample_rate,
            target_sample_rate,
        ))
    }
}

/// Encode mono `f32` samples as a 16-bit PCM WAV blob (one channel).
pub fn write_wav_bytes(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    if sample_rate == 0 {
        return Err(AsrError::Config(
            "ASR sample_rate must be positive".to_string(),
        ));
    }

    let mut buffer = Vec::with_capacity(samples.len() * 2 + 44);
    {
        let mut writer = hound::WavWriter::new(
            Cursor::new(&mut buffer),
            hound::WavSpec {
                channels: 1,
                sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .map_err(|e| AsrError::Audio(format!("WAV writer init failed: {e}")))?;

        for &sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            let value = if clamped >= 1.0 {
                i16::MAX
            } else {
                (clamped * 32_768.0) as i16
            };
            writer
                .write_sample(value)
                .map_err(|e| AsrError::Audio(format!("WAV write failed: {e}")))?;
        }
        writer
            .finalize()
            .map_err(|e| AsrError::Audio(format!("WAV finalize failed: {e}")))?;
    }
    Ok(buffer)
}

/// Decode any common audio format (mp3, wav, ogg, flac, …) to mono `f32`
/// samples at the target sample rate.
///
/// The Python port only handled WAV; this is the symphonia-backed
/// superset described in the porting notes.
pub fn decode_audio(audio_bytes: &[u8], target_sample_rate: u32) -> Result<Vec<f32>> {
    if audio_bytes.is_empty() {
        return Err(AsrError::Audio(
            "ASR audio body must not be empty".to_string(),
        ));
    }

    let cursor = Cursor::new(audio_bytes.to_vec());
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
    let hint = Hint::new();

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    );
    let mut format = match probed {
        Ok(f) => f,
        Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(AsrError::Audio(
                "ASR audio body is too short to probe".to_string(),
            ));
        }
        Err(e) => {
            return Err(AsrError::Audio(format!(
                "unsupported audio format: {e}"
            )));
        }
    };

    let track = format
        .format
        .default_track()
        .ok_or_else(|| AsrError::Audio("no default audio track found".to_string()))?;
    if track.codec_params.codec == CODEC_TYPE_NULL {
        return Err(AsrError::Audio(
            "audio track has no decodable codec".to_string(),
        ));
    }

    let source_sample_rate = track.codec_params.sample_rate.unwrap_or(target_sample_rate);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| AsrError::Audio(format!("audio decoder init failed: {e}")))?;

    let mut mono: Vec<f32> = Vec::new();
    // A reusable f32 buffer the decoder can write into.
    let mut f32_buf: Option<symphonia::core::audio::AudioBuffer<f32>> = None;

    while let Ok(packet) = format.format.next_packet() {
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let channels = spec.channels.count();
                let frames = decoded.frames();
                if frames == 0 {
                    continue;
                }

                // Lazily allocate (or resize) the f32 buffer to match this
                // packet. Symphonia can give us different buffer sizes per
                // packet, so we resize on the fly.
                let buf = f32_buf
                    .get_or_insert_with(|| decoded.make_equivalent::<f32>());
                if buf.capacity() < frames {
                    *buf = decoded.make_equivalent::<f32>();
                }
                decoded.convert(buf);
                let planes = buf.planes();
                let mut packet_mono = vec![0.0_f32; frames];
                for plane in planes.planes().iter().take(channels) {
                    for (i, sample) in plane.iter().enumerate() {
                        packet_mono[i] += *sample;
                    }
                }
                if channels > 1 {
                    for s in &mut packet_mono {
                        *s /= channels as f32;
                    }
                }
                mono.extend(packet_mono);
            }
            Err(SymError::DecodeError(_)) => {
                continue;
            }
            Err(SymError::ResetRequired) => {
                decoder.reset();
            }
            Err(e) => {
                return Err(AsrError::Audio(format!("audio decode failed: {e}")));
            }
        }
    }

    if mono.is_empty() {
        return Ok(Vec::new());
    }

    if source_sample_rate == target_sample_rate {
        Ok(mono)
    } else {
        Ok(resample_samples(
            &mono,
            source_sample_rate,
            target_sample_rate,
        ))
    }
}

/// Linear-interpolation resampler.
///
/// Matches the Python `resample_samples` helper 1:1. We don't pull in a
/// heavy resampling crate; for ASR warm-up an interpolating resampler is
/// good enough and keeps the dep footprint small.
pub fn resample_samples(
    samples: &[f32],
    input_sample_rate: u32,
    output_sample_rate: u32,
) -> Vec<f32> {
    if samples.is_empty() || input_sample_rate == output_sample_rate {
        return samples.to_vec();
    }
    if samples.len() == 1 {
        return samples.to_vec();
    }

    let output_length =
        std::cmp::max(1, (samples.len() as u64 * output_sample_rate as u64 / input_sample_rate as u64) as usize);
    if output_length == 1 {
        return vec![samples[0]];
    }

    let position_scale = (samples.len() - 1) as f64 / (output_length - 1) as f64;
    let mut out = Vec::with_capacity(output_length);
    for output_index in 0..output_length {
        let position = output_index as f64 * position_scale;
        let left_index = position.floor() as usize;
        let right_index = (left_index + 1).min(samples.len() - 1);
        let fraction = (position - left_index as f64) as f32;
        let value = samples[left_index] * (1.0 - fraction) + samples[right_index] * fraction;
        out.push(value);
    }
    out
}

/// Decode a raw little-endian PCM-16 byte stream to mono `f32` samples in
/// `[-1.0, 1.0]`.
///
/// Used by the VAD layer when the audio arrives in PCM-16 chunks from a
/// streaming source.
pub fn pcm16le_bytes_to_floats(audio_bytes: &[u8]) -> Vec<f32> {
    if audio_bytes.is_empty() {
        return Vec::new();
    }
    let trimmed = audio_bytes.len() - (audio_bytes.len() % 2);
    if trimmed == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(trimmed / 2);
    for chunk in audio_bytes[..trimmed].chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        out.push(sample as f32 / 32_768.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn round_trip_wav_through_hound() {
        // Build a small sine wave at 16 kHz, encode it, decode it back.
        let sample_rate = 16_000_u32;
        let frequency = 440.0_f32;
        let duration_seconds = 0.05_f32;
        let total_samples = (sample_rate as f32 * duration_seconds) as usize;
        let original: Vec<f32> = (0..total_samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (2.0 * std::f32::consts::PI * frequency * t).sin() * 0.5
            })
            .collect();

        let wav = write_wav_bytes(&original, sample_rate).expect("encode");
        assert!(!wav.is_empty());
        // Quick WAV magic check.
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");

        let decoded = read_wav_bytes(&wav, sample_rate).expect("decode");
        assert_eq!(decoded.len(), original.len());
        // First and middle samples should be very close.
        assert!(approx_eq(decoded[0], original[0], 1e-3));
        assert!(approx_eq(decoded[decoded.len() / 2], original[original.len() / 2], 1e-3));
    }

    #[test]
    fn decode_audio_handles_wav_via_symphonia() {
        // Encode a small WAV then decode it via the symphonia path.
        let sample_rate = 16_000_u32;
        let original: Vec<f32> = (0..800).map(|i| (i as f32 / 800.0) - 0.5).collect();
        let wav = write_wav_bytes(&original, sample_rate).expect("encode");

        let decoded = decode_audio(&wav, sample_rate).expect("symphonia decode");
        assert!(!decoded.is_empty());
        // Symphonia may apply dither; allow a generous tolerance.
        assert!(approx_eq(decoded[0], original[0], 5e-3));
    }

    #[test]
    fn resample_changes_length_and_keeps_shape() {
        let samples: Vec<f32> = (0..1000).map(|i| i as f32 / 999.0).collect();
        let resampled = resample_samples(&samples, 16_000, 8_000);
        assert!(resampled.len() >= 499 && resampled.len() <= 501);
        // Endpoints should match.
        assert!(approx_eq(resampled[0], 0.0, 1e-6));
        assert!(approx_eq(*resampled.last().unwrap(), 1.0, 1e-3));
    }

    #[test]
    fn pcm16le_round_trip_preserves_values() {
        let values = [0.0_f32, 0.25, -0.25, 0.5, -0.5, 1.0, -1.0];
        let mut bytes = Vec::with_capacity(values.len() * 2);
        for v in values {
            let s = (v.clamp(-1.0, 1.0) * 32_768.0) as i16;
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let floats = pcm16le_bytes_to_floats(&bytes);
        assert_eq!(floats.len(), values.len());
        for (orig, back) in values.iter().zip(floats.iter()) {
            assert!((orig - back).abs() < 1e-4, "orig={orig} back={back}");
        }
    }

    #[test]
    fn empty_inputs_are_handled() {
        assert!(decode_audio(&[], 16_000).is_err());
        assert!(read_wav_bytes(&[], 16_000).is_err());
        assert!(pcm16le_bytes_to_floats(&[]).is_empty());
        // pcm16 with a single trailing byte should be ignored.
        assert!(pcm16le_bytes_to_floats(&[0x12]).is_empty());
    }
}
