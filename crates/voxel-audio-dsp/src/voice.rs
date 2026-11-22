use std::{
    collections::HashMap,
    f32::consts::TAU,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

use cpal::{
    SampleFormat,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::codec::{OpusCodec, VOICE_FRAME_SAMPLES, VOICE_SAMPLE_RATE};

/// Reserved speaker ID for local solo self-echo testing in the Shed/Cavern.
pub const LOCAL_LOOPBACK_SPEAKER_ID: u16 = u16::MAX;

/// Capacity of the lock-free mic capture ring buffer (~340 ms @ 48 kHz).
const MIC_RING_CAPACITY: usize = 16_384;

/// Gate hang time in 20 ms frames (~200 ms) so word tails are never abruptly clipped.
const GATE_HANG_FRAMES: u8 = 10;

/// Gate hang time in samples at 48 kHz (~200 ms) for live callback telemetry.
const GATE_HANG_SAMPLES: u32 = VOICE_SAMPLE_RATE / 5;

/// Max Opus packet scratch buffer size (28 kbps @ 20 ms is ~70 bytes; 512 leaves huge headroom).
const MAX_OPUS_PACKET_BYTES: usize = 512;

/// Live microphone calibration & AI stealth telemetry (ideal for the Shed's Decibel Calibration Wall).
#[derive(Clone, Copy, Debug)]
pub struct MicTelemetry {
    /// Smoothed live mic loudness in dBFS (`-90.0..=0.0`).
    pub rms_db: f32,
    /// True when unmuted and the mic signal is above the noise gate (including hang time).
    pub gate_open: bool,
    /// Acoustic AI hearing energy (`0.0` below gate; whisper-safe `0.05..0.25` at `-42..-28 dB`;
    /// rising steeply to `1.0..4.0` for shouting/laughing `>-18 dB`).
    pub ai_noise_level: f32,
    /// False if no microphone device was found or OS permission was denied.
    pub available: bool,
}

/// Maps smoothed microphone loudness (`rms_db` in dBFS) and the configured `gate_threshold_db`
/// onto an AI acoustic detection energy curve.
///
/// - Below `gate_threshold_db`: `0.0` (silent to monsters)
/// - Soft whispers (`-42..-28 dB`, and up to `-27 dB`): gentle `0.05..0.33` (`< 0.35`, stealth-safe)
/// - Normal conversation (`-24..-18 dB`): sits around `0.55..1.0`
/// - Shouting / laughing (`>-18 dB`): spikes steeply above `1.4` up to `4.0`
pub fn compute_voice_ai_energy(rms_db: f32, gate_threshold_db: f32) -> f32 {
    if !rms_db.is_finite() || !gate_threshold_db.is_finite() {
        return 0.0;
    }
    let db = rms_db.clamp(-90.0, 0.0);
    let gate = gate_threshold_db.clamp(-90.0, 0.0);
    if db < gate {
        return 0.0;
    }

    if db < -42.0 {
        let span = (-42.0 - gate).max(1.0);
        let t = ((db - gate) / span).clamp(0.0, 1.0);
        0.01 + 0.04 * t
    } else if db <= -28.0 {
        let t = (db - (-42.0)) / 14.0;
        0.05 + 0.20 * t
    } else if db <= -18.0 {
        let t = (db - (-28.0)) / 10.0;
        0.25 + 0.75 * t
    } else {
        let t = ((db - (-18.0)) / 18.0).clamp(0.0, 1.0);
        1.0 + 3.0 * t.powf(1.05)
    }
}

/// Zero-allocation streaming linear resampler with continuous phase across chunk boundaries.
struct StreamingResampler {
    passthrough: bool,
    ratio: f64,
    phase: f64,
    prev: f32,
}

impl StreamingResampler {
    fn new(in_rate: f32, out_rate: f32) -> Self {
        let in_r = in_rate.max(1.0) as f64;
        let out_r = out_rate.max(1.0) as f64;
        let passthrough = (in_r - out_r).abs() < 1.0;
        Self {
            passthrough,
            ratio: in_r / out_r,
            phase: 1.0,
            prev: 0.0,
        }
    }

    #[inline]
    fn push_sample(&mut self, sample: f32, mut emit: impl FnMut(f32)) {
        if self.passthrough {
            emit(sample);
            return;
        }
        while self.phase <= 1.0 {
            let out = self.prev + (sample - self.prev) * (self.phase as f32);
            emit(out);
            self.phase += self.ratio;
        }
        self.phase -= 1.0;
        self.prev = sample;
    }
}

/// First-order 80 Hz DC-blocking high-pass filter for voice capture.
struct HighPass80Hz {
    alpha: f32,
    prev_x: f32,
    prev_y: f32,
}

impl HighPass80Hz {
    fn new(sample_rate: f32) -> Self {
        let rc = 1.0 / (TAU * 80.0);
        let dt = 1.0 / sample_rate.max(1.0);
        Self {
            alpha: (rc / (rc + dt)).clamp(0.5, 0.9999),
            prev_x: 0.0,
            prev_y: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.alpha * (self.prev_y + x - self.prev_x);
        self.prev_x = x;
        self.prev_y = if y.is_finite() { y } else { 0.0 };
        self.prev_y
    }
}

/// Enumerates available system microphone input device names cleanly without panicking.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for dev in devices {
        if let Ok(desc) = dev.description() {
            let name = desc.name().to_string();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

struct MicSharedState {
    threshold_db_bits: AtomicU32,
    input_gain_bits: AtomicU32,
    muted: AtomicBool,
    rms_db_bits: AtomicU32,
    gate_open: AtomicBool,
    available: AtomicBool,
}

impl MicSharedState {
    fn new(initial_threshold_db: f32) -> Self {
        let clamped = initial_threshold_db.clamp(-90.0, 0.0);
        Self {
            threshold_db_bits: AtomicU32::new(clamped.to_bits()),
            input_gain_bits: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            rms_db_bits: AtomicU32::new((-90.0f32).to_bits()),
            gate_open: AtomicBool::new(false),
            available: AtomicBool::new(false),
        }
    }
}

/// Real-time callback processor (zero heap allocations, zero locks).
struct CaptureProcessor {
    channels: usize,
    resampler: StreamingResampler,
    hp: HighPass80Hz,
    rms_sq: f32,
    hang_remaining: u32,
    tx: Producer<f32>,
    shared: Arc<MicSharedState>,
}

impl CaptureProcessor {
    fn new(
        in_sample_rate: f32,
        channels: usize,
        tx: Producer<f32>,
        shared: Arc<MicSharedState>,
    ) -> Self {
        Self {
            channels: channels.max(1),
            resampler: StreamingResampler::new(in_sample_rate, VOICE_SAMPLE_RATE as f32),
            hp: HighPass80Hz::new(VOICE_SAMPLE_RATE as f32),
            rms_sq: 0.0,
            hang_remaining: 0,
            tx,
            shared,
        }
    }

    #[inline]
    fn process_mono(&mut self, mono: f32, input_gain: f32, threshold_linear_sq: f32, muted: bool) {
        let scaled = mono * input_gain;
        let mono = if scaled.is_finite() {
            scaled.clamp(-4.0, 4.0)
        } else {
            0.0
        };

        // Fast attack (~12 ms), smooth release (~120 ms) @ 48 kHz for natural VU/gate response
        const ATTACK: f32 = 0.0017;
        const RELEASE: f32 = 0.00018;

        let hp = &mut self.hp;
        let rms_sq = &mut self.rms_sq;
        let hang_remaining = &mut self.hang_remaining;
        let tx = &mut self.tx;

        self.resampler.push_sample(mono, |s48| {
            let y = hp.process(s48);
            let sq = y * y;
            let coeff = if sq > *rms_sq { ATTACK } else { RELEASE };
            *rms_sq += (sq - *rms_sq) * coeff;

            if muted {
                *hang_remaining = 0;
            } else if *rms_sq >= threshold_linear_sq {
                *hang_remaining = GATE_HANG_SAMPLES;
            } else if *hang_remaining > 0 {
                *hang_remaining -= 1;
            }

            let _ = tx.push(y);
        });
    }

    fn process_interleaved_f32(&mut self, data: &[f32]) {
        let threshold_db = f32::from_bits(self.shared.threshold_db_bits.load(Ordering::Relaxed));
        let input_gain = f32::from_bits(self.shared.input_gain_bits.load(Ordering::Relaxed));
        let threshold_linear_sq = threshold_db_to_linear_sq(threshold_db);
        let muted = self.shared.muted.load(Ordering::Relaxed);
        let ch = self.channels;
        let inv_ch = 1.0 / ch as f32;

        for frame in data.chunks_exact(ch) {
            let mut sum = 0.0f32;
            for &s in frame {
                sum += s;
            }
            self.process_mono(sum * inv_ch, input_gain, threshold_linear_sq, muted);
        }
        self.publish_telemetry(muted);
    }

    fn process_interleaved_i16(&mut self, data: &[i16]) {
        let threshold_db = f32::from_bits(self.shared.threshold_db_bits.load(Ordering::Relaxed));
        let input_gain = f32::from_bits(self.shared.input_gain_bits.load(Ordering::Relaxed));
        let threshold_linear_sq = threshold_db_to_linear_sq(threshold_db);
        let muted = self.shared.muted.load(Ordering::Relaxed);
        let ch = self.channels;
        let inv_ch = 1.0 / (ch as f32 * 32768.0);

        for frame in data.chunks_exact(ch) {
            let mut sum = 0.0f32;
            for &s in frame {
                sum += s as f32;
            }
            self.process_mono(sum * inv_ch, input_gain, threshold_linear_sq, muted);
        }
        self.publish_telemetry(muted);
    }

    fn process_interleaved_u16(&mut self, data: &[u16]) {
        let threshold_db = f32::from_bits(self.shared.threshold_db_bits.load(Ordering::Relaxed));
        let input_gain = f32::from_bits(self.shared.input_gain_bits.load(Ordering::Relaxed));
        let threshold_linear_sq = threshold_db_to_linear_sq(threshold_db);
        let muted = self.shared.muted.load(Ordering::Relaxed);
        let ch = self.channels;
        let inv_ch = 1.0 / ch as f32;

        for frame in data.chunks_exact(ch) {
            let mut sum = 0.0f32;
            for &s in frame {
                sum += (s as f32 / 32768.0) - 1.0;
            }
            self.process_mono(sum * inv_ch, input_gain, threshold_linear_sq, muted);
        }
        self.publish_telemetry(muted);
    }

    fn process_interleaved_i32(&mut self, data: &[i32]) {
        let threshold_db = f32::from_bits(self.shared.threshold_db_bits.load(Ordering::Relaxed));
        let input_gain = f32::from_bits(self.shared.input_gain_bits.load(Ordering::Relaxed));
        let threshold_linear_sq = threshold_db_to_linear_sq(threshold_db);
        let muted = self.shared.muted.load(Ordering::Relaxed);
        let ch = self.channels;
        let inv_ch = 1.0 / ch as f64;

        for frame in data.chunks_exact(ch) {
            let mut sum = 0.0f64;
            for &s in frame {
                sum += s as f64 / 2_147_483_648.0;
            }
            self.process_mono(
                (sum * inv_ch) as f32,
                input_gain,
                threshold_linear_sq,
                muted,
            );
        }
        self.publish_telemetry(muted);
    }

    fn process_interleaved_f64(&mut self, data: &[f64]) {
        let threshold_db = f32::from_bits(self.shared.threshold_db_bits.load(Ordering::Relaxed));
        let input_gain = f32::from_bits(self.shared.input_gain_bits.load(Ordering::Relaxed));
        let threshold_linear_sq = threshold_db_to_linear_sq(threshold_db);
        let muted = self.shared.muted.load(Ordering::Relaxed);
        let ch = self.channels;
        let inv_ch = 1.0 / ch as f64;

        for frame in data.chunks_exact(ch) {
            let mut sum = 0.0f64;
            for &s in frame {
                sum += s;
            }
            self.process_mono(
                (sum * inv_ch) as f32,
                input_gain,
                threshold_linear_sq,
                muted,
            );
        }
        self.publish_telemetry(muted);
    }

    #[inline]
    fn publish_telemetry(&self, muted: bool) {
        let db = linear_rms_to_dbfs(self.rms_sq.max(0.0).sqrt());
        self.shared
            .rms_db_bits
            .store(db.to_bits(), Ordering::Relaxed);
        self.shared
            .gate_open
            .store(!muted && self.hang_remaining > 0, Ordering::Relaxed);
    }
}

#[inline]
fn threshold_db_to_linear_sq(threshold_db: f32) -> f32 {
    10.0f32.powf(threshold_db.clamp(-90.0, 0.0) / 10.0)
}

#[inline]
fn linear_rms_to_dbfs(rms: f32) -> f32 {
    if rms > 1e-5 {
        (20.0 * rms.log10()).clamp(-90.0, 0.0)
    } else {
        -90.0
    }
}

/// Real-time microphone capture -> 48 kHz mono HPF -> noise gate -> 20 ms Opus VoIP frames.
///
/// Gracefully degrades with `telemetry().available == false` if no microphone is present
/// or OS permissions are denied; never panics.
pub struct MicCapture {
    _stream: Option<cpal::Stream>,
    rx: Consumer<f32>,
    shared: Arc<MicSharedState>,
    codec: Option<OpusCodec>,
    frame_accum: Vec<f32>,
    encode_buf: [u8; MAX_OPUS_PACKET_BYTES],
    hang_frames: u8,
    seq: u16,
}

impl MicCapture {
    /// Starts capturing from the system default microphone with the given noise gate threshold (dBFS).
    pub fn start(initial_threshold_db: f32) -> Self {
        Self::start_with_device(initial_threshold_db, None)
    }

    /// Starts capturing from a named microphone device (falling back to the system default if
    /// `device_name` is `None` or not found).
    pub fn start_with_device(initial_threshold_db: f32, device_name: Option<&str>) -> Self {
        let (tx, rx) = RingBuffer::<f32>::new(MIC_RING_CAPACITY);
        let shared = Arc::new(MicSharedState::new(initial_threshold_db));
        let codec = OpusCodec::new().ok();

        let stream = start_input_stream(tx, Arc::clone(&shared), device_name);
        if stream.is_some() {
            shared.available.store(true, Ordering::Relaxed);
        }

        Self {
            _stream: stream,
            rx,
            shared,
            codec,
            frame_accum: Vec::with_capacity(VOICE_FRAME_SAMPLES * 4),
            encode_buf: [0u8; MAX_OPUS_PACKET_BYTES],
            hang_frames: 0,
            seq: 0,
        }
    }

    pub fn set_input_gain(&self, gain: f32) {
        let clamped = if gain.is_finite() {
            gain.clamp(0.1, 4.0)
        } else {
            1.0
        };
        self.shared
            .input_gain_bits
            .store(clamped.to_bits(), Ordering::Relaxed);
    }

    pub fn input_gain(&self) -> f32 {
        f32::from_bits(self.shared.input_gain_bits.load(Ordering::Relaxed))
    }

    pub fn set_gate_threshold_db(&self, db: f32) {
        let clamped = if db.is_finite() {
            db.clamp(-90.0, 0.0)
        } else {
            -42.0
        };
        self.shared
            .threshold_db_bits
            .store(clamped.to_bits(), Ordering::Relaxed);
    }

    pub fn gate_threshold_db(&self) -> f32 {
        f32::from_bits(self.shared.threshold_db_bits.load(Ordering::Relaxed))
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    pub fn is_muted(&self) -> bool {
        self.shared.muted.load(Ordering::Relaxed)
    }

    pub fn telemetry(&self) -> MicTelemetry {
        let available = self.shared.available.load(Ordering::Relaxed);
        if !available {
            return MicTelemetry {
                rms_db: -90.0,
                gate_open: false,
                ai_noise_level: 0.0,
                available: false,
            };
        }

        let rms_db = f32::from_bits(self.shared.rms_db_bits.load(Ordering::Relaxed))
            .clamp(-90.0, 0.0);
        let muted = self.is_muted();
        let threshold_db = self.gate_threshold_db();
        let gate_open = !muted && self.shared.gate_open.load(Ordering::Relaxed);
        let ai_noise_level = if muted {
            0.0
        } else {
            compute_voice_ai_energy(rms_db, threshold_db)
        };

        MicTelemetry {
            rms_db,
            gate_open,
            ai_noise_level,
            available: true,
        }
    }

    /// Drains accumulated 48 kHz mono mic samples, encodes gated 20 ms frames with `OpusCodec`,
    /// and returns `(seq, opus_packet)` pairs ready for `voxel-net` voice datagrams.
    pub fn poll_encoded_frames(&mut self) -> Vec<(u16, Vec<u8>)> {
        while let Ok(s) = self.rx.pop() {
            self.frame_accum.push(s);
        }

        let mut packets = Vec::new();
        let threshold_db = self.gate_threshold_db();
        let threshold_linear_sq = threshold_db_to_linear_sq(threshold_db);
        let muted = self.is_muted();
        let mut consumed = 0usize;

        while self.frame_accum.len() - consumed >= VOICE_FRAME_SAMPLES {
            let frame = &self.frame_accum[consumed..consumed + VOICE_FRAME_SAMPLES];
            consumed += VOICE_FRAME_SAMPLES;

            let energy = frame.iter().map(|s| s * s).sum::<f32>() / VOICE_FRAME_SAMPLES as f32;

            let should_emit = if muted {
                self.hang_frames = 0;
                false
            } else if energy >= threshold_linear_sq {
                self.hang_frames = GATE_HANG_FRAMES;
                true
            } else if self.hang_frames > 0 {
                self.hang_frames -= 1;
                true
            } else {
                false
            };

            if !should_emit {
                continue;
            }

            if let Some(ref mut codec) = self.codec
                && let Ok(len) = codec.encode_frame(frame, &mut self.encode_buf)
                && len > 0
            {
                let seq = self.seq;
                self.seq = self.seq.wrapping_add(1);
                packets.push((seq, self.encode_buf[..len].to_vec()));
            }
        }

        if consumed > 0 {
            self.frame_accum.drain(..consumed);
        }

        packets
    }
}

fn start_input_stream(
    tx: Producer<f32>,
    shared: Arc<MicSharedState>,
    device_name: Option<&str>,
) -> Option<cpal::Stream> {
    let host = cpal::default_host();
    let selected = device_name.and_then(|wanted| {
        host.input_devices().ok()?.find(|d| {
            d.description()
                .map(|desc| desc.name() == wanted)
                .unwrap_or(false)
        })
    });
    let device = selected.or_else(|| host.default_input_device())?;
    let default_cfg = device.default_input_config().ok()?;

    let in_rate = default_cfg.sample_rate() as f32;
    let channels = default_cfg.channels() as usize;
    let sample_format = default_cfg.sample_format();
    let config: cpal::StreamConfig = default_cfg.into();

    let mut proc = CaptureProcessor::new(in_rate, channels, tx, shared);
    let err_fn = |err| eprintln!("[voxel-audio-dsp] mic stream error: {err}");

    let stream = match sample_format {
        SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| proc.process_interleaved_f32(data),
                err_fn,
                None,
            )
            .ok()?,
        SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _| proc.process_interleaved_i16(data),
                err_fn,
                None,
            )
            .ok()?,
        SampleFormat::U16 => device
            .build_input_stream(
                config,
                move |data: &[u16], _| proc.process_interleaved_u16(data),
                err_fn,
                None,
            )
            .ok()?,
        SampleFormat::I32 => device
            .build_input_stream(
                config,
                move |data: &[i32], _| proc.process_interleaved_i32(data),
                err_fn,
                None,
            )
            .ok()?,
        SampleFormat::F64 => device
            .build_input_stream(
                config,
                move |data: &[f64], _| proc.process_interleaved_f64(data),
                err_fn,
                None,
            )
            .ok()?,
        _ => return None,
    };

    if stream.play().is_err() {
        return None;
    }

    Some(stream)
}

struct SpeakerDecoder {
    codec: OpusCodec,
    last_seq: Option<u16>,
    resampler: StreamingResampler,
    decode_scratch: [f32; VOICE_FRAME_SAMPLES],
}

impl SpeakerDecoder {
    fn new(output_sample_rate: f32) -> Result<Self, String> {
        Ok(Self {
            codec: OpusCodec::new()?,
            last_seq: None,
            resampler: StreamingResampler::new(VOICE_SAMPLE_RATE as f32, output_sample_rate),
            decode_scratch: [0.0; VOICE_FRAME_SAMPLES],
        })
    }
}

/// Per-speaker Opus VoIP decoder pool with sequence tracking, 1-2 packet PLC concealment,
/// and continuous linear resampling to the audio output device's native sample rate.
///
/// Supports any remote `speaker_id: u16` as well as [`LOCAL_LOOPBACK_SPEAKER_ID`] (`u16::MAX`)
/// for solo self-echo testing in the Shed/Cavern.
pub struct VoiceDecoderPool {
    output_sample_rate: f32,
    speakers: HashMap<u16, SpeakerDecoder>,
}

impl VoiceDecoderPool {
    pub fn new(output_sample_rate: f32) -> Self {
        Self {
            output_sample_rate: output_sample_rate.max(8_000.0),
            speakers: HashMap::new(),
        }
    }

    pub fn output_sample_rate(&self) -> f32 {
        self.output_sample_rate
    }

    pub fn remove_speaker(&mut self, speaker_id: u16) {
        self.speakers.remove(&speaker_id);
    }

    /// Decodes an incoming Opus VoIP packet for `speaker_id` at sequence `seq`, applying
    /// 1-2 frame PLC on small packet gaps and resampling to `output_sample_rate`.
    pub fn decode_packet(&mut self, speaker_id: u16, seq: u16, opus_payload: &[u8]) -> Vec<f32> {
        let out_rate = self.output_sample_rate;
        let speaker = match self.speakers.entry(speaker_id) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let Ok(dec) = SpeakerDecoder::new(out_rate) else {
                    return Vec::new();
                };
                entry.insert(dec)
            }
        };

        let mut plc_frames = 0usize;
        if let Some(prev) = speaker.last_seq {
            let diff = seq.wrapping_sub(prev);
            if diff == 0 || diff > 32_768 {
                // Duplicate or stale out-of-order packet
                return Vec::new();
            } else if diff == 2 || diff == 3 {
                // 1 or 2 dropped packets: conceal with Opus PLC before decoding this frame
                plc_frames = (diff - 1) as usize;
            }
            // Larger gaps (e.g. speaker paused and resumed later) resync cleanly without PLC spam
        }
        speaker.last_seq = Some(seq);

        let est_48k = (plc_frames + 1) * VOICE_FRAME_SAMPLES;
        let est_out = ((est_48k as f32) * (out_rate / VOICE_SAMPLE_RATE as f32)).ceil() as usize + 8;
        let mut out = Vec::with_capacity(est_out);

        for _ in 0..plc_frames {
            if let Ok(n) = speaker.codec.decode_plc(&mut speaker.decode_scratch) {
                for &s in &speaker.decode_scratch[..n] {
                    speaker.resampler.push_sample(s, |r| out.push(r));
                }
            }
        }

        let decode_res = if opus_payload.is_empty() {
            speaker.codec.decode_plc(&mut speaker.decode_scratch)
        } else {
            speaker
                .codec
                .decode_frame(opus_payload, &mut speaker.decode_scratch)
                .or_else(|_| speaker.codec.decode_plc(&mut speaker.decode_scratch))
        };

        if let Ok(n) = decode_res {
            for &s in &speaker.decode_scratch[..n] {
                speaker.resampler.push_sample(s, |r| out.push(r));
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_voice_ai_energy_whisper_vs_shout_curve() {
        let gate = -45.0;

        // Below gate: strictly silent to AI
        assert_eq!(compute_voice_ai_energy(-90.0, gate), 0.0);
        assert_eq!(compute_voice_ai_energy(-45.1, gate), 0.0);

        // Soft whispers (-42..-28 dB, and up to -27 dB): gentle whisper-safe band (< 0.35)
        let whisper_low = compute_voice_ai_energy(-42.0, gate);
        let whisper_mid = compute_voice_ai_energy(-35.0, gate);
        let whisper_high = compute_voice_ai_energy(-28.0, gate);
        let whisper_edge = compute_voice_ai_energy(-27.0, gate);
        assert!((whisper_low - 0.05).abs() < 1e-4, "whisper_low={whisper_low}");
        assert!(
            whisper_mid > 0.05 && whisper_mid < 0.25,
            "whisper_mid={whisper_mid}"
        );
        assert!((whisper_high - 0.25).abs() < 1e-4, "whisper_high={whisper_high}");
        assert!(
            whisper_edge < 0.35,
            "quiet whispering at -27 dB must stay stealth-safe (< 0.35), got {whisper_edge}"
        );

        // Normal conversation (-24..-18 dB): sits cleanly in 0.5..=1.0
        let talk_quiet = compute_voice_ai_energy(-24.0, gate);
        let normal = compute_voice_ai_energy(-22.0, gate);
        let loud_edge = compute_voice_ai_energy(-18.0, gate);
        assert!(
            (0.5..=1.0).contains(&talk_quiet),
            "normal conversation at -24 dB should sit in 0.5..=1.0, got {talk_quiet}"
        );
        assert!(
            (0.5..=1.0).contains(&normal) && normal > talk_quiet,
            "normal={normal}"
        );
        assert!((loud_edge - 1.0).abs() < 1e-4, "loud_edge={loud_edge}");

        // Shouting / laughing (>-18 dB): cleanly spikes > 1.4 up to 4.0
        let callout = compute_voice_ai_energy(-15.0, gate);
        let shout = compute_voice_ai_energy(-10.0, gate);
        let scream = compute_voice_ai_energy(0.0, gate);
        assert!(
            callout > 1.4,
            "shouting/laughing at -15 dB must spike > 1.4, got {callout}"
        );
        assert!(shout > 1.8 && shout < 3.0, "shout={shout}");
        assert!((scream - 4.0).abs() < 1e-4, "scream={scream}");

        // Monotonically non-decreasing across full dBFS range
        let mut prev = 0.0f32;
        for db_i in -90..=0 {
            let e = compute_voice_ai_energy(db_i as f32, gate);
            assert!(e.is_finite() && e >= prev, "non-monotonic at {db_i} dB: {e} < {prev}");
            prev = e;
        }
    }

    #[test]
    fn voice_decoder_pool_handles_plc_and_resampling() {
        let mut enc = OpusCodec::new().expect("encoder");
        let mut pool = VoiceDecoderPool::new(44_100.0);
        let mut pkt = [0u8; 512];
        let mut pcm = [0.0f32; VOICE_FRAME_SAMPLES];

        for (i, s) in pcm.iter_mut().enumerate() {
            *s = (TAU * 350.0 * i as f32 / VOICE_SAMPLE_RATE as f32).sin() * 0.4;
        }

        let len0 = enc.encode_frame(&pcm, &mut pkt).expect("enc 0");
        let out0 = pool.decode_packet(LOCAL_LOOPBACK_SPEAKER_ID, 10, &pkt[..len0]);
        // 960 @ 48 kHz -> ~882 @ 44.1 kHz
        assert!((875..=890).contains(&out0.len()), "out0 len={}", out0.len());
        assert!(out0.iter().all(|s| s.is_finite()));

        // Duplicate seq dropped cleanly
        assert!(pool.decode_packet(LOCAL_LOOPBACK_SPEAKER_ID, 10, &pkt[..len0]).is_empty());

        // Skip seq 11 (1 lost packet) -> seq 12 produces 1 PLC frame + 1 decoded frame (~1764 samples)
        let len2 = enc.encode_frame(&pcm, &mut pkt).expect("enc 2");
        let out2 = pool.decode_packet(LOCAL_LOOPBACK_SPEAKER_ID, 12, &pkt[..len2]);
        assert!((1750..=1780).contains(&out2.len()), "out2 len={}", out2.len());
        assert!(out2.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn capture_processor_gate_linear_sq_matches_db_threshold() {
        let (tx, _rx) = RingBuffer::<f32>::new(MIC_RING_CAPACITY);
        let shared = Arc::new(MicSharedState::new(-30.0));
        let mut proc = CaptureProcessor::new(VOICE_SAMPLE_RATE as f32, 1, tx, Arc::clone(&shared));

        // Quiet tone (~-60 dBFS): gate must stay closed
        let quiet: Vec<f32> = (0..1024)
            .map(|i| (TAU * 440.0 * i as f32 / VOICE_SAMPLE_RATE as f32).sin() * 0.001)
            .collect();
        proc.process_interleaved_f32(&quiet);
        assert!(!shared.gate_open.load(Ordering::Relaxed));
        let quiet_db = f32::from_bits(shared.rms_db_bits.load(Ordering::Relaxed));
        assert!(quiet_db < -45.0, "expected quiet rms_db < -45, got {quiet_db}");

        // Loud speech-band tone (~-15 dBFS): gate must open and telemetry rms_db must rise above -30 dB
        let loud: Vec<f32> = (0..2048)
            .map(|i| (TAU * 440.0 * i as f32 / VOICE_SAMPLE_RATE as f32).sin() * 0.25)
            .collect();
        proc.process_interleaved_f32(&loud);
        assert!(shared.gate_open.load(Ordering::Relaxed));
        let loud_db = f32::from_bits(shared.rms_db_bits.load(Ordering::Relaxed));
        assert!(loud_db > -25.0 && loud_db < -5.0, "unexpected loud rms_db: {loud_db}");

        // Muting closes the gate immediately on the next callback
        shared.muted.store(true, Ordering::Relaxed);
        proc.process_interleaved_f32(&loud[..256]);
        assert!(!shared.gate_open.load(Ordering::Relaxed));

        // Input gain boosts signal level cleanly
        shared.muted.store(false, Ordering::Relaxed);
        shared.input_gain_bits.store(3.0f32.to_bits(), Ordering::Relaxed);
        let soft: Vec<f32> = (0..2048)
            .map(|i| (TAU * 440.0 * i as f32 / VOICE_SAMPLE_RATE as f32).sin() * 0.03)
            .collect();
        proc.process_interleaved_f32(&soft);
        let boosted_db = f32::from_bits(shared.rms_db_bits.load(Ordering::Relaxed));
        assert!(boosted_db > -28.0, "expected boosted signal > -28 dB, got {boosted_db}");

        // Device enumeration must never panic
        let _ = list_input_devices();
    }
}
