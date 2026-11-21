//! Microphone capture with cpal. Records mono f32, resamples to 16 kHz for Whisper.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};
use std::sync::{Arc, Mutex};

pub const RATE: u32 = 16_000;

pub struct Recording {
    stream: cpal::Stream,
    buf: Arc<Mutex<Vec<f32>>>,
    rate: u32,
}

pub fn start(on_level: impl Fn(f32) + Send + Sync + 'static) -> Result<Recording, String> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or("No microphone found")?;
    let config = device.default_input_config().map_err(|e| format!("Microphone unavailable: {e}"))?;
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let buf = Arc::new(Mutex::new(Vec::<f32>::with_capacity(rate as usize * 30)));
    let on_level = Arc::new(on_level);
    let fmt = config.sample_format();
    let cfg: cpal::StreamConfig = config.into();

    let stream = match fmt {
        cpal::SampleFormat::F32 => build::<f32>(&device, &cfg, channels, rate, buf.clone(), on_level),
        cpal::SampleFormat::I16 => build::<i16>(&device, &cfg, channels, rate, buf.clone(), on_level),
        cpal::SampleFormat::U16 => build::<u16>(&device, &cfg, channels, rate, buf.clone(), on_level),
        cpal::SampleFormat::I32 => build::<i32>(&device, &cfg, channels, rate, buf.clone(), on_level),
        other => return Err(format!("Unsupported microphone format {other:?}")),
    }?;
    stream.play().map_err(|e| format!("Could not start microphone: {e}"))?;
    Ok(Recording { stream, buf, rate })
}

fn build<T>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    channels: usize,
    rate: u32,
    buf: Arc<Mutex<Vec<f32>>>,
    on_level: Arc<dyn Fn(f32) + Send + Sync>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let block = (rate / 25) as usize; // ~40 ms level updates
    let mut acc = 0f32;
    let mut n = 0usize;
    device
        .build_input_stream(
            cfg,
            move |data: &[T], _| {
                let mut b = buf.lock().unwrap();
                for frame in data.chunks(channels.max(1)) {
                    let s = frame.iter().map(|x| f32::from_sample(*x)).sum::<f32>() / frame.len() as f32;
                    b.push(s);
                    acc += s * s;
                    n += 1;
                    if n >= block {
                        on_level((acc / n as f32).sqrt());
                        acc = 0.0;
                        n = 0;
                    }
                }
            },
            |e| crate::log!("mic stream error: {e}"),
            None,
        )
        .map_err(|e| format!("Could not open microphone: {e}"))
}

impl Recording {
    /// Stop recording and return 16 kHz mono samples.
    pub fn finish(self) -> Vec<f32> {
        drop(self.stream);
        let raw = std::mem::take(&mut *self.buf.lock().unwrap());
        resample(&raw, self.rate, RATE)
    }
}

fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = (input.len() as f64 / ratio) as usize;
    if ratio > 1.0 {
        // Downsample: average the source window (cheap low-pass).
        (0..out_len)
            .map(|i| {
                let a = (i as f64 * ratio) as usize;
                let b = (((i + 1) as f64 * ratio) as usize).min(input.len()).max(a + 1);
                input[a..b].iter().sum::<f32>() / (b - a) as f32
            })
            .collect()
    } else {
        (0..out_len)
            .map(|i| {
                let p = i as f64 * ratio;
                let a = p as usize;
                let f = (p - a as f64) as f32;
                let x0 = input[a.min(input.len() - 1)];
                let x1 = input[(a + 1).min(input.len() - 1)];
                x0 + (x1 - x0) * f
            })
            .collect()
    }
}

/// Loudest 40 ms window (RMS). Used to skip silent recordings, which make Whisper hallucinate.
pub fn peak_rms(samples: &[f32]) -> f32 {
    samples
        .chunks(640)
        .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt())
        .fold(0.0, f32::max)
}

/// Encode 16 kHz mono samples as a 16-bit PCM WAV file in memory.
pub fn to_wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        w.extend_from_slice(&v.to_le_bytes());
    }
    w
}

/// Read a 16-bit PCM WAV (any rate/channels) into 16 kHz mono. Used by the --transcribe test mode.
pub fn read_wav(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut pos = 12;
    let (mut channels, mut rate, mut bits) = (1u16, 16000u32, 16u16);
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..(pos + 8 + len).min(bytes.len())];
        if id == b"fmt " {
            channels = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            if bits != 16 {
                return Err("only 16-bit WAV supported".into());
            }
            let mono: Vec<f32> = body
                .chunks_exact(2 * channels as usize)
                .map(|f| {
                    f.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).sum::<f32>() / channels as f32
                })
                .collect();
            return Ok(resample(&mono, rate, RATE));
        }
        pos += 8 + len + (len & 1);
    }
    Err("WAV has no data chunk".into())
}
