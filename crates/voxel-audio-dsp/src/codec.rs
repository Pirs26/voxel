use rusty_opus::{Application, OpusDecoder, OpusEncoder, SignalType};

/// Standard VoIP audio sample rate (48 kHz mono).
pub const VOICE_SAMPLE_RATE: u32 = 48_000;

/// Standard VoIP frame size: 20 ms at 48 kHz mono.
pub const VOICE_FRAME_SAMPLES: usize = 960;

/// Target VoIP bitrate (~16 kbps for crisp intelligible speech at ~40 bytes/20ms frame over P2P datagrams).
const VOICE_BITRATE_BPS: i32 = 16_000;

/// Engine-agnostic low-latency VoIP codec abstraction.
///
/// Wraps `rusty-opus` (100% pure Rust, 20ms frames @ 48 kHz mono) behind this trait
/// so networking and DSP stay decoupled from any specific codec crate.
pub trait VoiceCodec: Send {
    fn encode_frame(&mut self, pcm_48khz_mono: &[f32], out: &mut [u8]) -> Result<usize, String>;
    fn decode_frame(&mut self, packet: &[u8], out_pcm: &mut [f32]) -> Result<usize, String>;
    fn decode_plc(&mut self, out_pcm: &mut [f32]) -> Result<usize, String>;
}

/// Pure-Rust Opus VoIP codec (48 kHz, mono, VoIP mode, 16 kbps).
pub struct OpusCodec {
    encoder: OpusEncoder,
    decoder: OpusDecoder,
}

impl OpusCodec {
    /// Creates a new 48 kHz mono VoIP Opus encoder + decoder pair (~16 kbps).
    pub fn new() -> Result<Self, String> {
        let mut encoder = OpusEncoder::new(VOICE_SAMPLE_RATE as i32, 1, Application::Voip)
            .map_err(|e| format!("opus encoder init failed: {e}"))?;
        encoder.bitrate_bps = VOICE_BITRATE_BPS;
        encoder.signal_type = Some(SignalType::Voice);

        let decoder = OpusDecoder::new(VOICE_SAMPLE_RATE as i32, 1)
            .map_err(|e| format!("opus decoder init failed: {e}"))?;

        Ok(Self { encoder, decoder })
    }

    pub fn encode_frame(
        &mut self,
        pcm_48khz_mono: &[f32],
        out: &mut [u8],
    ) -> Result<usize, String> {
        if pcm_48khz_mono.len() != VOICE_FRAME_SAMPLES {
            return Err(format!(
                "expected {} samples (20ms @ 48kHz), got {}",
                VOICE_FRAME_SAMPLES,
                pcm_48khz_mono.len()
            ));
        }
        self.encoder
            .encode(pcm_48khz_mono, VOICE_FRAME_SAMPLES, out)
            .map_err(|e| format!("opus encode failed: {e}"))
    }

    pub fn decode_frame(&mut self, packet: &[u8], out_pcm: &mut [f32]) -> Result<usize, String> {
        if out_pcm.len() < VOICE_FRAME_SAMPLES {
            return Err(format!(
                "output PCM buffer too small: need at least {}, got {}",
                VOICE_FRAME_SAMPLES,
                out_pcm.len()
            ));
        }
        if packet.is_empty() {
            return self.decode_plc(out_pcm);
        }
        self.decoder
            .decode(packet, VOICE_FRAME_SAMPLES, &mut out_pcm[..VOICE_FRAME_SAMPLES])
            .map_err(|e| format!("opus decode failed: {e}"))
    }

    pub fn decode_plc(&mut self, out_pcm: &mut [f32]) -> Result<usize, String> {
        if out_pcm.len() < VOICE_FRAME_SAMPLES {
            return Err(format!(
                "output PCM buffer too small for PLC: need at least {}, got {}",
                VOICE_FRAME_SAMPLES,
                out_pcm.len()
            ));
        }
        self.decoder
            .decode(&[], VOICE_FRAME_SAMPLES, &mut out_pcm[..VOICE_FRAME_SAMPLES])
            .map_err(|e| format!("opus PLC decode failed: {e}"))
    }
}

impl VoiceCodec for OpusCodec {
    fn encode_frame(&mut self, pcm_48khz_mono: &[f32], out: &mut [u8]) -> Result<usize, String> {
        OpusCodec::encode_frame(self, pcm_48khz_mono, out)
    }

    fn decode_frame(&mut self, packet: &[u8], out_pcm: &mut [f32]) -> Result<usize, String> {
        OpusCodec::decode_frame(self, packet, out_pcm)
    }

    fn decode_plc(&mut self, out_pcm: &mut [f32]) -> Result<usize, String> {
        OpusCodec::decode_plc(self, out_pcm)
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    #[test]
    fn opus_codec_roundtrip_and_plc() {
        let mut codec = OpusCodec::new().expect("create OpusCodec");
        let mut packet = [0u8; 512];
        let mut decoded = [0.0f32; VOICE_FRAME_SAMPLES];

        let mut last_rms = 0.0f32;
        for frame_idx in 0..8 {
            let mut input = [0.0f32; VOICE_FRAME_SAMPLES];
            for (i, sample) in input.iter_mut().enumerate() {
                let n = (frame_idx * VOICE_FRAME_SAMPLES + i) as f32;
                let t = n / VOICE_SAMPLE_RATE as f32;
                *sample = (TAU * 440.0 * t).sin() * 0.45 + (TAU * 880.0 * t).sin() * 0.15;
            }

            let encoded_len = VoiceCodec::encode_frame(&mut codec, &input, &mut packet)
                .expect("encode 960-sample frame");
            assert!(encoded_len > 0 && encoded_len <= packet.len());

            let decoded_len =
                VoiceCodec::decode_frame(&mut codec, &packet[..encoded_len], &mut decoded)
                    .expect("decode opus frame");
            assert_eq!(decoded_len, VOICE_FRAME_SAMPLES);
            assert!(decoded.iter().all(|s| s.is_finite()));

            let energy: f32 = decoded.iter().map(|s| s * s).sum::<f32>() / decoded.len() as f32;
            last_rms = energy.sqrt();
        }

        // After codec warm-up, reconstructed speech-band tone should carry clear energy
        assert!(
            last_rms > 0.05 && last_rms < 1.5,
            "unexpected decoded RMS: {last_rms}"
        );

        // Packet-loss concealment should cleanly synthesize 20ms frames without NaN/Inf
        for _ in 0..2 {
            let plc_len = VoiceCodec::decode_plc(&mut codec, &mut decoded).expect("decode PLC");
            assert_eq!(plc_len, VOICE_FRAME_SAMPLES);
            assert!(decoded.iter().all(|s| s.is_finite()));
        }
    }
}
