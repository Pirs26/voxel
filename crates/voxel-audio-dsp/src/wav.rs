use std::path::Path;

/// Lightweight zero-dependency PCM WAV loader (16-bit signed int & 32-bit float, mono/stereo)
/// with linear resampling to the audio engine's output sample rate.
pub fn load_wav_mono(path: impl AsRef<Path>, target_sample_rate: f32) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path.as_ref())
        .map_err(|e| format!("failed to read {}: {e}", path.as_ref().display()))?;

    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("invalid WAV header".into());
    }

    let mut pos = 12usize;
    let mut audio_format = 0u16;
    let mut num_channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;
    let mut data_slice: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let payload_start = pos + 8;
        let payload_end = (payload_start + size).min(bytes.len());

        if id == b"fmt " && payload_end - payload_start >= 16 {
            let f = &bytes[payload_start..payload_end];
            audio_format = u16::from_le_bytes([f[0], f[1]]);
            num_channels = u16::from_le_bytes([f[2], f[3]]);
            sample_rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
            bits_per_sample = u16::from_le_bytes([f[14], f[15]]);
        } else if id == b"data" {
            data_slice = Some(&bytes[payload_start..payload_end]);
        }

        // Chunks are word-aligned (padded to even byte count)
        pos = payload_start + size + (size & 1);
    }

    let data = data_slice.ok_or("WAV missing data chunk")?;
    if num_channels == 0 || sample_rate == 0 {
        return Err("WAV missing valid fmt chunk".into());
    }

    let ch = num_channels as usize;
    let mono: Vec<f32> = match (audio_format, bits_per_sample) {
        (1, 16) => {
            let frame_bytes = 2 * ch;
            data.chunks_exact(frame_bytes)
                .map(|frame| {
                    let mut sum = 0.0f32;
                    for c in 0..ch {
                        let s = i16::from_le_bytes([frame[c * 2], frame[c * 2 + 1]]);
                        sum += s as f32 / 32768.0;
                    }
                    sum / ch as f32
                })
                .collect()
        }
        (3, 32) => {
            let frame_bytes = 4 * ch;
            data.chunks_exact(frame_bytes)
                .map(|frame| {
                    let mut sum = 0.0f32;
                    for c in 0..ch {
                        let s = f32::from_le_bytes(frame[c * 4..c * 4 + 4].try_into().unwrap());
                        sum += s;
                    }
                    sum / ch as f32
                })
                .collect()
        }
        _ => {
            return Err(format!(
                "unsupported WAV format={audio_format}, bits={bits_per_sample}"
            ));
        }
    };

    if (sample_rate as f32 - target_sample_rate).abs() < 1.0 || mono.len() < 2 {
        return Ok(mono);
    }

    // Linear resample to output stream rate
    let ratio = sample_rate as f32 / target_sample_rate;
    let out_len = ((mono.len() as f32) / ratio).round().max(1.0) as usize;
    let mut resampled = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f32 * ratio;
        let idx = (src_pos as usize).min(mono.len() - 2);
        let frac = src_pos - idx as f32;
        let s = mono[idx] + (mono[idx + 1] - mono[idx]) * frac;
        resampled.push(s);
    }
    Ok(resampled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_cc0_stone_footstep_if_present() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/sfx/footstep_stone.wav");
        if path.exists() {
            let samples = load_wav_mono(&path, 48_000.0).expect("load stone wav");
            assert!(samples.len() > 2000);
            assert!(samples.iter().all(|s| s.is_finite()));
        }
    }
}
