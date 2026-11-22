use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use glam::Vec3;
use rtrb::{Consumer, Producer, RingBuffer};
use voxel_acoustics::{Bands3, EarlyTap, MAX_EARLY_TAPS, ReverbEstimate, SourceAcousticParams};

use crate::{
    biquad::ThreeBandFilter,
    fdn_reverb::FdnReverb,
    spatial::{EarlyReflectionLine, stereo_pan_gains},
    synth::{SynthClip, load_or_generate_clip},
};

pub type SoundClipId = SynthClip;

const MAX_VOICES: usize = 24;

/// Maximum concurrent live proximity VoIP speaker streams mixed through the acoustic DSP chain.
pub const MAX_VOICE_CHANNELS: usize = 6;

/// Lock-free PCM ring buffer capacity per live voice channel (~200 ms @ 48 kHz).
const VOICE_RING_CAPACITY: usize = 9_600;

/// Lock-free commands sent from the simulation thread to the real-time `cpal` callback.
#[derive(Clone, Debug)]
pub enum AudioCommand {
    /// Play a one-shot or looping spatialized clip at a source ID.
    PlayClip {
        source_id: u64,
        clip: SoundClipId,
        volume: f32,
        pitch: f32,
        looping: bool,
        initial_params: SourceAcousticParams,
    },
    /// Update listener orientation & global room reverb parameters.
    UpdateListener {
        pos_m: Vec3,
        right: Vec3,
        reverb: ReverbEstimate,
    },
    /// Update solved voxel-density occlusion + early reflection taps for an active source.
    UpdateSource(SourceAcousticParams),
    /// Stop all voices matching `source_id`.
    StopSource(u64),
    /// Activate and update acoustic parameters for a streaming VoIP voice channel slot.
    UpdateVoiceChannel {
        slot: usize,
        params: SourceAcousticParams,
    },
    /// Deactivate and reset a streaming VoIP voice channel slot.
    ClearVoiceChannel {
        slot: usize,
    },
    /// Update master output volume and remote proximity voice chat volume.
    SetVolumes {
        master: f32,
        voice: f32,
    },
}

/// Enumerates available system audio output device names cleanly without panicking.
pub fn list_output_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else {
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

struct Voice {
    active: bool,
    source_id: u64,
    clip_idx: usize,
    cursor: f32,
    pitch: f32,
    volume: f32,
    looping: bool,
    pos_m: Vec3,
    direct_gain: Bands3,
    direct_gl: f32,
    direct_gr: f32,
    early_taps: [EarlyTap; MAX_EARLY_TAPS],
    early_gains: [(f32, f32, usize); MAX_EARLY_TAPS],
    active_tap_count: usize,
    reverb_send: f32,
    filter: ThreeBandFilter,
    early_line: EarlyReflectionLine,
}

impl Voice {
    fn new(sample_rate: f32) -> Self {
        Self {
            active: false,
            source_id: 0,
            clip_idx: 0,
            cursor: 0.0,
            pitch: 1.0,
            volume: 1.0,
            looping: false,
            pos_m: Vec3::ZERO,
            direct_gain: Bands3::ONE,
            direct_gl: std::f32::consts::FRAC_1_SQRT_2,
            direct_gr: std::f32::consts::FRAC_1_SQRT_2,
            early_taps: [EarlyTap::default(); MAX_EARLY_TAPS],
            early_gains: [(0.0, 0.0, 1); MAX_EARLY_TAPS],
            active_tap_count: 0,
            reverb_send: 0.25,
            filter: ThreeBandFilter::new(sample_rate),
            early_line: EarlyReflectionLine::new(sample_rate),
        }
    }

    #[inline]
    fn update_spatial_cache(&mut self, listener_pos_m: Vec3, listener_right: Vec3) {
        let (gl, gr) = stereo_pan_gains(self.pos_m - listener_pos_m, listener_right);
        self.direct_gl = gl;
        self.direct_gr = gr;
        self.active_tap_count = self.early_line.prepare_cached_taps(
            &self.early_taps,
            listener_right,
            &mut self.early_gains,
        );
    }
}

/// Real-time lock-free streaming voice channel passing through the exact same
/// 3-band muffling, early reflections, stereo panning, and FDN reverb as SFX.
struct VoiceChannel {
    active: bool,
    rx: Consumer<f32>,
    last_sample: f32,
    pos_m: Vec3,
    direct_gain: Bands3,
    direct_gl: f32,
    direct_gr: f32,
    early_taps: [EarlyTap; MAX_EARLY_TAPS],
    early_gains: [(f32, f32, usize); MAX_EARLY_TAPS],
    active_tap_count: usize,
    reverb_send: f32,
    filter: ThreeBandFilter,
    early_line: EarlyReflectionLine,
}

impl VoiceChannel {
    fn new(sample_rate: f32, rx: Consumer<f32>) -> Self {
        Self {
            active: false,
            rx,
            last_sample: 0.0,
            pos_m: Vec3::ZERO,
            direct_gain: Bands3::ONE,
            direct_gl: std::f32::consts::FRAC_1_SQRT_2,
            direct_gr: std::f32::consts::FRAC_1_SQRT_2,
            early_taps: [EarlyTap::default(); MAX_EARLY_TAPS],
            early_gains: [(0.0, 0.0, 1); MAX_EARLY_TAPS],
            active_tap_count: 0,
            reverb_send: 0.25,
            filter: ThreeBandFilter::new(sample_rate),
            early_line: EarlyReflectionLine::new(sample_rate),
        }
    }

    #[inline]
    fn update_spatial_cache(&mut self, listener_pos_m: Vec3, listener_right: Vec3) {
        let (gl, gr) = stereo_pan_gains(self.pos_m - listener_pos_m, listener_right);
        self.direct_gl = gl;
        self.direct_gr = gr;
        self.active_tap_count = self.early_line.prepare_cached_taps(
            &self.early_taps,
            listener_right,
            &mut self.early_gains,
        );
    }
}

struct MixerState {
    clips: Arc<Vec<Vec<f32>>>,
    voices: Vec<Voice>,
    voice_channels: Vec<VoiceChannel>,
    fdn: FdnReverb,
    listener_pos_m: Vec3,
    listener_right: Vec3,
    reverb: ReverbEstimate,
    master_volume: f32,
    voice_volume: f32,
}

impl MixerState {
    #[inline]
    fn prepare_block(&mut self, block_frames: usize) {
        if block_frames > 0 {
            self.fdn.prepare_block(self.reverb, block_frames);
        }
    }

    fn apply_command(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::PlayClip {
                source_id,
                clip,
                volume,
                pitch,
                looping,
                initial_params,
            } => {
                // Reuse inactive voice or steal the oldest active voice
                let slot = self
                    .voices
                    .iter()
                    .position(|v| !v.active)
                    .unwrap_or(0);
                let listener_pos_m = self.listener_pos_m;
                let listener_right = self.listener_right;
                let v = &mut self.voices[slot];
                v.active = true;
                v.source_id = source_id;
                v.clip_idx = clip.index();
                v.cursor = 0.0;
                v.pitch = pitch.clamp(0.5, 2.0);
                v.volume = volume;
                v.looping = looping;
                v.pos_m = initial_params.pos_m;
                v.direct_gain = initial_params.direct_gain;
                v.early_taps = initial_params.early_taps;
                v.reverb_send = initial_params.reverb_send;
                v.filter.reset();
                v.early_line.reset();
                v.update_spatial_cache(listener_pos_m, listener_right);
            }
            AudioCommand::UpdateListener {
                pos_m,
                right,
                reverb,
            } => {
                self.listener_pos_m = pos_m;
                self.listener_right = right.normalize_or_zero();
                self.reverb = reverb;
                let listener_pos_m = self.listener_pos_m;
                let listener_right = self.listener_right;
                for v in &mut self.voices {
                    if v.active {
                        v.update_spatial_cache(listener_pos_m, listener_right);
                    }
                }
                for vc in &mut self.voice_channels {
                    if vc.active {
                        vc.update_spatial_cache(listener_pos_m, listener_right);
                    }
                }
            }
            AudioCommand::UpdateSource(params) => {
                let listener_pos_m = self.listener_pos_m;
                let listener_right = self.listener_right;
                for v in &mut self.voices {
                    if v.active && v.source_id == params.id {
                        v.pos_m = params.pos_m;
                        v.direct_gain = params.direct_gain;
                        v.early_taps = params.early_taps;
                        v.reverb_send = params.reverb_send;
                        v.update_spatial_cache(listener_pos_m, listener_right);
                    }
                }
            }
            AudioCommand::StopSource(id) => {
                for v in &mut self.voices {
                    if v.source_id == id {
                        v.active = false;
                    }
                }
            }
            AudioCommand::UpdateVoiceChannel { slot, params } => {
                let listener_pos_m = self.listener_pos_m;
                let listener_right = self.listener_right;
                if let Some(vc) = self.voice_channels.get_mut(slot) {
                    if !vc.active {
                        vc.active = true;
                        vc.last_sample = 0.0;
                        vc.filter.reset();
                        vc.early_line.reset();
                    }
                    vc.pos_m = params.pos_m;
                    vc.direct_gain = params.direct_gain;
                    vc.early_taps = params.early_taps;
                    vc.reverb_send = params.reverb_send;
                    vc.update_spatial_cache(listener_pos_m, listener_right);
                }
            }
            AudioCommand::ClearVoiceChannel { slot } => {
                if let Some(vc) = self.voice_channels.get_mut(slot) {
                    vc.active = false;
                    vc.last_sample = 0.0;
                    vc.active_tap_count = 0;
                    vc.filter.reset();
                    vc.early_line.reset();
                    while vc.rx.pop().is_ok() {}
                }
            }
            AudioCommand::SetVolumes { master, voice } => {
                if master.is_finite() {
                    self.master_volume = master.clamp(0.0, 2.0);
                }
                if voice.is_finite() {
                    self.voice_volume = voice.clamp(0.0, 2.5);
                }
            }
        }
    }

    #[inline]
    fn render_frame(&mut self) -> (f32, f32) {
        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        let mut rev_in_l = 0.0f32;
        let mut rev_in_r = 0.0f32;

        for v in &mut self.voices {
            if !v.active {
                continue;
            }
            let buf = &self.clips[v.clip_idx];
            let idx = v.cursor as usize;
            if idx >= buf.len() {
                if v.looping {
                    v.cursor = 0.0;
                } else {
                    v.active = false;
                    continue;
                }
            }

            let idx = v.cursor as usize;
            let frac = v.cursor - idx as f32;
            let s0 = buf[idx];
            let s1 = if idx + 1 < buf.len() { buf[idx + 1] } else { 0.0 };
            let raw = (s0 + (s1 - s0) * frac) * v.volume;
            v.cursor += v.pitch;

            // 1. Direct path: 3-band voxel density muffling filter + cached stereo panning
            let muffled = v.filter.process(raw, v.direct_gain);
            let gl = v.direct_gl;
            let gr = v.direct_gr;
            dry_l += muffled * gl;
            dry_r += muffled * gr;

            // 2. Early reflections: precomputed directional echo taps
            let (el, er) = v
                .early_line
                .process_cached(raw, &v.early_gains[..v.active_tap_count]);
            dry_l += el;
            dry_r += er;

            // 3. Shared cavern FDN late reverb send (pre-diffused by early taps & muffled behind walls)
            let send = v.reverb_send;
            rev_in_l += (muffled * gl * 0.62 + el * 0.38) * send;
            rev_in_r += (muffled * gr * 0.62 + er * 0.38) * send;
        }

        // Mix up to MAX_VOICE_CHANNELS concurrent live VoIP streams through the exact same acoustic chain
        for vc in &mut self.voice_channels {
            if !vc.active {
                continue;
            }

            // 1. Pull next PCM sample; ramp smoothly to 0 on underrun to avoid clicks
            let raw = match vc.rx.pop() {
                Ok(sample) => {
                    let s = if sample.is_finite() {
                        sample.clamp(-4.0, 4.0)
                    } else {
                        0.0
                    };
                    vc.last_sample = s;
                    s
                }
                Err(_) => {
                    vc.last_sample *= 0.85;
                    if vc.last_sample.abs() < 1e-5 {
                        vc.last_sample = 0.0;
                    }
                    vc.last_sample
                }
            } * self.voice_volume;

            // 2. Direct path: 3-band voxel density muffling filter + cached stereo equal-power panning
            let muffled = vc.filter.process(raw, vc.direct_gain);
            let gl = vc.direct_gl;
            let gr = vc.direct_gr;
            dry_l += muffled * gl;
            dry_r += muffled * gr;

            // 3. Early reflections: precomputed directional echo taps
            let (el, er) = vc
                .early_line
                .process_cached(raw, &vc.early_gains[..vc.active_tap_count]);
            dry_l += el;
            dry_r += er;

            // 4. Shared cavern FDN late reverb send (pre-diffused by early taps & muffled behind walls)
            let send = vc.reverb_send;
            rev_in_l += (muffled * gl * 0.62 + el * 0.38) * send;
            rev_in_r += (muffled * gr * 0.62 + er * 0.38) * send;
        }

        let (wet_l, wet_r) = self.fdn.process_stereo_sample(rev_in_l, rev_in_r);
        // Zero-transcendental rational Padé soft limiter to guarantee zero harsh digital clipping
        let out_l = fast_soft_clip((dry_l + wet_l) * self.master_volume);
        let out_r = fast_soft_clip((dry_r + wet_r) * self.master_volume);
        (out_l, out_r)
    }
}

/// Rational Padé [3/2] soft-clipping approximation to `tanh(x)` with exact saturation at `±3.0 -> ±1.0`
/// and zero transcendental (`exp`) calls inside the 48 kHz real-time audio callback.
#[inline]
fn fast_soft_clip(x: f32) -> f32 {
    let xc = x.clamp(-3.0, 3.0);
    let x2 = xc * xc;
    xc * (27.0 + x2) / (27.0 + 9.0 * x2)
}

fn create_voice_ring_pairs(
    ring_capacity: usize,
) -> ([Producer<f32>; MAX_VOICE_CHANNELS], Vec<Consumer<f32>>) {
    let mut txs = Vec::with_capacity(MAX_VOICE_CHANNELS);
    let mut rxs = Vec::with_capacity(MAX_VOICE_CHANNELS);
    for _ in 0..MAX_VOICE_CHANNELS {
        let (p, c) = RingBuffer::<f32>::new(ring_capacity);
        txs.push(p);
        rxs.push(c);
    }
    let tx_arr: [Producer<f32>; MAX_VOICE_CHANNELS] = txs
        .try_into()
        .unwrap_or_else(|_| panic!("exact MAX_VOICE_CHANNELS voice ring producers"));
    (tx_arr, rxs)
}

/// Handle to the lock-free real-time `cpal` acoustic DSP audio engine.
pub struct AudioEngine {
    _stream: Option<cpal::Stream>,
    tx: Producer<AudioCommand>,
    voice_txs: [Producer<f32>; MAX_VOICE_CHANNELS],
    voice_slots: [Option<u16>; MAX_VOICE_CHANNELS],
    pub sample_rate: f32,
}

impl AudioEngine {
    /// Starts the real-time `cpal` output stream on the system default output device.
    pub fn start() -> Self {
        Self::start_with_device(None)
    }

    /// Starts the real-time `cpal` output stream on a named output device (falling back to
    /// the system default if `device_name` is `None` or not found). Falls back cleanly to a
    /// silent no-op handle on headless machines without an audio device.
    pub fn start_with_device(device_name: Option<&str>) -> Self {
        let (tx, mut rx) = RingBuffer::<AudioCommand>::new(512);

        let host = cpal::default_host();
        let selected = device_name.and_then(|wanted| {
            host.output_devices().ok()?.find(|d| {
                d.description()
                    .map(|desc| desc.name() == wanted)
                    .unwrap_or(false)
            })
        });
        let Some(device) = selected.or_else(|| host.default_output_device()) else {
            let (voice_txs, _rxs) = create_voice_ring_pairs(VOICE_RING_CAPACITY);
            return Self {
                _stream: None,
                tx,
                voice_txs,
                voice_slots: [None; MAX_VOICE_CHANNELS],
                sample_rate: 48_000.0,
            };
        };

        let Ok(default_cfg) = device.default_output_config() else {
            let (voice_txs, _rxs) = create_voice_ring_pairs(VOICE_RING_CAPACITY);
            return Self {
                _stream: None,
                tx,
                voice_txs,
                voice_slots: [None; MAX_VOICE_CHANNELS],
                sample_rate: 48_000.0,
            };
        };

        let sample_rate = default_cfg.sample_rate() as f32;
        let channels = default_cfg.channels() as usize;
        let config: cpal::StreamConfig = default_cfg.into();

        let ring_cap = ((sample_rate * 0.2) as usize).max(VOICE_RING_CAPACITY);
        let (voice_txs, voice_rxs) = create_voice_ring_pairs(ring_cap);

        let clips = Arc::new(
            SynthClip::ALL
                .iter()
                .map(|&clip| load_or_generate_clip(clip, sample_rate))
                .collect(),
        );

        let voice_channels = voice_rxs
            .into_iter()
            .map(|vrx| VoiceChannel::new(sample_rate, vrx))
            .collect();

        let mut mixer = MixerState {
            clips,
            voices: (0..MAX_VOICES).map(|_| Voice::new(sample_rate)).collect(),
            voice_channels,
            fdn: FdnReverb::new(sample_rate),
            listener_pos_m: Vec3::ZERO,
            listener_right: Vec3::X,
            reverb: ReverbEstimate::default(),
            master_volume: 1.0,
            voice_volume: 1.0,
        };

        let stream = device
            .build_output_stream(
                config,
                move |data: &mut [f32], _| {
                    drain_commands(&mut rx, &mut mixer);
                    let ch = channels.max(1);
                    mixer.prepare_block(data.len() / ch);
                    for frame in data.chunks_mut(ch) {
                        let (l, r) = mixer.render_frame();
                        if ch == 1 {
                            frame[0] = (l + r) * 0.5;
                        } else {
                            frame[0] = l;
                            frame[1] = r;
                            for extra in &mut frame[2..] {
                                *extra = 0.0;
                            }
                        }
                    }
                },
                |err| eprintln!("[voxel-audio-dsp] stream error: {err}"),
                None,
            )
            .ok();

        if let Some(ref s) = stream {
            let _ = s.play();
        }

        Self {
            _stream: stream,
            tx,
            voice_txs,
            voice_slots: [None; MAX_VOICE_CHANNELS],
            sample_rate,
        }
    }

    pub fn send(&mut self, cmd: AudioCommand) {
        let _ = self.tx.push(cmd);
    }

    fn find_or_alloc_voice_slot(&mut self, speaker_id: u16) -> Option<(usize, bool)> {
        if let Some(slot) = self
            .voice_slots
            .iter()
            .position(|&id| id == Some(speaker_id))
        {
            return Some((slot, false));
        }
        let free = self.voice_slots.iter().position(|id| id.is_none())?;
        self.voice_slots[free] = Some(speaker_id);
        Some((free, true))
    }

    /// Allocates a streaming voice channel slot if new, ensures it is active, and updates
    /// its solved voxel-density occlusion, early reflections, and cavern reverb send.
    pub fn update_voice_acoustics(&mut self, speaker_id: u16, params: SourceAcousticParams) {
        if let Some((slot, _)) = self.find_or_alloc_voice_slot(speaker_id) {
            self.send(AudioCommand::UpdateVoiceChannel { slot, params });
        }
    }

    /// Pushes decoded PCM (already resampled to `self.sample_rate`) into the speaker's
    /// lock-free ring buffer, safely dropping excess samples if full.
    ///
    /// If the speaker's voice slot is newly allocated, immediately activates the channel
    /// with unity direct gain so incoming speech (including Host `speaker_id == 0`) is
    /// audible right away even before the 20 Hz acoustic worker completes its next pass.
    pub fn push_voice_pcm(&mut self, speaker_id: u16, pcm_at_output_rate: &[f32]) {
        if pcm_at_output_rate.is_empty() {
            return;
        }
        let Some((slot, newly_allocated)) = self.find_or_alloc_voice_slot(speaker_id) else {
            return;
        };
        if newly_allocated {
            self.send(AudioCommand::UpdateVoiceChannel {
                slot,
                params: SourceAcousticParams {
                    id: u64::from(speaker_id),
                    pos_m: Vec3::ZERO,
                    direct_gain: Bands3::ONE,
                    solid_thickness_m: 0.0,
                    distance_m: 1.0,
                    early_taps: [EarlyTap::default(); MAX_EARLY_TAPS],
                    reverb_send: 0.18,
                },
            });
        }
        let prod = &mut self.voice_txs[slot];
        let free = prod.slots();
        if free == 0 {
            return;
        }
        // Keep the newest tail if incoming audio exceeds remaining ring buffer headroom
        let start = pcm_at_output_rate.len().saturating_sub(free);
        for &sample in &pcm_at_output_rate[start..] {
            if prod.push(sample).is_err() {
                break;
            }
        }
    }

    /// Releases a speaker's streaming voice channel slot and clears its DSP delay state.
    pub fn remove_voice_speaker(&mut self, speaker_id: u16) {
        if let Some(slot) = self
            .voice_slots
            .iter()
            .position(|&id| id == Some(speaker_id))
        {
            self.voice_slots[slot] = None;
            self.send(AudioCommand::ClearVoiceChannel { slot });
        }
    }
}

fn drain_commands(rx: &mut Consumer<AudioCommand>, mixer: &mut MixerState) {
    while let Ok(cmd) = rx.pop() {
        mixer.apply_command(cmd);
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    #[test]
    fn streaming_voice_channel_mixes_through_full_acoustic_chain() {
        let sample_rate = 48_000.0;
        let (mut voice_txs, voice_rxs) = create_voice_ring_pairs(VOICE_RING_CAPACITY);

        let voice_channels = voice_rxs
            .into_iter()
            .map(|vrx| VoiceChannel::new(sample_rate, vrx))
            .collect();

        let mut mixer = MixerState {
            clips: Arc::new(Vec::new()),
            voices: Vec::new(),
            voice_channels,
            fdn: FdnReverb::new(sample_rate),
            listener_pos_m: Vec3::ZERO,
            listener_right: Vec3::X,
            reverb: ReverbEstimate {
                rt60_sec: Bands3::new(2.4, 1.9, 1.1),
                mean_free_path_m: 14.0,
                wet_mix: 0.55,
                enclosure: 0.9,
            },
            master_volume: 1.0,
            voice_volume: 1.0,
        };

        let mut early_taps = [EarlyTap::default(); MAX_EARLY_TAPS];
        early_taps[0] = EarlyTap {
            delay_sec: 0.022,
            gain: Bands3::new(0.45, 0.35, 0.2),
            direction: Vec3::new(-1.0, 0.0, 0.0),
            active: true,
        };
        early_taps[1] = EarlyTap {
            delay_sec: 0.041,
            gain: Bands3::new(0.30, 0.22, 0.12),
            direction: Vec3::new(0.0, 0.0, -1.0),
            active: true,
        };

        mixer.apply_command(AudioCommand::UpdateVoiceChannel {
            slot: 0,
            params: SourceAcousticParams {
                id: 1,
                pos_m: Vec3::new(4.0, 0.0, 0.0), // To the listener's right
                direct_gain: Bands3::new(0.8, 0.45, 0.15),
                solid_thickness_m: 0.5,
                distance_m: 4.0,
                early_taps,
                reverb_send: 0.5,
            },
        });

        // Push 40 ms of voice-band tone into slot 0
        let n_voice = 1920usize;
        for i in 0..n_voice {
            let t = i as f32 / sample_rate;
            let s = (TAU * 300.0 * t).sin() * 0.5 + (TAU * 1200.0 * t).sin() * 0.25;
            voice_txs[0].push(s).expect("push voice sample");
        }

        // Render 120 ms total (40 ms live audio + 80 ms underrun/echo/reverb tail)
        let mut active_energy_l = 0.0f32;
        let mut active_energy_r = 0.0f32;
        let mut tail_energy = 0.0f32;

        for i in 0..(n_voice * 3) {
            if i % 256 == 0 {
                mixer.prepare_block(256);
            }
            let (l, r) = mixer.render_frame();
            assert!(
                l.is_finite() && r.is_finite(),
                "non-finite output at sample {i}: ({l}, {r})"
            );
            assert!(l.abs() <= 1.0 && r.abs() <= 1.0);

            if i < n_voice {
                active_energy_l += l * l;
                active_energy_r += r * r;
            } else if i >= n_voice + 200 {
                tail_energy += l * l + r * r;
            }
        }

        // Source is to the right (+X), so right channel should dominate during direct voice
        assert!(
            active_energy_r > active_energy_l && active_energy_r > 1.0,
            "expected right-panned voice energy: L={active_energy_l}, R={active_energy_r}"
        );
        // Early reflections + cavern FDN reverb tail should carry audible energy after voice stops
        assert!(
            tail_energy > 0.01,
            "expected audible early reflection + FDN reverb tail: {tail_energy}"
        );

        // Rotating the listener 180 degrees (-X right) must recompute cached pan gains so left dominates
        mixer.apply_command(AudioCommand::UpdateListener {
            pos_m: Vec3::ZERO,
            right: Vec3::NEG_X,
            reverb: mixer.reverb,
        });
        for i in 0..512 {
            let t = i as f32 / sample_rate;
            let s = (TAU * 400.0 * t).sin() * 0.5;
            voice_txs[0].push(s).expect("push voice sample after turn");
        }
        mixer.prepare_block(512);
        let mut turned_l = 0.0f32;
        let mut turned_r = 0.0f32;
        for _ in 0..512 {
            let (l, r) = mixer.render_frame();
            turned_l += l * l;
            turned_r += r * r;
        }
        assert!(
            turned_l > turned_r,
            "expected left-panned voice after listener turn: L={turned_l}, R={turned_r}"
        );

        // Master mute via SetVolumes must produce exact silence
        mixer.apply_command(AudioCommand::SetVolumes {
            master: 0.0,
            voice: 1.0,
        });
        for _ in 0..64 {
            let (l, r) = mixer.render_frame();
            assert_eq!(l, 0.0);
            assert_eq!(r, 0.0);
        }

        // Device enumeration must never panic
        let _ = list_output_devices();
    }

    #[test]
    fn push_voice_pcm_auto_activates_newly_allocated_host_channel() {
        let sample_rate = 48_000.0;
        let (tx, mut rx) = RingBuffer::<AudioCommand>::new(64);
        let (voice_txs, voice_rxs) = create_voice_ring_pairs(VOICE_RING_CAPACITY);

        let mut engine = AudioEngine {
            _stream: None,
            tx,
            voice_txs,
            voice_slots: [None; MAX_VOICE_CHANNELS],
            sample_rate,
        };

        let voice_channels = voice_rxs
            .into_iter()
            .map(|vrx| VoiceChannel::new(sample_rate, vrx))
            .collect();

        let mut mixer = MixerState {
            clips: Arc::new(Vec::new()),
            voices: Vec::new(),
            voice_channels,
            fdn: FdnReverb::new(sample_rate),
            listener_pos_m: Vec3::ZERO,
            listener_right: Vec3::X,
            reverb: ReverbEstimate::default(),
            master_volume: 1.0,
            voice_volume: 1.0,
        };

        // Push PCM for Host (speaker_id == 0) BEFORE update_voice_acoustics is ever called
        let pcm: Vec<f32> = (0..960)
            .map(|i| (TAU * 360.0 * i as f32 / sample_rate).sin() * 0.5)
            .collect();
        engine.push_voice_pcm(0, &pcm);

        drain_commands(&mut rx, &mut mixer);
        mixer.prepare_block(960);

        let mut energy = 0.0f32;
        for _ in 0..960 {
            let (l, r) = mixer.render_frame();
            assert!(l.is_finite() && r.is_finite());
            energy += l * l + r * r;
        }

        assert!(
            energy > 1.0,
            "expected push_voice_pcm(0, ...) to auto-activate channel and produce audible output before update_voice_acoustics, got energy={energy}"
        );
    }
}
